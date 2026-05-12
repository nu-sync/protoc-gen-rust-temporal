//! Default implementation of the `crate::temporal_runtime` facade that
//! `protoc-gen-rust-temporal` emits calls against. Backed by
//! `temporalio-client = "=0.4"` (exact-patch pinned — bridge crate minor
//! versions track SDK reshapes; plugin emit is unaffected).
//!
//! # Usage
//!
//! Add the dep and re-export from your crate's `lib.rs`:
//!
//! ```toml
//! [dependencies]
//! temporal-proto-runtime-bridge = "0.1"
//! ```
//!
//! ```ignore
//! pub use temporal_proto_runtime_bridge as temporal_runtime;
//! ```
//!
//! That's the whole wiring. The hand-written `temporal_runtime.rs` becomes
//! optional — only consumers who stub for tests or pin a vendored SDK keep
//! their own.
//!
//! See `docs/RUNTIME-API.md` for the contract this crate implements.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use temporalio_client::grpc::WorkflowService;
use temporalio_client::tonic::IntoRequest;
use temporalio_client::{
    Client, NamespacedClient, UntypedQuery, UntypedSignal, UntypedUpdate, UntypedWorkflowHandle,
    WorkflowGetResultOptions, WorkflowQueryOptions, WorkflowSignalOptions, WorkflowStartOptions,
    WorkflowStartSignal, WorkflowStartUpdateOptions, WorkflowUpdateWaitStage,
};
use temporalio_common::UntypedWorkflow;
use temporalio_common::data_converters::RawValue;
use temporalio_common::protos::temporal::api::common::v1::{
    Payload, Payloads, WorkflowExecution, WorkflowType,
};
use temporalio_common::protos::temporal::api::enums::v1 as sdk_enums;
use temporalio_common::protos::temporal::api::enums::v1::{TaskQueueKind, WorkflowIdConflictPolicy};
use temporalio_common::protos::temporal::api::taskqueue::v1::TaskQueue;
use temporalio_common::protos::temporal::api::update::v1 as update;
use temporalio_common::protos::temporal::api::update::v1::WaitPolicy as ProtoWaitPolicy;
use temporalio_common::protos::temporal::api::workflowservice::v1::execute_multi_operation_request::{
    Operation, operation::Operation as OperationKind,
};
use temporalio_common::protos::temporal::api::workflowservice::v1::execute_multi_operation_response::response::Response as RespKind;
use temporalio_common::protos::temporal::api::workflowservice::v1::{
    ExecuteMultiOperationRequest, StartWorkflowExecutionRequest, UpdateWorkflowExecutionRequest,
};

pub use temporal_proto_runtime::TemporalProtoMessage;

/// Encoding constant for the wire-format triple (`metadata.encoding`).
const ENCODING: &str = temporal_proto_runtime::ENCODING;

/// Opaque handle on the Temporal client connection. Cheaply cloneable via
/// `Arc`. Constructed by [`connect`] or directly by the consumer.
#[derive(Clone)]
pub struct TemporalClient {
    inner: Arc<Client>,
}

impl TemporalClient {
    /// Wrap an existing `temporalio_client::Client` (already-constructed by
    /// the consumer, e.g. via custom transport).
    pub fn from_client(client: Client) -> Self {
        Self {
            inner: Arc::new(client),
        }
    }

    /// Wrap a shared `Arc<Client>` without re-wrapping.
    pub fn from_arc(client: Arc<Client>) -> Self {
        Self { inner: client }
    }

    /// Borrow the underlying SDK client. Escape hatch for power users who
    /// need SDK-typed access (e.g. for features the facade hasn't grown yet).
    pub fn sdk(&self) -> &Client {
        &self.inner
    }
}

/// Live workflow handle. Stores the workflow id (and run id if known) so we
/// can re-derive an `UntypedWorkflowHandle` per call without lifetime tying.
pub struct WorkflowHandle {
    client: TemporalClient,
    workflow_id: String,
    run_id: Option<String>,
}

impl WorkflowHandle {
    /// The workflow id. Always populated.
    pub fn workflow_id(&self) -> &str {
        &self.workflow_id
    }

    /// The run id, if known. Populated by `start_workflow_proto*`;
    /// `attach_handle` leaves it `None` (consumer didn't supply one).
    pub fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }

    fn untyped(&self) -> UntypedWorkflowHandle<Client> {
        self.client
            .inner
            .get_workflow_handle::<UntypedWorkflow>(&self.workflow_id)
    }
}

/// Mirror of cludden's `IDReusePolicy`. Variants match the proto enum modulo
/// the unspecified default (we model that as `Option::None` at call sites).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowIdReusePolicy {
    AllowDuplicate,
    AllowDuplicateFailedOnly,
    RejectDuplicate,
    TerminateIfRunning,
}

impl From<WorkflowIdReusePolicy> for sdk_enums::WorkflowIdReusePolicy {
    fn from(value: WorkflowIdReusePolicy) -> Self {
        // SDK marks TERMINATE_IF_RUNNING deprecated (recommends the
        // WorkflowIdConflictPolicy::TerminateExisting replacement). Cludden's
        // schema still exposes it as a valid IDReusePolicy variant for parity
        // with the Go runtime, so we preserve it here too.
        #[allow(deprecated)]
        match value {
            WorkflowIdReusePolicy::AllowDuplicate => Self::AllowDuplicate,
            WorkflowIdReusePolicy::AllowDuplicateFailedOnly => Self::AllowDuplicateFailedOnly,
            WorkflowIdReusePolicy::RejectDuplicate => Self::RejectDuplicate,
            WorkflowIdReusePolicy::TerminateIfRunning => Self::TerminateIfRunning,
        }
    }
}

/// Update stage to wait for before the update call returns. The Rust facade
/// always returns the update's output, so the call site still blocks on
/// completion; `WaitPolicy` controls the *stage acknowledgement* level the
/// server reports back at, not whether `get_result` is awaited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitPolicy {
    Admitted,
    Accepted,
    Completed,
}

impl From<WaitPolicy> for sdk_enums::UpdateWorkflowExecutionLifecycleStage {
    fn from(value: WaitPolicy) -> Self {
        match value {
            WaitPolicy::Admitted => Self::Admitted,
            WaitPolicy::Accepted => Self::Accepted,
            WaitPolicy::Completed => Self::Completed,
        }
    }
}

/// Convenience: build a single `binary/protobuf` payload from a prost message.
fn encode_proto_payload<T: TemporalProtoMessage>(msg: &T) -> Payload {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("encoding".to_string(), ENCODING.as_bytes().to_vec());
    metadata.insert(
        "messageType".to_string(),
        T::MESSAGE_TYPE.as_bytes().to_vec(),
    );
    Payload {
        metadata,
        data: prost::Message::encode_to_vec(msg),
        external_payloads: vec![],
    }
}

/// Convenience: decode a single `binary/protobuf` payload back into a prost
/// message. Metadata mismatch is *not* checked here — the wire-format invariant
/// is asserted by `temporal-proto-runtime`'s `TemporalDeserializable` impl;
/// this helper is only reached after the SDK has already validated metadata.
fn decode_proto_payload<T: TemporalProtoMessage>(
    payload: &Payload,
) -> std::result::Result<T, prost::DecodeError> {
    T::decode(payload.data.as_slice())
}

// ── Client construction ────────────────────────────────────────────────

/// Connect to a Temporal frontend and produce a [`TemporalClient`].
///
/// Convenience for the common case. Power users who need custom transport,
/// interceptors, or a vendored `Client` should construct one themselves and
/// call [`TemporalClient::from_client`] or [`TemporalClient::from_arc`].
pub async fn connect(url: &str, namespace: &str) -> Result<TemporalClient> {
    use temporalio_client::{ClientOptions, Connection, ConnectionOptions};
    use url::Url;

    let parsed = Url::parse(url).with_context(|| format!("parse temporal url {url}"))?;
    let connection = Connection::connect(ConnectionOptions::new(parsed).build())
        .await
        .context("connect to Temporal frontend")?;
    let client = Client::new(connection, ClientOptions::new(namespace.to_string()).build())
        .context("build Temporal client")?;
    Ok(TemporalClient::from_client(client))
}

// ── Workflow lifecycle ─────────────────────────────────────────────────

/// Attach to an existing workflow by ID. The returned handle has no run id
/// (the SDK will resolve to the most-recent run on each call).
pub fn attach_handle(client: &TemporalClient, workflow_id: String) -> WorkflowHandle {
    WorkflowHandle {
        client: client.clone(),
        workflow_id,
        run_id: None,
    }
}

/// Generate a fresh random workflow id. Used by the plugin when a workflow
/// has no proto-level `id` template — templated ids are materialised inline
/// as `<wf>_id(...)` functions and never reach this call site.
pub fn random_workflow_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Start a workflow with a proto-encoded input.
#[allow(clippy::too_many_arguments)]
pub async fn start_workflow_proto<I>(
    client: &TemporalClient,
    workflow_name: &'static str,
    workflow_id: &str,
    task_queue: &str,
    input: &I,
    id_reuse_policy: Option<WorkflowIdReusePolicy>,
    execution_timeout: Option<Duration>,
    run_timeout: Option<Duration>,
    task_timeout: Option<Duration>,
) -> Result<WorkflowHandle>
where
    I: TemporalProtoMessage,
{
    let payload = encode_proto_payload(input);
    let raw = RawValue::new(vec![payload]);
    let base = WorkflowStartOptions::new(task_queue.to_string(), workflow_id.to_string())
        .maybe_execution_timeout(execution_timeout)
        .maybe_run_timeout(run_timeout)
        .maybe_task_timeout(task_timeout);
    // bon builders use typestate — id_reuse_policy has #[builder(default)],
    // so we only call the setter when present.
    let options = match id_reuse_policy {
        Some(p) => base.id_reuse_policy(p.into()).build(),
        None => base.build(),
    };
    let handle = client
        .sdk()
        .start_workflow(UntypedWorkflow::new(workflow_name), raw, options)
        .await
        .with_context(|| format!("start workflow {workflow_name}"))?;
    let info = handle.info().clone();
    Ok(WorkflowHandle {
        client: client.clone(),
        workflow_id: info.workflow_id,
        run_id: info.run_id,
    })
}

/// Start a workflow whose input is `google.protobuf.Empty`. The plugin
/// emits a call to this function instead of `start_workflow_proto` when
/// the input message is Empty, avoiding the need to express `()` as a
/// `TemporalProtoMessage`.
#[allow(clippy::too_many_arguments)]
pub async fn start_workflow_proto_empty(
    client: &TemporalClient,
    workflow_name: &'static str,
    workflow_id: &str,
    task_queue: &str,
    id_reuse_policy: Option<WorkflowIdReusePolicy>,
    execution_timeout: Option<Duration>,
    run_timeout: Option<Duration>,
    task_timeout: Option<Duration>,
) -> Result<WorkflowHandle> {
    let raw = RawValue::new(vec![]);
    let base = WorkflowStartOptions::new(task_queue.to_string(), workflow_id.to_string())
        .maybe_execution_timeout(execution_timeout)
        .maybe_run_timeout(run_timeout)
        .maybe_task_timeout(task_timeout);
    let options = match id_reuse_policy {
        Some(p) => base.id_reuse_policy(p.into()).build(),
        None => base.build(),
    };
    let handle = client
        .sdk()
        .start_workflow(UntypedWorkflow::new(workflow_name), raw, options)
        .await
        .with_context(|| format!("start workflow {workflow_name}"))?;
    let info = handle.info().clone();
    Ok(WorkflowHandle {
        client: client.clone(),
        workflow_id: info.workflow_id,
        run_id: info.run_id,
    })
}

/// Wait for a workflow to complete and decode its single proto output.
pub async fn wait_result_proto<O>(handle: &WorkflowHandle) -> Result<O>
where
    O: TemporalProtoMessage,
{
    let raw = handle
        .untyped()
        .get_result(WorkflowGetResultOptions::builder().build())
        .await
        .context("await workflow result")?;
    let payload = raw
        .payloads
        .first()
        .context("workflow returned no payloads")?;
    decode_proto_payload::<O>(payload).context("decode workflow output")
}

/// Wait variant for workflows that return `google.protobuf.Empty`.
pub async fn wait_result_unit(handle: &WorkflowHandle) -> Result<()> {
    handle
        .untyped()
        .get_result(WorkflowGetResultOptions::builder().build())
        .await
        .context("await workflow result")?;
    Ok(())
}

// ── Signals ────────────────────────────────────────────────────────────

/// Send a typed signal with proto input.
pub async fn signal_proto<I>(handle: &WorkflowHandle, name: &str, input: &I) -> Result<()>
where
    I: TemporalProtoMessage,
{
    let payload = encode_proto_payload(input);
    let raw = RawValue::new(vec![payload]);
    handle
        .untyped()
        .signal(
            UntypedSignal::<UntypedWorkflow>::new(name),
            raw,
            WorkflowSignalOptions::builder().build(),
        )
        .await
        .with_context(|| format!("send signal {name}"))?;
    Ok(())
}

/// Send a signal whose input is `google.protobuf.Empty`.
pub async fn signal_unit(handle: &WorkflowHandle, name: &str) -> Result<()> {
    let raw = RawValue::new(vec![]);
    handle
        .untyped()
        .signal(
            UntypedSignal::<UntypedWorkflow>::new(name),
            raw,
            WorkflowSignalOptions::builder().build(),
        )
        .await
        .with_context(|| format!("send signal {name}"))?;
    Ok(())
}

// ── Queries ────────────────────────────────────────────────────────────

/// Run a query with proto input and decode the typed response.
pub async fn query_proto<I, O>(handle: &WorkflowHandle, name: &str, input: &I) -> Result<O>
where
    I: TemporalProtoMessage,
    O: TemporalProtoMessage,
{
    let payload = encode_proto_payload(input);
    let raw_input = RawValue::new(vec![payload]);
    let raw_out: RawValue = handle
        .untyped()
        .query(
            UntypedQuery::<UntypedWorkflow>::new(name),
            raw_input,
            WorkflowQueryOptions::builder().build(),
        )
        .await
        .with_context(|| format!("run query {name}"))?;
    let payload = raw_out
        .payloads
        .first()
        .context("query returned no payloads")?;
    decode_proto_payload::<O>(payload).context("decode query output")
}

/// Run a query whose input is `google.protobuf.Empty`.
pub async fn query_proto_empty<O>(handle: &WorkflowHandle, name: &str) -> Result<O>
where
    O: TemporalProtoMessage,
{
    let raw_input = RawValue::new(vec![]);
    let raw_out: RawValue = handle
        .untyped()
        .query(
            UntypedQuery::<UntypedWorkflow>::new(name),
            raw_input,
            WorkflowQueryOptions::builder().build(),
        )
        .await
        .with_context(|| format!("run query {name}"))?;
    let payload = raw_out
        .payloads
        .first()
        .context("query returned no payloads")?;
    decode_proto_payload::<O>(payload).context("decode query output")
}

// ── Updates ────────────────────────────────────────────────────────────

fn wait_stage_from(policy: WaitPolicy) -> WorkflowUpdateWaitStage {
    match policy {
        WaitPolicy::Admitted => WorkflowUpdateWaitStage::Admitted,
        WaitPolicy::Accepted => WorkflowUpdateWaitStage::Accepted,
        WaitPolicy::Completed => WorkflowUpdateWaitStage::Completed,
    }
}

/// Send an update with proto input and wait for the result.
pub async fn update_proto<I, O>(
    handle: &WorkflowHandle,
    name: &str,
    input: &I,
    wait_policy: WaitPolicy,
) -> Result<O>
where
    I: TemporalProtoMessage,
    O: TemporalProtoMessage,
{
    let payload = encode_proto_payload(input);
    let raw_input = RawValue::new(vec![payload]);
    let update_handle = handle
        .untyped()
        .start_update(
            UntypedUpdate::<UntypedWorkflow>::new(name),
            raw_input,
            WorkflowStartUpdateOptions::builder()
                .wait_for_stage(wait_stage_from(wait_policy))
                .build(),
        )
        .await
        .with_context(|| format!("start update {name}"))?;
    let raw_out: RawValue = update_handle
        .get_result()
        .await
        .with_context(|| format!("await update {name} result"))?;
    let payload = raw_out
        .payloads
        .first()
        .context("update returned no payloads")?;
    decode_proto_payload::<O>(payload).context("decode update output")
}

/// Send an update whose input is `google.protobuf.Empty`.
pub async fn update_proto_empty<O>(
    handle: &WorkflowHandle,
    name: &str,
    wait_policy: WaitPolicy,
) -> Result<O>
where
    O: TemporalProtoMessage,
{
    let raw_input = RawValue::new(vec![]);
    let update_handle = handle
        .untyped()
        .start_update(
            UntypedUpdate::<UntypedWorkflow>::new(name),
            raw_input,
            WorkflowStartUpdateOptions::builder()
                .wait_for_stage(wait_stage_from(wait_policy))
                .build(),
        )
        .await
        .with_context(|| format!("start update {name}"))?;
    let raw_out: RawValue = update_handle
        .get_result()
        .await
        .with_context(|| format!("await update {name} result"))?;
    let payload = raw_out
        .payloads
        .first()
        .context("update returned no payloads")?;
    decode_proto_payload::<O>(payload).context("decode update output")
}

// ── With-start helpers ─────────────────────────────────────────────────

/// Atomically start a workflow and send it an initial signal. The plugin
/// emits a call to this function alongside the generated client whenever a
/// signal annotation declares `start: true`.
#[allow(clippy::too_many_arguments)]
pub async fn signal_with_start_workflow_proto<W, S>(
    client: &TemporalClient,
    workflow_name: &'static str,
    workflow_id: &str,
    task_queue: &str,
    workflow_input: &W,
    signal_name: &str,
    signal_input: &S,
    id_reuse_policy: Option<WorkflowIdReusePolicy>,
    execution_timeout: Option<Duration>,
    run_timeout: Option<Duration>,
    task_timeout: Option<Duration>,
) -> Result<WorkflowHandle>
where
    W: TemporalProtoMessage,
    S: TemporalProtoMessage,
{
    let workflow_payload = encode_proto_payload(workflow_input);
    let signal_payload = encode_proto_payload(signal_input);
    let workflow_raw = RawValue::new(vec![workflow_payload]);
    let signal_payloads = Payloads {
        payloads: vec![signal_payload],
    };

    let start_signal = WorkflowStartSignal::new(signal_name.to_string())
        .input(signal_payloads)
        .build();

    let base = WorkflowStartOptions::new(task_queue.to_string(), workflow_id.to_string())
        .maybe_execution_timeout(execution_timeout)
        .maybe_run_timeout(run_timeout)
        .maybe_task_timeout(task_timeout)
        .start_signal(start_signal);
    let options = match id_reuse_policy {
        Some(p) => base.id_reuse_policy(p.into()).build(),
        None => base.build(),
    };

    let handle = client
        .sdk()
        .start_workflow(UntypedWorkflow::new(workflow_name), workflow_raw, options)
        .await
        .with_context(|| format!("signal-with-start workflow {workflow_name}"))?;
    let info = handle.info().clone();
    Ok(WorkflowHandle {
        client: client.clone(),
        workflow_id: info.workflow_id,
        run_id: info.run_id,
    })
}

/// Atomically start a workflow and send it an initial update. The plugin
/// emits a call to this function alongside the generated client whenever an
/// update annotation declares `start: true`.
///
/// Backed by the server's `ExecuteMultiOperationRequest` gRPC, since
/// `temporalio-client 0.4` doesn't expose a friendly wrapper for this combo.
#[allow(clippy::too_many_arguments)]
pub async fn update_with_start_workflow_proto<W, U, O>(
    client: &TemporalClient,
    workflow_name: &'static str,
    workflow_id: &str,
    task_queue: &str,
    workflow_input: &W,
    update_name: &str,
    update_input: &U,
    wait_policy: WaitPolicy,
    id_reuse_policy: Option<WorkflowIdReusePolicy>,
    execution_timeout: Option<Duration>,
    run_timeout: Option<Duration>,
    task_timeout: Option<Duration>,
) -> Result<(WorkflowHandle, O)>
where
    W: TemporalProtoMessage,
    U: TemporalProtoMessage,
    O: TemporalProtoMessage,
{
    let sdk_client = client.sdk();
    let namespace = sdk_client.namespace();
    let identity = sdk_client.identity();

    let workflow_payload = encode_proto_payload(workflow_input);
    let update_payload = encode_proto_payload(update_input);

    let id_reuse = id_reuse_policy
        .map(sdk_enums::WorkflowIdReusePolicy::from)
        .unwrap_or(sdk_enums::WorkflowIdReusePolicy::Unspecified) as i32;

    let start = StartWorkflowExecutionRequest {
        namespace: namespace.clone(),
        workflow_id: workflow_id.to_string(),
        workflow_type: Some(WorkflowType {
            name: workflow_name.to_string(),
        }),
        task_queue: Some(TaskQueue {
            name: task_queue.to_string(),
            kind: TaskQueueKind::Unspecified as i32,
            normal_name: String::new(),
        }),
        input: Some(Payloads {
            payloads: vec![workflow_payload],
        }),
        workflow_execution_timeout: execution_timeout.and_then(|d| d.try_into().ok()),
        workflow_run_timeout: run_timeout.and_then(|d| d.try_into().ok()),
        workflow_task_timeout: task_timeout.and_then(|d| d.try_into().ok()),
        workflow_id_reuse_policy: id_reuse,
        // Update-with-start needs a non-default conflict policy server-side;
        // UseExisting is the documented choice (start if absent, attach if
        // present).
        workflow_id_conflict_policy: WorkflowIdConflictPolicy::UseExisting as i32,
        request_id: uuid::Uuid::new_v4().to_string(),
        identity: identity.clone(),
        ..Default::default()
    };

    let update_request = UpdateWorkflowExecutionRequest {
        namespace: namespace.clone(),
        workflow_execution: Some(WorkflowExecution {
            workflow_id: workflow_id.to_string(),
            run_id: String::new(),
        }),
        wait_policy: Some(ProtoWaitPolicy {
            lifecycle_stage: sdk_enums::UpdateWorkflowExecutionLifecycleStage::from(wait_policy)
                as i32,
        }),
        request: Some(update::Request {
            meta: Some(update::Meta {
                update_id: uuid::Uuid::new_v4().to_string(),
                identity: identity.clone(),
            }),
            input: Some(update::Input {
                header: None,
                name: update_name.to_string(),
                args: Some(Payloads {
                    payloads: vec![update_payload],
                }),
            }),
        }),
        ..Default::default()
    };

    let req = ExecuteMultiOperationRequest {
        namespace: namespace.clone(),
        operations: vec![
            Operation {
                operation: Some(OperationKind::StartWorkflow(start)),
            },
            Operation {
                operation: Some(OperationKind::UpdateWorkflow(update_request)),
            },
        ],
        resource_id: workflow_id.to_string(),
    };

    let response =
        WorkflowService::execute_multi_operation(&mut sdk_client.clone(), req.into_request())
            .await
            .with_context(|| format!("update-with-start workflow {workflow_name}"))?
            .into_inner();

    let start_resp = response
        .responses
        .first()
        .and_then(|r| r.response.as_ref())
        .context("execute_multi_operation: missing start response")?;
    let update_resp = response
        .responses
        .get(1)
        .and_then(|r| r.response.as_ref())
        .context("execute_multi_operation: missing update response")?;

    let run_id = match start_resp {
        RespKind::StartWorkflow(r) => r.run_id.clone(),
        RespKind::UpdateWorkflow(_) => {
            anyhow::bail!("execute_multi_operation: response[0] was not StartWorkflow")
        }
    };
    let update_payloads = match update_resp {
        RespKind::UpdateWorkflow(r) => r
            .outcome
            .as_ref()
            .and_then(|o| match &o.value {
                Some(update::outcome::Value::Success(s)) => Some(s.payloads.clone()),
                _ => None,
            })
            .context("execute_multi_operation: update outcome had no success payloads")?,
        RespKind::StartWorkflow(_) => {
            anyhow::bail!("execute_multi_operation: response[1] was not UpdateWorkflow")
        }
    };

    let update_payload = update_payloads
        .first()
        .context("update returned no payloads")?;
    let output: O = decode_proto_payload(update_payload).context("decode update output")?;

    Ok((
        WorkflowHandle {
            client: client.clone(),
            workflow_id: workflow_id.to_string(),
            run_id: if run_id.is_empty() {
                None
            } else {
                Some(run_id)
            },
        },
        output,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Eq, prost::Message)]
    struct Sample {
        #[prost(string, tag = "1")]
        name: String,
    }

    impl TemporalProtoMessage for Sample {
        const MESSAGE_TYPE: &'static str = "test.v1.Sample";
    }

    #[test]
    fn encode_decode_round_trip() {
        let original = Sample {
            name: "hello".into(),
        };
        let payload = encode_proto_payload(&original);
        assert_eq!(
            payload.metadata.get("encoding").map(Vec::as_slice),
            Some(b"binary/protobuf".as_slice()),
        );
        assert_eq!(
            payload.metadata.get("messageType").map(Vec::as_slice),
            Some(b"test.v1.Sample".as_slice()),
        );
        let decoded: Sample = decode_proto_payload(&payload).expect("decode");
        assert_eq!(decoded, original);
    }

    #[test]
    #[allow(deprecated)] // mirrors the `From` impl — same rationale.
    fn workflow_id_reuse_policy_maps_to_sdk_variants() {
        assert_eq!(
            sdk_enums::WorkflowIdReusePolicy::from(WorkflowIdReusePolicy::AllowDuplicate),
            sdk_enums::WorkflowIdReusePolicy::AllowDuplicate,
        );
        assert_eq!(
            sdk_enums::WorkflowIdReusePolicy::from(WorkflowIdReusePolicy::AllowDuplicateFailedOnly),
            sdk_enums::WorkflowIdReusePolicy::AllowDuplicateFailedOnly,
        );
        assert_eq!(
            sdk_enums::WorkflowIdReusePolicy::from(WorkflowIdReusePolicy::RejectDuplicate),
            sdk_enums::WorkflowIdReusePolicy::RejectDuplicate,
        );
        assert_eq!(
            sdk_enums::WorkflowIdReusePolicy::from(WorkflowIdReusePolicy::TerminateIfRunning),
            sdk_enums::WorkflowIdReusePolicy::TerminateIfRunning,
        );
    }

    #[test]
    fn wait_policy_maps_to_sdk_lifecycle_stages() {
        use sdk_enums::UpdateWorkflowExecutionLifecycleStage as Stage;
        assert_eq!(Stage::from(WaitPolicy::Admitted), Stage::Admitted);
        assert_eq!(Stage::from(WaitPolicy::Accepted), Stage::Accepted);
        assert_eq!(Stage::from(WaitPolicy::Completed), Stage::Completed);
    }

    #[test]
    fn random_workflow_id_produces_distinct_uuids() {
        let a = random_workflow_id();
        let b = random_workflow_id();
        assert_ne!(a, b);
        // UUID v4 canonical length = 36 (8-4-4-4-12 hex with hyphens).
        assert_eq!(a.len(), 36);
        let chars: Vec<char> = a.chars().collect();
        for &i in &[8usize, 13, 18, 23] {
            assert_eq!(chars[i], '-', "expected hyphen at position {i} in {a}");
        }
    }

    #[test]
    fn wait_stage_from_maps_to_sdk_stage() {
        use temporalio_client::WorkflowUpdateWaitStage as Stage;
        assert!(matches!(
            wait_stage_from(WaitPolicy::Admitted),
            Stage::Admitted
        ));
        assert!(matches!(
            wait_stage_from(WaitPolicy::Accepted),
            Stage::Accepted
        ));
        assert!(matches!(
            wait_stage_from(WaitPolicy::Completed),
            Stage::Completed
        ));
    }
}
