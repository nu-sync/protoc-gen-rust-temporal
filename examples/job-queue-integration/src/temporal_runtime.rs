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
    // scheme you prefer). The PoC uses uuid v4.
    todo!("hook up uuid::Uuid::new_v4()")
}

pub fn eval_id_expression(_expr: &str) -> String {
    // Cludden's id expressions are Go templates over the workflow input.
    // The Rust plugin emits the raw template string; consumer-side
    // expansion is up to you. Simplest path: treat the expression as a
    // literal id and substitute `{{ .Name }}` / similar tokens against
    // your input. The PoC implements a 30-LOC mini-renderer.
    todo!("evaluate Go template against typed input")
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
