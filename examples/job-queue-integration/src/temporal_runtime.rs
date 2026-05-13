//! Consumer-supplied bridge between the plugin-generated client surface
//! and the Temporal Rust SDK. The plugin's generated code references
//! every symbol below via `crate::temporal_runtime::*`.
//!
//! This file is a **stub**: it pins the API the plugin expects but leaves
//! the SDK calls as `todo!()` so the example compiles without a Temporal
//! Server. Replace the bodies with calls into `temporalio-client` /
//! `temporalio-sdk` once you wire up a real consumer.
//!
//! The PoC at
//! `/Users/wcygan/Development/job-queue/crates/jobs-proto/src/temporal_runtime.rs`
//! is the reference implementation.

use std::time::Duration;

use anyhow::Result;
pub use temporal_proto_runtime::{TemporalProtoMessage, TypedProtoMessage};

pub struct ActivityContext;

pub mod worker {
    pub struct Worker;

    pub trait ActivityImplementer {}

    pub trait WorkflowImplementer {}

    impl Worker {
        pub fn register_activities<I: ActivityImplementer>(&mut self, _impl: I) -> &mut Self {
            self
        }

        pub fn register_workflow<W: WorkflowImplementer>(&mut self) -> &mut Self {
            self
        }
    }
}

#[derive(Clone)]
pub struct TemporalClient {
    // e.g. inner: temporalio_client::Client,
}

#[derive(Clone)]
pub struct WorkflowHandle {
    workflow_id: String,
    // e.g. inner: temporalio_client::WorkflowHandle,
}

impl WorkflowHandle {
    pub fn workflow_id(&self) -> &str {
        &self.workflow_id
    }
}

#[derive(Debug, Clone, Copy)]
pub enum WorkflowIdReusePolicy {
    AllowDuplicate,
    AllowDuplicateFailedOnly,
    RejectDuplicate,
    TerminateIfRunning,
}

#[derive(Debug, Clone, Copy)]
pub enum WaitPolicy {
    Admitted,
    Accepted,
    Completed,
}

pub fn attach_handle(_client: &TemporalClient, workflow_id: String) -> WorkflowHandle {
    WorkflowHandle { workflow_id }
}

pub fn random_workflow_id() -> String {
    // Replace with a uuid::Uuid::new_v4().to_string() (or whatever id
    // scheme you prefer). The PoC uses uuid v4. The plugin only calls
    // this when a workflow has no proto-level `id` template; templates
    // are materialised inline as `<wf>_id(...)` functions by the plugin.
    todo!("hook up uuid::Uuid::new_v4()")
}

#[allow(clippy::too_many_arguments)]
pub async fn start_workflow_proto<I: TemporalProtoMessage>(
    _client: &TemporalClient,
    _workflow_name: &'static str,
    _workflow_id: &str,
    _task_queue: &str,
    _input: &I,
    _id_reuse_policy: Option<WorkflowIdReusePolicy>,
    _execution_timeout: Option<Duration>,
    _run_timeout: Option<Duration>,
    _task_timeout: Option<Duration>,
) -> Result<WorkflowHandle> {
    todo!("client.start_workflow_execution(...)")
}

/// Empty-input variant of [`start_workflow_proto`]. The plugin emits a call
/// to this function when a workflow's input is `google.protobuf.Empty`,
/// avoiding the need to express `()` as a `TemporalProtoMessage`.
///
/// **Wire-format contract:** even though there is no `&I` payload arg,
/// the bridge MUST encode an `(encoding="binary/protobuf",
/// messageType="google.protobuf.Empty", data=[])` payload onto the
/// outgoing `StartWorkflow` request. See `docs/RUNTIME-API.md` →
/// "Empty-input contract" and `WIRE-FORMAT.md` for why a payload-less
/// `RawValue` is not equivalent and silently breaks mixed-language
/// (Rust + Go) interop. The default bridge crate
/// (`temporal-proto-runtime-bridge`) gets this right out of the box;
/// hand-rolled bridges must too.
#[allow(clippy::too_many_arguments)]
pub async fn start_workflow_proto_empty(
    _client: &TemporalClient,
    _workflow_name: &'static str,
    _workflow_id: &str,
    _task_queue: &str,
    _id_reuse_policy: Option<WorkflowIdReusePolicy>,
    _execution_timeout: Option<Duration>,
    _run_timeout: Option<Duration>,
    _task_timeout: Option<Duration>,
) -> Result<WorkflowHandle> {
    todo!("client.start_workflow_execution(...) with no payload")
}

pub async fn wait_result_proto<O: TemporalProtoMessage>(_handle: &WorkflowHandle) -> Result<O> {
    todo!("handle.result().await")
}

pub async fn wait_result_unit(_handle: &WorkflowHandle) -> Result<()> {
    todo!("handle.result().await for Empty-output workflows")
}

pub async fn signal_proto<I: TemporalProtoMessage>(
    _handle: &WorkflowHandle,
    _signal_name: &str,
    _input: &I,
) -> Result<()> {
    todo!("handle.signal(name, payload).await")
}

pub async fn signal_unit(_handle: &WorkflowHandle, _signal_name: &str) -> Result<()> {
    todo!("handle.signal(name, Empty).await")
}

pub async fn query_proto<I: TemporalProtoMessage, O: TemporalProtoMessage>(
    _handle: &WorkflowHandle,
    _query_name: &str,
    _input: &I,
) -> Result<O> {
    todo!("handle.query(name, input).await")
}

pub async fn query_proto_empty<O: TemporalProtoMessage>(
    _handle: &WorkflowHandle,
    _query_name: &str,
) -> Result<O> {
    todo!("handle.query(name, Empty).await")
}

pub async fn query_unit<I: TemporalProtoMessage>(
    _handle: &WorkflowHandle,
    _query_name: &str,
    _input: &I,
) -> Result<()> {
    todo!("handle.query(name, input).await — validate Empty response")
}

pub async fn query_proto_empty_unit(_handle: &WorkflowHandle, _query_name: &str) -> Result<()> {
    todo!("handle.query(name, Empty).await — validate Empty response")
}

pub async fn update_proto<I: TemporalProtoMessage, O: TemporalProtoMessage>(
    _handle: &WorkflowHandle,
    _update_name: &str,
    _input: &I,
    _wait_policy: WaitPolicy,
) -> Result<O> {
    todo!("handle.update(name, input, wait_policy).await")
}

pub async fn update_proto_empty<O: TemporalProtoMessage>(
    _handle: &WorkflowHandle,
    _update_name: &str,
    _wait_policy: WaitPolicy,
) -> Result<O> {
    todo!("handle.update(name, Empty, wait_policy).await")
}

pub async fn update_unit<I: TemporalProtoMessage>(
    _handle: &WorkflowHandle,
    _update_name: &str,
    _input: &I,
    _wait_policy: WaitPolicy,
) -> Result<()> {
    todo!("handle.update(name, input, wait_policy).await — validate Empty response")
}

pub async fn update_proto_empty_unit(
    _handle: &WorkflowHandle,
    _update_name: &str,
    _wait_policy: WaitPolicy,
) -> Result<()> {
    todo!("handle.update(name, Empty, wait_policy).await — validate Empty response")
}

#[allow(clippy::too_many_arguments)]
pub async fn signal_with_start_workflow_proto<W: TemporalProtoMessage, S: TemporalProtoMessage>(
    _client: &TemporalClient,
    _workflow_name: &'static str,
    _workflow_id: &str,
    _task_queue: &str,
    _workflow_input: &W,
    _signal_name: &str,
    _signal_input: &S,
    _id_reuse_policy: Option<WorkflowIdReusePolicy>,
    _execution_timeout: Option<Duration>,
    _run_timeout: Option<Duration>,
    _task_timeout: Option<Duration>,
) -> Result<WorkflowHandle> {
    todo!("client.signal_with_start_workflow_execution(...)")
}

#[allow(clippy::too_many_arguments)]
pub async fn update_with_start_workflow_proto<
    W: TemporalProtoMessage,
    U: TemporalProtoMessage,
    O: TemporalProtoMessage,
>(
    _client: &TemporalClient,
    _workflow_name: &'static str,
    _workflow_id: &str,
    _task_queue: &str,
    _workflow_input: &W,
    _update_name: &str,
    _update_input: &U,
    _wait_policy: WaitPolicy,
    _id_reuse_policy: Option<WorkflowIdReusePolicy>,
    _execution_timeout: Option<Duration>,
    _run_timeout: Option<Duration>,
    _task_timeout: Option<Duration>,
) -> Result<(WorkflowHandle, O)> {
    todo!("client.update_with_start_workflow_execution(...)")
}

#[allow(clippy::too_many_arguments)]
pub async fn update_with_start_workflow_proto_unit<
    W: TemporalProtoMessage,
    U: TemporalProtoMessage,
>(
    _client: &TemporalClient,
    _workflow_name: &'static str,
    _workflow_id: &str,
    _task_queue: &str,
    _workflow_input: &W,
    _update_name: &str,
    _update_input: &U,
    _wait_policy: WaitPolicy,
    _id_reuse_policy: Option<WorkflowIdReusePolicy>,
    _execution_timeout: Option<Duration>,
    _run_timeout: Option<Duration>,
    _task_timeout: Option<Duration>,
) -> Result<WorkflowHandle> {
    todo!("client.update_with_start_workflow_execution(...) — validate Empty response")
}
