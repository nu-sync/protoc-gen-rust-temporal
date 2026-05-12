//! Internal representation produced by `parse.rs` and consumed by `render.rs`.
//!
//! The schema deliberately mirrors cludden's `temporal.v1.*` annotation
//! surface (see `proto/temporal/v1/temporal.proto`), only retaining the
//! fields needed for v1 Rust client emit. Anything we read but ignore (XNS,
//! patches, CLI options) lives in the descriptor pool and is silently
//! dropped here.

use std::time::Duration;

/// One Temporal-bearing proto service after parsing + validation.
#[derive(Debug)]
pub struct ServiceModel {
    /// Fully-qualified proto package, e.g. `"jobs.v1"`.
    pub package: String,
    /// Service name from the proto, e.g. `"JobService"`.
    pub service: String,
    /// Source `.proto` file path as `protoc` saw it.
    pub source_file: String,
    /// `temporal.v1.service.task_queue` if the service carries the annotation.
    /// Used as the default `task_queue` when a workflow does not override it.
    pub default_task_queue: Option<String>,
    pub workflows: Vec<WorkflowModel>,
    pub signals: Vec<SignalModel>,
    pub queries: Vec<QueryModel>,
    pub updates: Vec<UpdateModel>,
    pub activities: Vec<ActivityModel>,
}

#[derive(Debug)]
pub struct WorkflowModel {
    /// Rpc method name as declared in proto (e.g. `"RunJob"`).
    pub rpc_method: String,
    /// Cross-language workflow registration name. Defaults to
    /// `"<package>.<Service>.<Rpc>"` (the proto method's fully-qualified
    /// name) when `WorkflowOptions.name` is empty, matching
    /// `cludden/protoc-gen-go-temporal`'s `method.Desc.FullName()` so
    /// Rust + Go workers register against the same Temporal name.
    pub registered_name: String,
    pub input_type: ProtoType,
    pub output_type: ProtoType,
    /// Effective task queue: `WorkflowOptions.task_queue` if set, else the
    /// service-level default. `None` means neither was supplied — render
    /// will require the caller to pass one.
    pub task_queue: Option<String>,
    /// Parsed form of cludden's `id` Go-template expression, compiled at
    /// parse time against the workflow's input message descriptor. Each
    /// segment is either a literal piece of the template or a reference to
    /// a field on the input message. Render emits a private
    /// `<wf>_id(input: &Input) -> String` function that walks the segments
    /// via `format!`, so the substitution happens at codegen time — no
    /// runtime template engine required.
    pub id_expression: Option<Vec<IdTemplateSegment>>,
    pub id_reuse_policy: Option<IdReusePolicy>,
    pub execution_timeout: Option<Duration>,
    pub run_timeout: Option<Duration>,
    pub task_timeout: Option<Duration>,
    /// Additional names this workflow is also registered under.
    pub aliases: Vec<String>,
    pub attached_signals: Vec<SignalRef>,
    pub attached_queries: Vec<QueryRef>,
    pub attached_updates: Vec<UpdateRef>,
}

/// Reference from a `WorkflowOptions.signal` entry to a sibling signal rpc.
#[derive(Debug, Clone)]
pub struct SignalRef {
    /// Value of the `ref` field — must match a sibling rpc method name.
    pub rpc_method: String,
    /// If `true`, emit a `_with_start` free function alongside the client.
    pub start: bool,
}

#[derive(Debug, Clone)]
pub struct QueryRef {
    pub rpc_method: String,
}

#[derive(Debug, Clone)]
pub struct UpdateRef {
    pub rpc_method: String,
    pub start: bool,
    pub validate: Option<bool>,
}

#[derive(Debug)]
pub struct SignalModel {
    pub rpc_method: String,
    /// Cross-language signal name. Defaults to the proto method's
    /// fully-qualified name `"<package>.<Service>.<Rpc>"` when
    /// `SignalOptions.name` is empty, matching the Go plugin's
    /// `string(method.Desc.FullName())` default.
    pub registered_name: String,
    pub input_type: ProtoType,
    /// Must be `google.protobuf.Empty` — validated.
    pub output_type: ProtoType,
}

#[derive(Debug)]
pub struct QueryModel {
    pub rpc_method: String,
    pub registered_name: String,
    pub input_type: ProtoType,
    pub output_type: ProtoType,
}

#[derive(Debug)]
pub struct UpdateModel {
    pub rpc_method: String,
    pub registered_name: String,
    pub input_type: ProtoType,
    pub output_type: ProtoType,
    /// Whether `UpdateOptions.validate` was set on this rpc.
    pub validate: bool,
}

#[derive(Debug)]
pub struct ActivityModel {
    /// Rpc method name. Activity emit is validate-only in v1, but we still
    /// resolve names so collisions with workflow / signal / query / update
    /// can be rejected.
    pub rpc_method: String,
    pub registered_name: String,
    pub input_type: ProtoType,
    pub output_type: ProtoType,
}

/// A proto type reference, resolved to its fully-qualified name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoType {
    /// Fully-qualified proto type name, e.g. `"jobs.v1.JobInput"` — never
    /// includes the leading `.` that descriptors use.
    pub full_name: String,
    /// `true` when the type is `google.protobuf.Empty`.
    pub is_empty: bool,
}

impl ProtoType {
    pub fn new(full_name: impl Into<String>) -> Self {
        let full_name = full_name.into();
        let normalised = full_name
            .strip_prefix('.')
            .unwrap_or(&full_name)
            .to_string();
        let is_empty = normalised == "google.protobuf.Empty";
        Self {
            full_name: normalised,
            is_empty,
        }
    }

    /// Final path segment of `full_name`. For `Empty`, returns `"()"` to
    /// reflect the render-time substitution.
    pub fn rust_name(&self) -> &str {
        if self.is_empty {
            return "()";
        }
        self.full_name.rsplit('.').next().unwrap_or(&self.full_name)
    }
}

/// One segment of a workflow's `id` template, resolved against the
/// workflow input message's descriptor at parse time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdTemplateSegment {
    /// A literal piece of the template — emitted verbatim into the
    /// generated `format!`.
    Literal(String),
    /// A reference to a field on the workflow input message. The string
    /// is the **Rust** field name (snake_case), so generated code can
    /// substitute `input.<field>` directly. Validated to exist on the
    /// input descriptor at parse time.
    Field(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdReusePolicy {
    AllowDuplicate,
    AllowDuplicateFailedOnly,
    RejectDuplicate,
    TerminateIfRunning,
}

impl IdReusePolicy {
    /// Variant identifier on `temporalio_common::WorkflowIdReusePolicy`.
    pub fn rust_variant(self) -> &'static str {
        match self {
            Self::AllowDuplicate => "AllowDuplicate",
            Self::AllowDuplicateFailedOnly => "AllowDuplicateFailedOnly",
            Self::RejectDuplicate => "RejectDuplicate",
            Self::TerminateIfRunning => "TerminateIfRunning",
        }
    }
}
