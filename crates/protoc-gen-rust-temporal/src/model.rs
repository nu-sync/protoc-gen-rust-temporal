//! Internal representation produced by `parse.rs` and consumed by `render.rs`.
//!
//! The schema deliberately mirrors cludden's `temporal.v1.*` annotation
//! surface (see `proto/temporal/v1/temporal.proto`), only retaining the
//! fields needed for v1 Rust client emit. Anything we read but ignore (XNS,
//! patches, CLI options) lives in the descriptor pool and is silently
//! dropped here.
//!
//! # Adding "reject unsupported X" rules
//!
//! Validation that should refuse a proto field must run **before** that
//! field is projected away. The model layer narrows the schema — rejection
//! code added at a call site that runs after projection has no visibility
//! into the fields the projection threw away, so the user's proto-level
//! setting silently disappears instead of erroring.
//!
//! Concretely: rejection of fields nested inside `WorkflowOptions.signal[]`,
//! `WorkflowOptions.query[]`, and `WorkflowOptions.update[]` lives in
//! `parse::reject_unsupported_workflow_{signal,query,update}_ref`, which
//! runs in `workflow_from` against the raw proto, not against
//! `attached_signals` / `attached_queries` / `attached_updates`.

use std::time::Duration;

/// Parsed `(temporal.v1.cli)` service-level annotation. Each field is
/// `None` / empty when the proto omits it; the renderer applies them
/// only when set so unannotated services emit the historical defaults.
#[derive(Debug, Clone, Default)]
pub struct ServiceCliSpec {
    /// `ignore = true` → suppress the entire CLI module for this
    /// service (overrides the heuristic that drops the module only
    /// when every workflow is `cli.ignore`'d).
    pub ignore: bool,
    /// Top-level command name override (`#[command(name = …)]`).
    pub name: Option<String>,
    /// Top-level command help-text override (`#[command(about = …)]`).
    pub usage: Option<String>,
    /// Extra top-level command aliases (`#[command(alias = […])]`).
    pub aliases: Vec<String>,
}

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
    /// Service-level `(temporal.v1.cli)` overrides for the top-level
    /// `Cli` struct's `#[command(name, about, alias)]` attributes.
    /// `None` when the proto omits the annotation. `Some(spec)` with
    /// `ignore = true` suppresses the CLI module entirely.
    pub cli_options: Option<ServiceCliSpec>,
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
    /// Proto-declared default policy when the start request collides with a
    /// running workflow id. `None` lets the server pick its default.
    pub id_conflict_policy: Option<IdConflictPolicy>,
    /// Proto-declared parent-close policy. Only meaningful when the
    /// workflow runs as a child; render folds it into the per-workflow
    /// `<workflow>_default_child_options()` factory. `None` lets the
    /// server pick its default (`Terminate`).
    pub parent_close_policy: Option<ParentClosePolicyKind>,
    /// Proto-declared `wait_for_cancellation`. Child-only — folds into
    /// `<workflow>_default_child_options()` as `cancel_type:
    /// ChildWorkflowCancellationType::WaitCancellationCompleted`. `false`
    /// (default) emits no setter so the SDK's default behaviour stays.
    pub wait_for_cancellation: bool,
    /// Parsed `(temporal.v1.workflow).search_attributes` Bloblang
    /// expression. `None` means the proto didn't declare any. R7 slice 1
    /// only models the empty-map literal `root = {}`; richer
    /// expressions are still refused at parse with a slice-bounded
    /// diagnostic. See `docs/R7-BLOBLANG.md`.
    pub search_attributes: Option<SearchAttributesSpec>,
    /// Proto-declared default retry policy for the workflow. `None` means
    /// the proto omits the field and the server picks defaults.
    pub retry_policy: Option<RetryPolicySpec>,
    pub execution_timeout: Option<Duration>,
    pub run_timeout: Option<Duration>,
    pub task_timeout: Option<Duration>,
    /// Additional names this workflow is also registered under.
    pub aliases: Vec<String>,
    /// `WorkflowOptions.cli.ignore`: omit this workflow from the generated
    /// CLI scaffold when `cli=true`.
    pub cli_ignore: bool,
    /// `WorkflowOptions.cli.name`: override the kebab-case subcommand name
    /// clap derives from the variant. `None` falls back to the default.
    pub cli_name: Option<String>,
    /// `WorkflowOptions.cli.aliases`: extra clap subcommand aliases. Both
    /// `start-<wf>` and `attach-<wf>` variants inherit them.
    pub cli_aliases: Vec<String>,
    /// `WorkflowOptions.cli.usage`: help-text override emitted as
    /// `#[command(about = "<usage>")]` on both the start and attach
    /// variants. `None` leaves the docstring-derived default in place.
    pub cli_usage: Option<String>,
    /// `WorkflowOptions.enable_eager_start`: ask the server for eager
    /// workflow execution (the request can be satisfied by a local worker
    /// if one has slots, cutting first-task latency). NOTE the upstream
    /// SDK field name is `enable_eager_workflow_start`; we follow the SDK
    /// naming inside generated code so it lines up with the bridge call.
    pub enable_eager_workflow_start: bool,
    pub attached_signals: Vec<SignalRef>,
    pub attached_queries: Vec<QueryRef>,
    pub attached_updates: Vec<UpdateRef>,
}

/// Reference from a `WorkflowOptions.signal` entry to a sibling signal rpc.
#[derive(Debug, Clone)]
pub struct SignalRef {
    /// Value of the `ref` field — same-service refs hold the bare rpc
    /// method name; cross-service refs hold the fully-qualified path
    /// (`pkg.Service.Method`).
    pub rpc_method: String,
    /// If `true`, emit a `_with_start` free function alongside the client.
    pub start: bool,
    /// Metadata captured at parse time when `rpc_method` resolves to an
    /// rpc on a *different* service. `None` for same-service refs (the
    /// existing same-service signal lookup in `render.rs` covers those).
    pub cross_service: Option<CrossServiceTarget>,
    /// Per-`WorkflowOptions.signal[N].cli.name` override. `None` keeps
    /// clap's kebab-case `signal-<name>` default.
    pub cli_name: Option<String>,
    /// Per-`WorkflowOptions.signal[N].cli.aliases` override.
    pub cli_aliases: Vec<String>,
    /// Per-`WorkflowOptions.signal[N].cli.usage` override.
    pub cli_usage: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QueryRef {
    pub rpc_method: String,
    pub cross_service: Option<CrossServiceTarget>,
}

#[derive(Debug, Clone)]
pub struct UpdateRef {
    pub rpc_method: String,
    pub start: bool,
    pub validate: Option<bool>,
    pub cross_service: Option<CrossServiceTarget>,
    /// Per-`(temporal.v1.workflow).update[N].workflow_id_conflict_policy`
    /// override. Threads into the workflow's `<update>_with_start` free
    /// function so the start half honours the proto's choice instead
    /// of falling back to the bridge's `UseExisting` default. `None`
    /// preserves the historical fallback.
    pub id_conflict_policy: Option<IdConflictPolicy>,
}

/// Resolved target of a cross-service `signal` / `query` / `update`
/// ref. Captured at parse time so `render.rs` can emit a typed Handle
/// method without re-traversing the descriptor pool.
#[derive(Debug, Clone)]
pub struct CrossServiceTarget {
    /// Cross-language registration name (the fully-qualified proto
    /// method name, mirroring `<package>.<Service>.<Rpc>`). This is
    /// what gets sent over the wire — `WorkflowHandle::<rpc>` uses it
    /// to find the target signal/query/update handler on the target
    /// workflow.
    pub registered_name: String,
    /// Proto input type of the target rpc.
    pub input_type: ProtoType,
    /// Proto output type of the target rpc. Always `Empty` for signals,
    /// per cludden's schema invariant — carried here for symmetry with
    /// query/update so the same struct works for all three.
    pub output_type: ProtoType,
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
    /// Parsed `UpdateOptions.id` template — a workflow-id template that
    /// targets the *parent* workflow. Compiled at parse time against the
    /// update's *input* descriptor (not the workflow's), so each segment
    /// names a Rust field on the update input. Render emits a private
    /// `<update>_workflow_id(input: &<Input>) -> String` helper plus a
    /// client-level `<update>_by_template` convenience method that uses
    /// the derived id to find the parent workflow.
    pub id_expression: Option<Vec<IdTemplateSegment>>,
    /// Proto-declared default `WaitPolicy` for this update. When the
    /// caller leaves the update method's `wait_policy` arg as `None`,
    /// codegen folds this value in. `None` here means the proto didn't
    /// declare one, so the call must pass `Some(...)`.
    pub default_wait_policy: Option<WaitPolicyKind>,
}

/// Mirror of cludden's `WaitPolicy` enum (sans `Unspecified`, which we
/// model as `Option::None` at call sites). The render layer maps each
/// variant to the bridge facade's `temporal_runtime::WaitPolicy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitPolicyKind {
    Admitted,
    Accepted,
    Completed,
}

impl WaitPolicyKind {
    /// Variant identifier on `temporal_runtime::WaitPolicy`.
    pub fn rust_variant(self) -> &'static str {
        match self {
            Self::Admitted => "Admitted",
            Self::Accepted => "Accepted",
            Self::Completed => "Completed",
        }
    }
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
    /// Proto-declared `ActivityOptions` defaults compiled into a generated
    /// `<activity>_default_options()` factory. `None` here means no
    /// runtime-affecting field was declared and no factory is emitted.
    pub default_options: Option<ActivityOptionsSpec>,
}

/// Compiled form of `(temporal.v1.activity)` runtime fields. Held on the
/// model so render can emit a `<activity>_default_options() -> ActivityOptions`
/// factory bound to the SDK's typestate builder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityOptionsSpec {
    pub task_queue: Option<String>,
    pub schedule_to_close_timeout: Option<Duration>,
    pub schedule_to_start_timeout: Option<Duration>,
    pub start_to_close_timeout: Option<Duration>,
    pub heartbeat_timeout: Option<Duration>,
    pub retry_policy: Option<RetryPolicySpec>,
    /// `true` when the proto declares `wait_for_cancellation = true`.
    /// Render chains
    /// `.cancellation_type(ActivityCancellationType::WaitCancellationCompleted)`
    /// onto the factory builder. `false` (default) maps to the SDK's
    /// default `TryCancel` and emits no setter — matching Go-plugin
    /// behaviour.
    pub wait_for_cancellation: bool,
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

/// Compiled form of `(temporal.v1.workflow).search_attributes`.
/// Slice 1 ships the `Empty` variant; slice 2 ships `Static` with
/// literal key/value entries; slice 3 (deferred) will add a
/// `WithInput` variant for `this.<field>` references.
/// See `docs/R7-BLOBLANG.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchAttributesSpec {
    /// Empty map — proto declared `root = {}`. Emits as a no-op on
    /// the start path.
    Empty,
    /// Non-empty literal map — proto declared
    /// `root = { "Key1": <literal>, "Key2": <literal>, … }` where each
    /// `<literal>` is a string / signed integer / boolean.
    Static(Vec<(String, SearchAttributeLiteral)>),
}

/// One entry in a `Static` `SearchAttributesSpec`. Slices 2 + 3:
/// - `String` / `Int` / `Bool` — slice-2 primitive literals.
/// - `StringField` / `IntField` / `BoolField` — slice-3 `this.<field>`
///   references where `<field>` resolves to a singular `string`,
///   `int64`, or `bool` field on the workflow input message. The
///   string is the Rust snake_case field name (validated against the
///   descriptor at parse time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchAttributeLiteral {
    String(String),
    Int(i64),
    Bool(bool),
    StringField(String),
    IntField(String),
    BoolField(String),
}

/// Compiled form of `(temporal.v1.workflow).retry_policy`. Holds the
/// fields the render layer needs to emit a `temporal_runtime::RetryPolicy`
/// literal at the start path's default-fold step. `backoff_coefficient`
/// is stored as raw bits so the `Eq` derive holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicySpec {
    pub initial_interval: Option<Duration>,
    pub backoff_coefficient_bits: u64,
    pub max_interval: Option<Duration>,
    pub max_attempts: i32,
    pub non_retryable_error_types: Vec<String>,
}

impl RetryPolicySpec {
    pub fn backoff_coefficient(&self) -> f64 {
        f64::from_bits(self.backoff_coefficient_bits)
    }
}

/// Policy a child workflow follows when its parent workflow closes.
/// Mirrors `(temporal.v1.workflow).parent_close_policy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentClosePolicyKind {
    Terminate,
    Abandon,
    RequestCancel,
}

impl ParentClosePolicyKind {
    /// Variant identifier on `temporal_runtime::worker::ParentClosePolicy`.
    pub fn rust_variant(self) -> &'static str {
        match self {
            Self::Terminate => "Terminate",
            Self::Abandon => "Abandon",
            Self::RequestCancel => "RequestCancel",
        }
    }
}

/// Policy for when a start request collides with a **running** workflow.
/// Distinct from [`IdReusePolicy`] — that one targets *closed* runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdConflictPolicy {
    Fail,
    UseExisting,
    TerminateExisting,
}

impl IdConflictPolicy {
    /// Variant identifier on `temporal_runtime::WorkflowIdConflictPolicy`.
    pub fn rust_variant(self) -> &'static str {
        match self {
            Self::Fail => "Fail",
            Self::UseExisting => "UseExisting",
            Self::TerminateExisting => "TerminateExisting",
        }
    }
}
