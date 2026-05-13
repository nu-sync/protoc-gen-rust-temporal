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
    WorkflowCancelOptions, WorkflowGetResultOptions, WorkflowQueryOptions, WorkflowSignalOptions,
    WorkflowStartOptions, WorkflowStartSignal, WorkflowStartUpdateOptions, WorkflowTerminateOptions,
    WorkflowUpdateWaitStage,
};
use temporalio_common::UntypedWorkflow;
use temporalio_common::data_converters::RawValue;
use temporalio_common::protos::temporal::api::common::v1::{
    Payload, Payloads, WorkflowExecution, WorkflowType,
};
use temporalio_common::protos::temporal::api::enums::v1 as sdk_enums;
use temporalio_common::protos::temporal::api::enums::v1::{
    TaskQueueKind, WorkflowIdConflictPolicy as ProtoWorkflowIdConflictPolicy,
};
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

/// `google.protobuf.Empty` marker. The plugin spells out
/// `temporal_runtime::ProtoEmpty` for the `Input` / `Output` associated
/// types on per-rpc `ActivityDefinition` / `WorkflowDefinition` impls
/// whenever the proto declares `google.protobuf.Empty` on either side.
/// Consumers normally don't construct it directly — generated helpers
/// hide it behind unit-typed signatures.
pub use temporal_proto_runtime::ProtoEmpty;
pub use temporal_proto_runtime::TemporalProtoMessage;
/// Re-export the `binary/protobuf` payload wrapper so plugin-emitted
/// activity marker structs can spell out `temporal_runtime::TypedProtoMessage<T>`
/// for `ActivityDefinition::{Input, Output}`. The `sdk` feature on
/// `temporal-proto-runtime` ships the `TemporalSerializable` /
/// `TemporalDeserializable` impls on `TypedProtoMessage<T>` — the orphan
/// rule blocks impls on raw `T` directly, so the wrapper is the bridge.
pub use temporal_proto_runtime::TypedProtoMessage;
/// Re-export the `temporalio-common` `Payload` type so plugin-emitted
/// code can spell out `temporal_runtime::Payload` for the
/// `search_attributes` map value type. The bridge already calls into
/// this type from `encode_search_attribute_*`; surfacing it at the
/// top-level keeps the generated start-path emit single-named.
pub use temporalio_common::protos::temporal::api::common::v1::Payload as ProtoPayload;

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

/// Policy for a start request whose `workflow_id` matches a **running**
/// workflow. Maps to `temporalio-common`'s `WorkflowIdConflictPolicy`.
/// `Unspecified` lets the server fall through to its default; we model that
/// as `Option::None` at call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowIdConflictPolicy {
    Fail,
    UseExisting,
    TerminateExisting,
}

/// Policy a child workflow follows when its parent workflow closes.
/// Mirrors `(temporal.v1.workflow).parent_close_policy`. `Unspecified`
/// is modelled as `Option::None` at call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentClosePolicy {
    Terminate,
    Abandon,
    RequestCancel,
}

impl From<ParentClosePolicy> for sdk_enums::ParentClosePolicy {
    fn from(value: ParentClosePolicy) -> Self {
        match value {
            ParentClosePolicy::Terminate => Self::Terminate,
            ParentClosePolicy::Abandon => Self::Abandon,
            ParentClosePolicy::RequestCancel => Self::RequestCancel,
        }
    }
}

impl From<WorkflowIdConflictPolicy> for sdk_enums::WorkflowIdConflictPolicy {
    fn from(value: WorkflowIdConflictPolicy) -> Self {
        match value {
            WorkflowIdConflictPolicy::Fail => Self::Fail,
            WorkflowIdConflictPolicy::UseExisting => Self::UseExisting,
            WorkflowIdConflictPolicy::TerminateExisting => Self::TerminateExisting,
        }
    }
}

/// Retry policy for a workflow start request. Mirrors cludden's
/// `RetryPolicy`; maps to `temporalio_common::protos::temporal::api::common::v1::RetryPolicy`
/// (the API uses `maximum_*` naming while cludden's schema uses `max_*` —
/// we follow cludden's, matching the proto annotation users actually write).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RetryPolicy {
    pub initial_interval: Option<Duration>,
    /// Stored as the underlying bits so `Eq` works; access via
    /// [`Self::backoff_coefficient`].
    backoff_coefficient_bits: u64,
    pub max_interval: Option<Duration>,
    pub max_attempts: i32,
    pub non_retryable_error_types: Vec<String>,
}

impl RetryPolicy {
    /// Empty policy — server picks defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// The exponential backoff multiplier. `0.0` means "unset".
    pub fn backoff_coefficient(&self) -> f64 {
        f64::from_bits(self.backoff_coefficient_bits)
    }

    /// Set the exponential backoff multiplier.
    pub fn set_backoff_coefficient(&mut self, value: f64) {
        self.backoff_coefficient_bits = value.to_bits();
    }

    /// Builder-style setter for the backoff coefficient.
    #[must_use]
    pub fn with_backoff_coefficient(mut self, value: f64) -> Self {
        self.set_backoff_coefficient(value);
        self
    }
}

impl From<RetryPolicy> for temporalio_common::protos::temporal::api::common::v1::RetryPolicy {
    fn from(value: RetryPolicy) -> Self {
        Self {
            initial_interval: value.initial_interval.map(duration_to_proto),
            backoff_coefficient: value.backoff_coefficient(),
            maximum_interval: value.max_interval.map(duration_to_proto),
            maximum_attempts: value.max_attempts,
            non_retryable_error_types: value.non_retryable_error_types,
        }
    }
}

fn duration_to_proto(d: Duration) -> prost_wkt_types::Duration {
    let seconds = i64::try_from(d.as_secs()).unwrap_or(i64::MAX);
    let nanos = i32::try_from(d.subsec_nanos()).unwrap_or(0);
    prost_wkt_types::Duration { seconds, nanos }
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

/// Build a single `binary/protobuf` payload from a prost message. The
/// metadata triple matches `WIRE-FORMAT.md`: `encoding = "binary/protobuf"`,
/// `messageType = T::MESSAGE_TYPE`, raw prost bytes in `data`. Public so
/// downstream tooling (custom dispatch layers, payload migrators, etc.) can
/// construct the same byte-identical payloads the generated client emits.
pub fn encode_proto_payload<T: TemporalProtoMessage>(msg: &T) -> Payload {
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

/// Decode a single `binary/protobuf` payload back into a prost message,
/// enforcing the `WIRE-FORMAT.md` triple. Generated client code calls this
/// directly — it does NOT go through the SDK's `TemporalDeserializable`
/// path (which would validate metadata for us), because the SDK returns
/// raw `Payloads` for the workflow/query/update result helpers we use here.
/// Skipping the check would let a misconfigured worker hand back arbitrary
/// bytes that decode as garbage instead of failing loudly. Public so
/// downstream tooling building payload routers outside the generated
/// client can validate + decode against the same contract.
pub fn decode_proto_payload<T: TemporalProtoMessage>(payload: &Payload) -> Result<T> {
    let encoding = payload.metadata.get("encoding").map(Vec::as_slice);
    if encoding != Some(ENCODING.as_bytes()) {
        anyhow::bail!(
            "payload encoding mismatch: expected {ENCODING:?}, got {:?}",
            encoding.map(String::from_utf8_lossy),
        );
    }
    let msg_type = payload.metadata.get("messageType").map(Vec::as_slice);
    if msg_type != Some(T::MESSAGE_TYPE.as_bytes()) {
        anyhow::bail!(
            "payload messageType mismatch: expected {:?}, got {:?}",
            T::MESSAGE_TYPE,
            msg_type.map(String::from_utf8_lossy),
        );
    }
    T::decode(payload.data.as_slice()).context("decode payload bytes")
}

/// Build the `(binary/protobuf, google.protobuf.Empty, data=[])` payload
/// triple that `WIRE-FORMAT.md` mandates for every `google.protobuf.Empty`
/// input.
///
/// **Do NOT replace this with `RawValue::new(vec![])`.** Sending a
/// payload-less `RawValue` looks like "no input" on the wire, which the
/// Go SDK's `ProtoPayloadConverter` does not produce for an `Empty`
/// message — cludden's Go workers and clients always emit the Empty
/// triple even when the message has no fields. Mixed-language workflows
/// would silently fail to encode/decode otherwise.
const EMPTY_MESSAGE_TYPE: &str = "google.protobuf.Empty";
fn encode_empty_payload() -> Payload {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("encoding".to_string(), ENCODING.as_bytes().to_vec());
    metadata.insert(
        "messageType".to_string(),
        EMPTY_MESSAGE_TYPE.as_bytes().to_vec(),
    );
    Payload {
        metadata,
        // google.protobuf.Empty has no fields — wire-bytes are empty by
        // construction, NOT because the payload itself is missing.
        data: vec![],
        external_payloads: vec![],
    }
}

/// R7 slice-2 building block — encode a string value into a Temporal
/// search-attribute `Payload`. Temporal stores search attributes
/// JSON-encoded with `encoding = "json/plain"`; for keyword/text
/// attributes the value lands as a single JSON string.
///
/// The plugin doesn't call this today (slice 1 only models the empty
/// map case). R7 slice 2 will route generated `search_attributes`
/// literal entries through this helper when the proto declares a
/// static map like `root = { "Environment": "production" }`.
pub fn encode_search_attribute_string(value: &str) -> Payload {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("encoding".to_string(), b"json/plain".to_vec());
    // Minimal JSON-escape: backslash + double-quote. Search attribute
    // values in cludden's examples are simple identifiers; the broader
    // JSON-escape set lands when slice 2 implementation needs it.
    let escaped: String = value
        .chars()
        .flat_map(|c| match c {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            c => vec![c],
        })
        .collect();
    let data = format!("\"{escaped}\"").into_bytes();
    Payload {
        metadata,
        data,
        external_payloads: vec![],
    }
}

/// R7 slice-2 building block — encode a 64-bit signed integer search
/// attribute. Same `json/plain` encoding as
/// [`encode_search_attribute_string`]; values land as a JSON number.
pub fn encode_search_attribute_int(value: i64) -> Payload {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("encoding".to_string(), b"json/plain".to_vec());
    Payload {
        metadata,
        data: value.to_string().into_bytes(),
        external_payloads: vec![],
    }
}

/// Encode an IEEE-754 double search attribute. Mirrors the shape of
/// the int / string / bool encoders. `json/plain`-encoded JSON number.
/// NaN and infinities are rejected at the encoder boundary because
/// neither value has a valid JSON literal form — silent serialisation
/// to non-JSON tokens would round-trip differently across languages.
pub fn encode_search_attribute_double(value: f64) -> Result<Payload> {
    if !value.is_finite() {
        anyhow::bail!(
            "encode_search_attribute_double: value {value} is not finite (NaN / infinity has no JSON literal)",
        );
    }
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("encoding".to_string(), b"json/plain".to_vec());
    // `{:?}` preserves the decimal point on whole-number doubles
    // (`1.0`, not `1`) so the wire shape stays unambiguously a JSON
    // number rather than slipping toward int-looking output.
    Ok(Payload {
        metadata,
        data: format!("{value:?}").into_bytes(),
        external_payloads: vec![],
    })
}

/// Decode a `double` search-attribute `Payload` written by
/// [`encode_search_attribute_double`].
pub fn decode_search_attribute_double(payload: &Payload) -> Result<f64> {
    let data = check_json_plain(payload, "double")?;
    let s = std::str::from_utf8(data)
        .context("decode_search_attribute_double: data is not valid UTF-8")?;
    let v: f64 = s
        .parse()
        .with_context(|| format!("decode_search_attribute_double: not an f64 literal: {s:?}"))?;
    if !v.is_finite() {
        anyhow::bail!(
            "decode_search_attribute_double: decoded value {v} is not finite — payload may be corrupt",
        );
    }
    Ok(v)
}

/// R7 slice-2 building block — encode a boolean search attribute.
pub fn encode_search_attribute_bool(value: bool) -> Payload {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("encoding".to_string(), b"json/plain".to_vec());
    Payload {
        metadata,
        data: (if value { "true" } else { "false" }).as_bytes().to_vec(),
        external_payloads: vec![],
    }
}

/// Validate that a search-attribute `Payload` carries the canonical
/// `json/plain` encoding the bridge emits via
/// [`encode_search_attribute_string`] / `_int` / `_bool`. Returns the
/// raw JSON bytes ready for type-specific parsing. Server-supplied
/// payloads under a different encoding (e.g. `binary/protobuf`)
/// surface a precise diagnostic so consumers don't silently mis-decode.
fn check_json_plain<'a>(payload: &'a Payload, what: &str) -> Result<&'a [u8]> {
    let encoding = payload
        .metadata
        .get("encoding")
        .map(|v| v.as_slice())
        .unwrap_or(b"");
    if encoding != b"json/plain" {
        anyhow::bail!(
            "decode_search_attribute_{what}: expected `json/plain` encoding, got {encoding:?}",
        );
    }
    Ok(&payload.data)
}

/// Decode a `string` search-attribute `Payload` written by
/// [`encode_search_attribute_string`]. Accepts the minimal JSON-escape
/// the encoder produces (backslash + double-quote).
pub fn decode_search_attribute_string(payload: &Payload) -> Result<String> {
    let data = check_json_plain(payload, "string")?;
    let s = std::str::from_utf8(data)
        .context("decode_search_attribute_string: data is not valid UTF-8")?;
    let s = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(|| anyhow::anyhow!("decode_search_attribute_string: missing JSON quotes"))?;
    // Inverse of the minimal JSON-escape the encoder applies.
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some(other) => {
                    anyhow::bail!("decode_search_attribute_string: unsupported escape `\\{other}`")
                }
                None => anyhow::bail!("decode_search_attribute_string: trailing backslash"),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

/// Decode a signed-integer search-attribute `Payload` written by
/// [`encode_search_attribute_int`].
pub fn decode_search_attribute_int(payload: &Payload) -> Result<i64> {
    let data = check_json_plain(payload, "int")?;
    let s = std::str::from_utf8(data)
        .context("decode_search_attribute_int: data is not valid UTF-8")?;
    s.parse::<i64>()
        .with_context(|| format!("decode_search_attribute_int: not a base-10 i64: {s:?}"))
}

/// Decode a boolean search-attribute `Payload` written by
/// [`encode_search_attribute_bool`].
pub fn decode_search_attribute_bool(payload: &Payload) -> Result<bool> {
    let data = check_json_plain(payload, "bool")?;
    match data {
        b"true" => Ok(true),
        b"false" => Ok(false),
        other => {
            let s = std::str::from_utf8(other).unwrap_or("<non-utf8>");
            anyhow::bail!("decode_search_attribute_bool: expected `true`/`false`, got {s:?}")
        }
    }
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
    let client = Client::new(
        connection,
        ClientOptions::new(namespace.to_string()).build(),
    )
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
    id_conflict_policy: Option<WorkflowIdConflictPolicy>,
    execution_timeout: Option<Duration>,
    run_timeout: Option<Duration>,
    task_timeout: Option<Duration>,
    enable_eager_workflow_start: bool,
    retry_policy: Option<RetryPolicy>,
    search_attributes: Option<std::collections::HashMap<String, Payload>>,
) -> Result<WorkflowHandle>
where
    I: TemporalProtoMessage,
{
    let payload = encode_proto_payload(input);
    let raw = RawValue::new(vec![payload]);
    let base = WorkflowStartOptions::new(task_queue.to_string(), workflow_id.to_string())
        .maybe_execution_timeout(execution_timeout)
        .maybe_run_timeout(run_timeout)
        .maybe_task_timeout(task_timeout)
        .maybe_retry_policy(retry_policy.map(Into::into))
        .maybe_search_attributes(search_attributes)
        .enable_eager_workflow_start(enable_eager_workflow_start);
    // bon builders use typestate — every conditional setter has its own
    // `Set*` marker, so the call chain must terminate in a single
    // `build()` per arm. Materialise the option matrix here.
    let options = match (id_reuse_policy, id_conflict_policy) {
        (Some(reuse), Some(conflict)) => base
            .id_reuse_policy(reuse.into())
            .id_conflict_policy(conflict.into())
            .build(),
        (Some(reuse), None) => base.id_reuse_policy(reuse.into()).build(),
        (None, Some(conflict)) => base.id_conflict_policy(conflict.into()).build(),
        (None, None) => base.build(),
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
    id_conflict_policy: Option<WorkflowIdConflictPolicy>,
    execution_timeout: Option<Duration>,
    run_timeout: Option<Duration>,
    task_timeout: Option<Duration>,
    enable_eager_workflow_start: bool,
    retry_policy: Option<RetryPolicy>,
    search_attributes: Option<std::collections::HashMap<String, Payload>>,
) -> Result<WorkflowHandle> {
    let raw = RawValue::new(vec![encode_empty_payload()]);
    let base = WorkflowStartOptions::new(task_queue.to_string(), workflow_id.to_string())
        .maybe_execution_timeout(execution_timeout)
        .maybe_run_timeout(run_timeout)
        .maybe_task_timeout(task_timeout)
        .maybe_retry_policy(retry_policy.map(Into::into))
        .maybe_search_attributes(search_attributes)
        .enable_eager_workflow_start(enable_eager_workflow_start);
    let options = match (id_reuse_policy, id_conflict_policy) {
        (Some(reuse), Some(conflict)) => base
            .id_reuse_policy(reuse.into())
            .id_conflict_policy(conflict.into())
            .build(),
        (Some(reuse), None) => base.id_reuse_policy(reuse.into()).build(),
        (None, Some(conflict)) => base.id_conflict_policy(conflict.into()).build(),
        (None, None) => base.build(),
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
/// Validates the returned payload carries the `(binary/protobuf,
/// google.protobuf.Empty, data=[])` triple — same wire-format invariant as
/// the typed path, applied to the empty case so a worker that returns a
/// non-empty result can't silently round-trip as success.
pub async fn wait_result_unit(handle: &WorkflowHandle) -> Result<()> {
    let raw = handle
        .untyped()
        .get_result(WorkflowGetResultOptions::builder().build())
        .await
        .context("await workflow result")?;
    let payload = raw
        .payloads
        .first()
        .context("workflow returned no payloads")?;
    validate_empty_payload(payload).context("validate workflow output")
}

/// Enforce the `(binary/protobuf, google.protobuf.Empty, data=[])` triple
/// that `WIRE-FORMAT.md` mandates for any `google.protobuf.Empty` payload.
fn validate_empty_payload(payload: &Payload) -> Result<()> {
    let encoding = payload.metadata.get("encoding").map(Vec::as_slice);
    if encoding != Some(ENCODING.as_bytes()) {
        anyhow::bail!(
            "empty payload encoding mismatch: expected {ENCODING:?}, got {:?}",
            encoding.map(String::from_utf8_lossy),
        );
    }
    let msg_type = payload.metadata.get("messageType").map(Vec::as_slice);
    if msg_type != Some(EMPTY_MESSAGE_TYPE.as_bytes()) {
        anyhow::bail!(
            "empty payload messageType mismatch: expected {EMPTY_MESSAGE_TYPE:?}, got {:?}",
            msg_type.map(String::from_utf8_lossy),
        );
    }
    if !payload.data.is_empty() {
        anyhow::bail!(
            "empty payload carried {} byte(s) of data — google.protobuf.Empty has no fields",
            payload.data.len(),
        );
    }
    Ok(())
}

// ── Signals ────────────────────────────────────────────────────────────

/// Request cancellation of a running workflow. The server records the
/// request and routes it to the workflow's cancel handler; the workflow's
/// own logic decides what to do. `reason` is recorded in event history.
pub async fn cancel_workflow(handle: &WorkflowHandle, reason: &str) -> Result<()> {
    let opts = WorkflowCancelOptions::builder()
        .reason(reason.to_string())
        .build();
    handle
        .untyped()
        .cancel(opts)
        .await
        .with_context(|| format!("cancel workflow {}", handle.workflow_id))?;
    Ok(())
}

/// Terminate a running workflow. Unlike [`cancel_workflow`], this is a
/// hard kill — the workflow's cancel handler does not run and history is
/// finalized with a `WorkflowExecutionTerminated` event. `reason` is
/// recorded; the server picks a UUID request id.
pub async fn terminate_workflow(handle: &WorkflowHandle, reason: &str) -> Result<()> {
    let opts = WorkflowTerminateOptions::builder()
        .reason(reason.to_string())
        .build();
    handle
        .untyped()
        .terminate(opts)
        .await
        .with_context(|| format!("terminate workflow {}", handle.workflow_id))?;
    Ok(())
}

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
    let raw = RawValue::new(vec![encode_empty_payload()]);
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
    let raw_input = RawValue::new(vec![encode_empty_payload()]);
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

/// Run a query whose output is `google.protobuf.Empty`. Mirrors
/// [`wait_result_unit`] — the response payload must be the canonical Empty
/// triple, so a non-empty result can't silently round-trip as success.
pub async fn query_unit<I>(handle: &WorkflowHandle, name: &str, input: &I) -> Result<()>
where
    I: TemporalProtoMessage,
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
    validate_empty_payload(payload).context("validate query output")
}

/// Run a query whose input and output are both `google.protobuf.Empty`.
pub async fn query_proto_empty_unit(handle: &WorkflowHandle, name: &str) -> Result<()> {
    let raw_input = RawValue::new(vec![encode_empty_payload()]);
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
    validate_empty_payload(payload).context("validate query output")
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
    let raw_input = RawValue::new(vec![encode_empty_payload()]);
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

/// Send an update whose output is `google.protobuf.Empty`. Mirrors
/// [`wait_result_unit`].
pub async fn update_unit<I>(
    handle: &WorkflowHandle,
    name: &str,
    input: &I,
    wait_policy: WaitPolicy,
) -> Result<()>
where
    I: TemporalProtoMessage,
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
    validate_empty_payload(payload).context("validate update output")
}

/// Send an update whose input and output are both `google.protobuf.Empty`.
pub async fn update_proto_empty_unit(
    handle: &WorkflowHandle,
    name: &str,
    wait_policy: WaitPolicy,
) -> Result<()> {
    let raw_input = RawValue::new(vec![encode_empty_payload()]);
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
    validate_empty_payload(payload).context("validate update output")
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
    id_conflict_policy: Option<WorkflowIdConflictPolicy>,
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
        // honour the proto-declared override when present, otherwise
        // fall back to UseExisting (start if absent, attach if present).
        workflow_id_conflict_policy: id_conflict_policy
            .map(ProtoWorkflowIdConflictPolicy::from)
            .unwrap_or(ProtoWorkflowIdConflictPolicy::UseExisting)
            as i32,
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

/// Sibling of [`update_with_start_workflow_proto`] for updates whose output
/// is `google.protobuf.Empty`. The plugin routes here when the update rpc's
/// return type is Empty, since `()` does not implement [`TemporalProtoMessage`]
/// and cannot be substituted for the `O` generic on the typed variant.
#[allow(clippy::too_many_arguments)]
pub async fn update_with_start_workflow_proto_unit<W, U>(
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
    id_conflict_policy: Option<WorkflowIdConflictPolicy>,
) -> Result<WorkflowHandle>
where
    W: TemporalProtoMessage,
    U: TemporalProtoMessage,
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
        workflow_id_conflict_policy: id_conflict_policy
            .map(ProtoWorkflowIdConflictPolicy::from)
            .unwrap_or(ProtoWorkflowIdConflictPolicy::UseExisting)
            as i32,
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
    validate_empty_payload(update_payload).context("validate update output")?;

    Ok(WorkflowHandle {
        client: client.clone(),
        workflow_id: workflow_id.to_string(),
        run_id: if run_id.is_empty() {
            None
        } else {
            Some(run_id)
        },
    })
}

// ── Worker primitives (feature = "worker") ─────────────────────────────

/// Re-exports of the SDK worker primitives used by consumers wiring the
/// plugin-generated `<Service>Activities` trait to a Temporal worker.
///
/// **Stability:** these are direct re-exports of `temporalio-sdk 0.4` types.
/// When the SDK reshapes between minor versions, the bridge crate's minor
/// version bumps with it (per the design's SDK pinning rule). Consumer code
/// that touches these types may need adjustment at SDK upgrade time; the
/// plugin's emit is unaffected.
#[cfg(feature = "worker")]
pub mod worker {
    pub use super::ParentClosePolicy;
    pub use temporalio_common::protos::coresdk::child_workflow::ChildWorkflowCancellationType;
    pub use temporalio_common::protos::coresdk::workflow_commands::ActivityCancellationType;
    pub use temporalio_common::{ActivityDefinition, SignalDefinition, WorkflowDefinition};
    pub use temporalio_sdk::activities::{
        ActivityContext, ActivityDefinitions, ActivityError, ActivityImplementer,
    };
    pub use temporalio_sdk::workflows::{WorkflowImplementation, WorkflowImplementer};
    pub use temporalio_sdk::{
        ActivityCloseTimeouts, ActivityExecutionError, ActivityOptions, ChildWorkflowOptions,
        ChildWorkflowStartError, ContinueAsNewOptions, ExternalWorkflowHandle,
        LocalActivityOptions, StartedChildWorkflow, WorkflowContext,
    };
    pub use temporalio_sdk::{
        SignalExternalOk, SignalExternalWfResult, Worker, WorkflowTermination,
    };
}

/// Top-level re-export so plugin-emitted code can resolve
/// `crate::temporal_runtime::ActivityContext` without thinking about the
/// worker submodule. Required by Phase 2 `activities=true` emit.
#[cfg(feature = "worker")]
pub use worker::ActivityContext;

/// Re-export `clap` (with the `derive` feature) so plugin-emitted CLI code
/// can resolve `temporal_runtime::clap::Parser` / `Subcommand` / `Args`
/// without the consumer adding a direct clap dep. Phase 4.0 emit references
/// this path.
#[cfg(feature = "cli")]
pub use clap;

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
    fn retry_policy_converts_to_sdk_shape() {
        let mut p = RetryPolicy::new();
        p.initial_interval = Some(Duration::from_secs(1));
        p.max_interval = Some(Duration::from_secs(60));
        p.max_attempts = 5;
        p.non_retryable_error_types = vec!["X".to_string(), "Y".to_string()];
        p.set_backoff_coefficient(2.0);

        let sdk: temporalio_common::protos::temporal::api::common::v1::RetryPolicy = p.into();
        assert_eq!(sdk.maximum_attempts, 5);
        assert!((sdk.backoff_coefficient - 2.0).abs() < f64::EPSILON);
        assert_eq!(
            sdk.initial_interval,
            Some(prost_wkt_types::Duration {
                seconds: 1,
                nanos: 0
            })
        );
        assert_eq!(
            sdk.maximum_interval,
            Some(prost_wkt_types::Duration {
                seconds: 60,
                nanos: 0
            })
        );
        assert_eq!(sdk.non_retryable_error_types, vec!["X", "Y"]);
    }

    // Regression guard for the wire-format contract: every empty-input
    // bridge helper MUST emit the `(binary/protobuf, google.protobuf.Empty,
    // data=[])` payload triple — *not* an absent payload. A previous
    // implementation passed `RawValue::new(vec![])` and silently dropped
    // the metadata, which broke mixed-language interop with cludden's Go
    // workers (they always emit the Empty triple).
    #[test]
    fn search_attribute_string_encodes_to_json_string() {
        let p = encode_search_attribute_string("production");
        assert_eq!(
            p.metadata.get("encoding").map(Vec::as_slice),
            Some(b"json/plain".as_slice()),
        );
        assert_eq!(p.data, br#""production""#.to_vec());
    }

    #[test]
    fn search_attribute_string_escapes_quotes_and_backslashes() {
        let p = encode_search_attribute_string(r#"with"quote\and\backslash"#);
        // The escaped JSON form: backslashes and quotes get escaped.
        assert_eq!(
            std::str::from_utf8(&p.data).unwrap(),
            r#""with\"quote\\and\\backslash""#
        );
    }

    #[test]
    fn search_attribute_int_encodes_as_json_number() {
        let p = encode_search_attribute_int(42);
        assert_eq!(p.data, b"42".to_vec());
        let neg = encode_search_attribute_int(-7);
        assert_eq!(neg.data, b"-7".to_vec());
    }

    #[test]
    fn search_attribute_bool_encodes_as_json_bool() {
        assert_eq!(encode_search_attribute_bool(true).data, b"true".to_vec());
        assert_eq!(encode_search_attribute_bool(false).data, b"false".to_vec());
    }

    #[test]
    fn search_attribute_string_decode_roundtrips_through_encode() {
        // Round-trip every escape branch the minimal encoder produces.
        for original in [
            "production",
            "with\"quote",
            "with\\backslash",
            r#"with"quote\and\backslash"#,
            "plain",
            "",
        ] {
            let p = encode_search_attribute_string(original);
            let decoded = decode_search_attribute_string(&p).expect("decode must succeed");
            assert_eq!(decoded, original, "round-trip drift on {original:?}");
        }
    }

    #[test]
    fn search_attribute_int_decode_roundtrips_through_encode() {
        for original in [0i64, 1, -1, 42, -7, i64::MAX, i64::MIN] {
            let p = encode_search_attribute_int(original);
            let decoded = decode_search_attribute_int(&p).expect("decode must succeed");
            assert_eq!(decoded, original);
        }
    }

    #[test]
    fn search_attribute_double_roundtrips_through_encode() {
        for original in [
            0.0_f64,
            1.0,
            -1.0,
            1.5,
            -0.25,
            std::f64::consts::PI,
            1e10,
            1e-10,
        ] {
            let p = encode_search_attribute_double(original).expect("finite double must encode");
            let decoded =
                decode_search_attribute_double(&p).expect("round-trip decode must succeed");
            assert_eq!(decoded, original, "round-trip drift on {original:?}");
        }
        // Whole-number doubles must keep the decimal point so the
        // wire shape stays JSON-number, not JSON-int.
        let p = encode_search_attribute_double(1.0).expect("finite double must encode");
        assert_eq!(p.data, b"1.0".to_vec());
    }

    #[test]
    fn search_attribute_double_rejects_nan_and_infinity() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = encode_search_attribute_double(bad).unwrap_err().to_string();
            assert!(
                err.contains("not finite"),
                "encoder must refuse NaN / infinity: {err}"
            );
        }
    }

    #[test]
    fn search_attribute_double_decode_rejects_non_numeric_data() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("encoding".to_string(), b"json/plain".to_vec());
        let bad = Payload {
            metadata,
            data: b"nope".to_vec(),
            external_payloads: vec![],
        };
        let err = decode_search_attribute_double(&bad)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not an f64 literal"), "diagnostic: {err}");
    }

    #[test]
    fn search_attribute_bool_decode_roundtrips_through_encode() {
        assert!(decode_search_attribute_bool(&encode_search_attribute_bool(true)).unwrap());
        assert!(!decode_search_attribute_bool(&encode_search_attribute_bool(false)).unwrap());
    }

    #[test]
    fn search_attribute_decode_rejects_wrong_encoding() {
        // A payload claiming `binary/protobuf` must NOT be decoded as
        // a `json/plain` search attribute — the encoder contract says
        // search attributes are always `json/plain`, and silent
        // mis-decode would surface corrupted values.
        let mut bad = encode_search_attribute_string("x");
        bad.metadata
            .insert("encoding".to_string(), b"binary/protobuf".to_vec());
        let err = decode_search_attribute_string(&bad)
            .unwrap_err()
            .to_string();
        assert!(err.contains("expected `json/plain`"), "diagnostic: {err}");
    }

    #[test]
    fn search_attribute_int_decode_rejects_non_numeric_data() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("encoding".to_string(), b"json/plain".to_vec());
        let bad = Payload {
            metadata,
            data: b"not-a-number".to_vec(),
            external_payloads: vec![],
        };
        let err = decode_search_attribute_int(&bad).unwrap_err().to_string();
        assert!(err.contains("not a base-10 i64"), "diagnostic: {err}");
    }

    #[test]
    fn search_attribute_bool_decode_rejects_other_literals() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("encoding".to_string(), b"json/plain".to_vec());
        let bad = Payload {
            metadata,
            data: b"yes".to_vec(),
            external_payloads: vec![],
        };
        let err = decode_search_attribute_bool(&bad).unwrap_err().to_string();
        assert!(err.contains("expected `true`/`false`"), "diagnostic: {err}");
    }

    #[test]
    fn empty_payload_carries_the_full_triple() {
        let payload = encode_empty_payload();
        assert_eq!(
            payload.metadata.get("encoding").map(Vec::as_slice),
            Some(b"binary/protobuf".as_slice()),
            "encoding metadata must be present"
        );
        assert_eq!(
            payload.metadata.get("messageType").map(Vec::as_slice),
            Some(b"google.protobuf.Empty".as_slice()),
            "messageType must name google.protobuf.Empty"
        );
        assert!(
            payload.data.is_empty(),
            "Empty's serialized wire bytes are zero length by construction"
        );
        assert!(payload.external_payloads.is_empty());
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

    // Regression guards for the bridge's wire-format decode contract. The
    // SDK hands us raw `Payloads` for workflow/query/update results — it
    // does NOT run them through `TemporalDeserializable`, so the metadata
    // check has to live in the bridge or it doesn't run at all.
    #[test]
    fn decode_rejects_wrong_encoding() {
        let mut payload = encode_proto_payload(&Sample { name: "x".into() });
        payload
            .metadata
            .insert("encoding".to_string(), b"json/plain".to_vec());
        let err = decode_proto_payload::<Sample>(&payload)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("encoding mismatch"),
            "expected encoding-mismatch diagnostic, got: {err}"
        );
    }

    #[test]
    fn decode_rejects_wrong_message_type() {
        let mut payload = encode_proto_payload(&Sample { name: "x".into() });
        payload
            .metadata
            .insert("messageType".to_string(), b"other.v1.Wrong".to_vec());
        let err = decode_proto_payload::<Sample>(&payload)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("messageType mismatch"),
            "expected messageType-mismatch diagnostic, got: {err}"
        );
    }

    #[test]
    fn decode_rejects_missing_metadata() {
        let payload = Payload {
            metadata: std::collections::HashMap::new(),
            data: prost::Message::encode_to_vec(&Sample { name: "x".into() }),
            external_payloads: vec![],
        };
        let err = decode_proto_payload::<Sample>(&payload)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("encoding mismatch"),
            "expected diagnostic when metadata is missing, got: {err}"
        );
    }

    // Regression guards for `wait_result_unit`'s payload validation. A
    // worker that returns a non-empty payload from a workflow declared to
    // return Empty must fail the wait, not silently round-trip as success.
    #[test]
    fn validate_empty_accepts_canonical_triple() {
        let payload = encode_empty_payload();
        validate_empty_payload(&payload).expect("canonical Empty triple must validate");
    }

    #[test]
    fn validate_empty_rejects_non_empty_data() {
        let mut payload = encode_empty_payload();
        payload.data = vec![0x01];
        let err = validate_empty_payload(&payload).unwrap_err().to_string();
        assert!(
            err.contains("byte"),
            "expected non-empty-data diagnostic, got: {err}"
        );
    }

    #[test]
    fn validate_empty_rejects_typed_message_type() {
        let mut payload = encode_empty_payload();
        payload
            .metadata
            .insert("messageType".to_string(), b"test.v1.Sample".to_vec());
        let err = validate_empty_payload(&payload).unwrap_err().to_string();
        assert!(
            err.contains("messageType mismatch"),
            "expected messageType-mismatch diagnostic, got: {err}"
        );
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
