//! `DescriptorPool` -> `Vec<ServiceModel>` extraction.
//!
//! The descriptor pool is built in `main.rs` via `decode_file_descriptor_set`
//! so that `temporal.v1.*` extensions on `MethodOptions` / `ServiceOptions`
//! survive — prost-types would otherwise drop them silently.
//!
//! Parsing strategy: re-encode each extension `Value` back to bytes through
//! `prost-reflect`'s `DynamicMessage` and decode into the strongly-typed
//! prost message via `prost::Message::decode`. This avoids hand-walking
//! `Value` trees.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use prost::Message;
use prost_reflect::{
    DescriptorPool, DynamicMessage, ExtensionDescriptor, MethodDescriptor, ServiceDescriptor, Value,
};

use crate::model::{
    ActivityModel, IdConflictPolicy, IdReusePolicy, IdTemplateSegment, ProtoType, QueryModel,
    QueryRef, RetryPolicySpec, ServiceModel, SignalModel, SignalRef, UpdateModel, UpdateRef,
    WaitPolicyKind, WorkflowModel,
};
use crate::temporal::api::enums::v1::WorkflowIdConflictPolicy as ProtoConflictPolicy;
use crate::temporal::v1::{
    ActivityOptions, IdReusePolicy as ProtoPolicy, QueryOptions, ServiceOptions, SignalOptions,
    UpdateOptions, WaitPolicy as ProtoWaitPolicy, WorkflowOptions,
};
use heck::ToSnakeCase;

const SERVICE_EXT: &str = "temporal.v1.service";
const WORKFLOW_EXT: &str = "temporal.v1.workflow";
const ACTIVITY_EXT: &str = "temporal.v1.activity";
const SIGNAL_EXT: &str = "temporal.v1.signal";
const QUERY_EXT: &str = "temporal.v1.query";
const UPDATE_EXT: &str = "temporal.v1.update";

struct ExtensionSet {
    service: ExtensionDescriptor,
    workflow: ExtensionDescriptor,
    activity: ExtensionDescriptor,
    signal: ExtensionDescriptor,
    query: ExtensionDescriptor,
    update: ExtensionDescriptor,
}

impl ExtensionSet {
    fn load(pool: &DescriptorPool) -> Result<Self> {
        Ok(Self {
            service: get_ext(pool, SERVICE_EXT)?,
            workflow: get_ext(pool, WORKFLOW_EXT)?,
            activity: get_ext(pool, ACTIVITY_EXT)?,
            signal: get_ext(pool, SIGNAL_EXT)?,
            query: get_ext(pool, QUERY_EXT)?,
            update: get_ext(pool, UPDATE_EXT)?,
        })
    }
}

fn get_ext(pool: &DescriptorPool, name: &str) -> Result<ExtensionDescriptor> {
    pool.get_extension_by_name(name)
        .ok_or_else(|| anyhow!("missing extension definition: {name}"))
}

pub fn parse(
    pool: &DescriptorPool,
    files_to_generate: &HashSet<String>,
) -> Result<Vec<ServiceModel>> {
    // Early-exit when none of the targets carry any services. This matters
    // for buf v2 modules that include the vendored `temporal/v1/temporal.proto`
    // alongside consumer protos: buf sends one CodeGeneratorRequest per
    // target file, so the plugin gets invoked with the annotation schema
    // itself as the target. That file declares the `temporal.v1.*`
    // extensions but uses none of them, so loading `ExtensionSet` would
    // fail when the request only contains the annotation schema and not
    // a file that uses the extensions. Skipping the lookup keeps the
    // plugin a no-op in that case, which is the right answer — there's
    // nothing to render.
    let has_any_services = pool
        .files()
        .filter(|f| files_to_generate.contains(f.name()))
        .any(|f| f.services().next().is_some());
    if !has_any_services {
        return Ok(Vec::new());
    }

    let ext = ExtensionSet::load(pool)?;

    let mut out = Vec::new();
    for file in pool.files() {
        if !files_to_generate.contains(file.name()) {
            continue;
        }
        for service in file.services() {
            if let Some(model) = parse_service(&file, &service, &ext)? {
                out.push(model);
            }
        }
    }
    Ok(out)
}

fn parse_service(
    file: &prost_reflect::FileDescriptor,
    service: &ServiceDescriptor,
    ext: &ExtensionSet,
) -> Result<Option<ServiceModel>> {
    let package = file.package_name().to_string();
    let service_name = service.name().to_string();

    let default_task_queue = service_default_task_queue(service, &ext.service)?;

    let mut workflows = Vec::new();
    let mut signals = Vec::new();
    let mut queries = Vec::new();
    let mut updates = Vec::new();
    let mut activities = Vec::new();

    for method in service.methods() {
        match method_kind(&method, ext)? {
            MethodKind::Workflow(opts) => {
                workflows.push(workflow_from(&method, *opts, &package, &service_name)?);
            }
            MethodKind::Signal(opts) => {
                signals.push(signal_from(&method, opts, &package, &service_name));
            }
            MethodKind::Query(opts) => {
                queries.push(query_from(&method, opts, &package, &service_name));
            }
            MethodKind::Update(opts) => {
                updates.push(update_from(&method, opts, &package, &service_name)?);
            }
            MethodKind::Activity(opts) => {
                activities.push(activity_from(&method, *opts, &package, &service_name)?);
            }
            MethodKind::None => continue,
        }
    }

    if workflows.is_empty()
        && signals.is_empty()
        && queries.is_empty()
        && updates.is_empty()
        && activities.is_empty()
    {
        return Ok(None);
    }

    Ok(Some(ServiceModel {
        package,
        service: service_name,
        source_file: file.name().to_string(),
        default_task_queue,
        workflows,
        signals,
        queries,
        updates,
        activities,
    }))
}

fn service_default_task_queue(
    service: &ServiceDescriptor,
    service_ext: &ExtensionDescriptor,
) -> Result<Option<String>> {
    let opts: DynamicMessage = service.options();
    if !opts.has_extension(service_ext) {
        return Ok(None);
    }
    let value = opts.get_extension(service_ext);
    let bytes = encode_message_value(&value)?;
    let parsed = ServiceOptions::decode(bytes.as_slice())?;
    reject_unsupported_service_options(&parsed, service.name())?;
    Ok((!parsed.task_queue.is_empty()).then_some(parsed.task_queue))
}

/// `task_queue` is the only `ServiceOptions` field the v1 emit honours.
/// `patches` (workflow patch versioning) and `namespace` (deprecated default
/// namespace) would change runtime behaviour but are not threaded through
/// the generator — refuse them rather than silently strip.
fn reject_unsupported_service_options(opts: &ServiceOptions, service: &str) -> Result<()> {
    let mut unsupported: Vec<&'static str> = Vec::new();
    if !opts.patches.is_empty() {
        unsupported.push("patches");
    }
    #[allow(deprecated)] // intentional: see workflow-options namespace comment.
    if !opts.namespace.is_empty() {
        unsupported.push("namespace (deprecated)");
    }
    if unsupported.is_empty() {
        return Ok(());
    }
    Err(anyhow!(
        "{service}: (temporal.v1.service) sets runtime-affecting field(s) {} that the v1 Rust client emit does not yet honour. Remove the field(s) or pin to a generator release that supports them.",
        unsupported.join(", "),
    ))
}

enum MethodKind {
    // WorkflowOptions is ~700 bytes — boxed so MethodKind stays small.
    Workflow(Box<WorkflowOptions>),
    Activity(Box<ActivityOptions>),
    Signal(SignalOptions),
    Query(QueryOptions),
    Update(UpdateOptions),
    None,
}

fn method_kind(method: &MethodDescriptor, ext: &ExtensionSet) -> Result<MethodKind> {
    let opts: DynamicMessage = method.options();

    // Different `temporal.v1.*` extensions live at different extension field
    // numbers, so a single MethodOptions *can* carry more than one of them
    // simultaneously — that's the Go plugin's co-annotation surface (e.g.
    // workflow + activity, signal + activity). The Rust emit does not yet
    // model co-annotations (see ROADMAP.md R1). Refuse the proto rather than
    // silently honour one annotation and drop the others, which would compile
    // clean and ship a service that's missing half its Temporal contract.
    let mut declared: Vec<&'static str> = Vec::new();
    if opts.has_extension(&ext.workflow) {
        declared.push("workflow");
    }
    if opts.has_extension(&ext.activity) {
        declared.push("activity");
    }
    if opts.has_extension(&ext.signal) {
        declared.push("signal");
    }
    if opts.has_extension(&ext.query) {
        declared.push("query");
    }
    if opts.has_extension(&ext.update) {
        declared.push("update");
    }
    if declared.len() > 1 {
        return Err(anyhow!(
            "{}.{}: rpc carries multiple Temporal annotations ({}) — co-annotations are not yet supported by the v1 Rust plugin (see ROADMAP.md R1). Split the rpc or pin to a generator release that supports the combination.",
            method.parent_service().name(),
            method.name(),
            declared.join(" + "),
        ));
    }

    if opts.has_extension(&ext.workflow) {
        return decode_kind::<WorkflowOptions>(&opts.get_extension(&ext.workflow));
    }
    if opts.has_extension(&ext.activity) {
        return decode_kind::<ActivityOptions>(&opts.get_extension(&ext.activity));
    }
    if opts.has_extension(&ext.signal) {
        return decode_kind::<SignalOptions>(&opts.get_extension(&ext.signal));
    }
    if opts.has_extension(&ext.query) {
        return decode_kind::<QueryOptions>(&opts.get_extension(&ext.query));
    }
    if opts.has_extension(&ext.update) {
        return decode_kind::<UpdateOptions>(&opts.get_extension(&ext.update));
    }
    Ok(MethodKind::None)
}

trait IntoMethodKind {
    fn into_kind(self) -> MethodKind;
}

impl IntoMethodKind for WorkflowOptions {
    fn into_kind(self) -> MethodKind {
        MethodKind::Workflow(Box::new(self))
    }
}
impl IntoMethodKind for ActivityOptions {
    fn into_kind(self) -> MethodKind {
        MethodKind::Activity(Box::new(self))
    }
}
impl IntoMethodKind for SignalOptions {
    fn into_kind(self) -> MethodKind {
        MethodKind::Signal(self)
    }
}
impl IntoMethodKind for QueryOptions {
    fn into_kind(self) -> MethodKind {
        MethodKind::Query(self)
    }
}
impl IntoMethodKind for UpdateOptions {
    fn into_kind(self) -> MethodKind {
        MethodKind::Update(self)
    }
}

fn decode_kind<T: Message + Default + IntoMethodKind>(value: &Value) -> Result<MethodKind> {
    let bytes = encode_message_value(value)?;
    let parsed = T::decode(bytes.as_slice())?;
    Ok(parsed.into_kind())
}

fn encode_message_value(value: &Value) -> Result<Vec<u8>> {
    match value {
        Value::Message(m) => Ok(m.encode_to_vec()),
        other => Err(anyhow!("expected message extension, got {other:?}")),
    }
}

fn workflow_from(
    method: &MethodDescriptor,
    opts: WorkflowOptions,
    package: &str,
    service_name: &str,
) -> Result<WorkflowModel> {
    let rpc_method = method.name().to_string();
    reject_unsupported_workflow_options(&opts, service_name, &rpc_method)?;
    reject_unsupported_workflow_signal_ref(&opts.signal, service_name, &rpc_method)?;
    reject_unsupported_workflow_query_ref(&opts.query, service_name, &rpc_method)?;
    reject_unsupported_workflow_update_ref(&opts.update, service_name, &rpc_method)?;
    let registered_name = if opts.name.is_empty() {
        default_registered_name(package, service_name, &rpc_method)
    } else {
        opts.name
    };

    let id_expression = if opts.id.is_empty() {
        None
    } else {
        Some(
            parse_id_template(&opts.id, &method.input()).with_context(|| {
                format!("parse (temporal.v1.workflow).id template on {service_name}.{rpc_method}")
            })?,
        )
    };

    if let Some(cli) = opts.cli.as_ref() {
        reject_unsupported_workflow_cli_options(cli, service_name, &rpc_method)?;
    }
    let cli_ignore = opts.cli.as_ref().is_some_and(|c| c.ignore);
    let enable_eager_workflow_start = opts.enable_eager_start;
    Ok(WorkflowModel {
        rpc_method,
        registered_name,
        input_type: ProtoType::new(method.input().full_name()),
        output_type: ProtoType::new(method.output().full_name()),
        task_queue: (!opts.task_queue.is_empty()).then_some(opts.task_queue),
        id_expression,
        id_reuse_policy: id_reuse_policy_from_proto(opts.id_reuse_policy),
        id_conflict_policy: id_conflict_policy_from_proto(opts.workflow_id_conflict_policy),
        retry_policy: opts.retry_policy.map(retry_policy_from_proto),
        execution_timeout: opts.execution_timeout.and_then(duration_from_proto),
        run_timeout: opts.run_timeout.and_then(duration_from_proto),
        task_timeout: opts.task_timeout.and_then(duration_from_proto),
        aliases: opts.aliases,
        cli_ignore,
        enable_eager_workflow_start,
        attached_signals: opts
            .signal
            .into_iter()
            .map(|s| SignalRef {
                rpc_method: s.r#ref,
                start: s.start,
            })
            .collect(),
        attached_queries: opts
            .query
            .into_iter()
            .map(|q| QueryRef {
                rpc_method: q.r#ref,
            })
            .collect(),
        attached_updates: opts
            .update
            .into_iter()
            .map(|u| UpdateRef {
                rpc_method: u.r#ref,
                start: u.start,
                validate: u.validate,
            })
            .collect(),
    })
}

fn signal_from(
    method: &MethodDescriptor,
    opts: SignalOptions,
    package: &str,
    service: &str,
) -> SignalModel {
    let rpc_method = method.name().to_string();
    let registered_name = if opts.name.is_empty() {
        default_registered_name(package, service, &rpc_method)
    } else {
        opts.name
    };
    SignalModel {
        rpc_method,
        registered_name,
        input_type: ProtoType::new(method.input().full_name()),
        output_type: ProtoType::new(method.output().full_name()),
    }
}

fn query_from(
    method: &MethodDescriptor,
    opts: QueryOptions,
    package: &str,
    service: &str,
) -> QueryModel {
    let rpc_method = method.name().to_string();
    let registered_name = if opts.name.is_empty() {
        default_registered_name(package, service, &rpc_method)
    } else {
        opts.name
    };
    QueryModel {
        rpc_method,
        registered_name,
        input_type: ProtoType::new(method.input().full_name()),
        output_type: ProtoType::new(method.output().full_name()),
    }
}

fn update_from(
    method: &MethodDescriptor,
    opts: UpdateOptions,
    package: &str,
    service: &str,
) -> Result<UpdateModel> {
    let rpc_method = method.name().to_string();
    reject_unsupported_update_options(&opts, service, &rpc_method)?;
    let registered_name = if opts.name.is_empty() {
        default_registered_name(package, service, &rpc_method)
    } else {
        opts.name
    };
    let id_expression = if opts.id.is_empty() {
        None
    } else {
        Some(
            parse_id_template(&opts.id, &method.input()).with_context(|| {
                format!("parse (temporal.v1.update).id template on {service}.{rpc_method}")
            })?,
        )
    };
    // Resolve the proto-declared default wait policy. `wait_for_stage` is
    // the live field; `wait_policy` (deprecated) is the legacy predecessor.
    // Per cludden's Go plugin, prefer `wait_for_stage` when both are set,
    // and treat the deprecated `wait_policy` as the fallback so ports from
    // Go-side legacy protos still honour the declared default.
    #[allow(deprecated)]
    let raw_wait = if opts.wait_for_stage != 0 {
        opts.wait_for_stage
    } else {
        opts.wait_policy
    };
    let default_wait_policy = wait_policy_from_proto(raw_wait);
    Ok(UpdateModel {
        rpc_method,
        registered_name,
        input_type: ProtoType::new(method.input().full_name()),
        output_type: ProtoType::new(method.output().full_name()),
        validate: opts.validate,
        id_expression,
        default_wait_policy,
    })
}

fn wait_policy_from_proto(raw: i32) -> Option<WaitPolicyKind> {
    match ProtoWaitPolicy::try_from(raw).ok()? {
        ProtoWaitPolicy::Unspecified => None,
        ProtoWaitPolicy::Admitted => Some(WaitPolicyKind::Admitted),
        ProtoWaitPolicy::Accepted => Some(WaitPolicyKind::Accepted),
        ProtoWaitPolicy::Completed => Some(WaitPolicyKind::Completed),
    }
}

fn activity_from(
    method: &MethodDescriptor,
    opts: ActivityOptions,
    package: &str,
    service: &str,
) -> Result<ActivityModel> {
    let rpc_method = method.name().to_string();
    reject_unsupported_activity_options(&opts, service, &rpc_method)?;
    let registered_name = if opts.name.is_empty() {
        default_registered_name(package, service, &rpc_method)
    } else {
        opts.name
    };
    Ok(ActivityModel {
        rpc_method,
        registered_name,
        input_type: ProtoType::new(method.input().full_name()),
        output_type: ProtoType::new(method.output().full_name()),
    })
}

/// v1 client emit honours a deliberate subset of `WorkflowOptions`. Fields
/// that would change runtime behaviour but are *not* yet plumbed through
/// the generator must be a hard error — silent drops cause hard-to-debug
/// production divergences (the user sets `retry_policy` in the proto,
/// observes the generated client compiling clean, and ships a workflow
/// with no retry policy at all). The Go plugin honours every field below;
/// this Rust plugin will too, once each lands in the emit pipeline.
fn reject_unsupported_workflow_options(
    opts: &WorkflowOptions,
    service: &str,
    rpc: &str,
) -> Result<()> {
    let mut unsupported: Vec<&'static str> = Vec::new();
    if !opts.search_attributes.is_empty() {
        unsupported.push("search_attributes");
    }
    if !opts.typed_search_attributes.is_empty() {
        unsupported.push("typed_search_attributes");
    }
    if opts.parent_close_policy != 0 {
        unsupported.push("parent_close_policy");
    }
    if opts.wait_for_cancellation {
        unsupported.push("wait_for_cancellation");
    }
    if opts.versioning_behavior != 0 {
        unsupported.push("versioning_behavior");
    }
    if !opts.patches.is_empty() {
        unsupported.push("patches");
    }
    // `namespace` is `[deprecated = true]` in cludden's schema but still in
    // wide use on Go-side protos. Refusing it surfaces the port-to-Rust gap
    // explicitly instead of letting workflows fan out to the wrong namespace.
    #[allow(deprecated)] // intentional: see comment above.
    if !opts.namespace.is_empty() {
        unsupported.push("namespace (deprecated)");
    }
    if unsupported.is_empty() {
        return Ok(());
    }
    Err(anyhow!(
        "{service}.{rpc}: (temporal.v1.workflow) sets runtime-affecting field(s) {} that the v1 Rust client emit does not yet honour. Remove the field(s) or pin to a generator release that supports them.",
        unsupported.join(", "),
    ))
}

/// `UpdateOptions` has no still-rejected fields after R5. `id`,
/// `wait_for_stage`, and the deprecated `wait_policy` are all parsed into
/// the model. This function stays as the rejection sink for future fields
/// the schema grows.
fn reject_unsupported_update_options(
    _opts: &UpdateOptions,
    _service: &str,
    _rpc: &str,
) -> Result<()> {
    Ok(())
}

/// Per-update fields nested inside `WorkflowOptions.update[]` that the v1
/// emit drops. The bridge's `update-with-start` path hardcodes
/// `WorkflowIdConflictPolicy::UseExisting`, so a proto-level override would
/// be silently ignored — refuse the proto rather than ship the wrong policy.
///
/// **Why this lives in parse, not model:** rejection must run against the
/// raw `WorkflowOptions.Update` proto, before [`workflow_from`] projects
/// it into the narrower [`crate::model::UpdateRef`] (which intentionally
/// drops `cli` / `xns` / `workflow_id_conflict_policy`). Any future
/// "reject unsupported X" fix on nested refs belongs here too.
fn reject_unsupported_workflow_update_ref(
    refs: &[crate::temporal::v1::workflow_options::Update],
    service: &str,
    rpc: &str,
) -> Result<()> {
    for r in refs {
        let mut unsupported: Vec<&'static str> = Vec::new();
        if r.workflow_id_conflict_policy != 0 {
            unsupported.push("workflow_id_conflict_policy");
        }
        if r.cli.is_some() {
            unsupported.push("cli");
        }
        if r.xns.is_some() {
            unsupported.push("xns");
        }
        if unsupported.is_empty() {
            continue;
        }
        return Err(anyhow!(
            "{service}.{rpc}: (temporal.v1.workflow).update[ref={}] sets field(s) {} that the v1 Rust client emit does not yet honour. Remove the field(s) or pin to a generator release that supports them.",
            r.r#ref,
            unsupported.join(", "),
        ));
    }
    Ok(())
}

/// `WorkflowOptions.cli` is the per-workflow CLI override block. Today the
/// only field threaded into emit is `ignore` (filters the workflow out of
/// the `cli=true` scaffold). The other fields — `name`, `usage`, and
/// `aliases` — would change the generated command's user-facing surface
/// but are not yet plumbed through, so honouring `ignore` silently while
/// dropping them would surprise users. Reject the silent-drop cases.
fn reject_unsupported_workflow_cli_options(
    cli: &crate::temporal::v1::CliCommandOptions,
    service: &str,
    rpc: &str,
) -> Result<()> {
    let mut unsupported: Vec<&'static str> = Vec::new();
    if !cli.name.is_empty() {
        unsupported.push("cli.name");
    }
    if !cli.usage.is_empty() {
        unsupported.push("cli.usage");
    }
    if !cli.aliases.is_empty() {
        unsupported.push("cli.aliases");
    }
    if unsupported.is_empty() {
        return Ok(());
    }
    Err(anyhow!(
        "{service}.{rpc}: (temporal.v1.workflow).cli sets field(s) {} that the v1 Rust client emit does not yet honour. Remove the field(s) or pin to a generator release that supports them.",
        unsupported.join(", "),
    ))
}

/// Sibling of [`reject_unsupported_workflow_update_ref`] for `signal` refs
/// nested in `WorkflowOptions.signal[]`. The model layer projects only
/// `ref` and `start`, dropping `cli` and `xns` silently.
fn reject_unsupported_workflow_signal_ref(
    refs: &[crate::temporal::v1::workflow_options::Signal],
    service: &str,
    rpc: &str,
) -> Result<()> {
    for r in refs {
        let mut unsupported: Vec<&'static str> = Vec::new();
        if r.cli.is_some() {
            unsupported.push("cli");
        }
        if r.xns.is_some() {
            unsupported.push("xns");
        }
        if unsupported.is_empty() {
            continue;
        }
        return Err(anyhow!(
            "{service}.{rpc}: (temporal.v1.workflow).signal[ref={}] sets field(s) {} that the v1 Rust client emit does not yet honour. Remove the field(s) or pin to a generator release that supports them.",
            r.r#ref,
            unsupported.join(", "),
        ));
    }
    Ok(())
}

/// Sibling of [`reject_unsupported_workflow_update_ref`] for `query` refs
/// nested in `WorkflowOptions.query[]`. The model layer projects only
/// `ref`, dropping `xns` silently. (Queries have no `cli` field on the
/// nested ref.)
fn reject_unsupported_workflow_query_ref(
    refs: &[crate::temporal::v1::workflow_options::Query],
    service: &str,
    rpc: &str,
) -> Result<()> {
    for r in refs {
        if r.xns.is_some() {
            return Err(anyhow!(
                "{service}.{rpc}: (temporal.v1.workflow).query[ref={}] sets field(s) xns that the v1 Rust client emit does not yet honour. Remove the field(s) or pin to a generator release that supports them.",
                r.r#ref,
            ));
        }
    }
    Ok(())
}

/// In v1 the plugin only emits a name-const + trait surface for activities
/// (under `activities=true`); none of `ActivityOptions`' runtime fields
/// (timeouts, task_queue override, retry policy, wait-for-cancellation)
/// flow into generated code. Refuse silent drops here for the same reason
/// as workflow options.
fn reject_unsupported_activity_options(
    opts: &ActivityOptions,
    service: &str,
    rpc: &str,
) -> Result<()> {
    let mut unsupported: Vec<&'static str> = Vec::new();
    if !opts.task_queue.is_empty() {
        unsupported.push("task_queue");
    }
    if opts.schedule_to_close_timeout.is_some() {
        unsupported.push("schedule_to_close_timeout");
    }
    if opts.schedule_to_start_timeout.is_some() {
        unsupported.push("schedule_to_start_timeout");
    }
    if opts.start_to_close_timeout.is_some() {
        unsupported.push("start_to_close_timeout");
    }
    if opts.heartbeat_timeout.is_some() {
        unsupported.push("heartbeat_timeout");
    }
    if opts.wait_for_cancellation {
        unsupported.push("wait_for_cancellation");
    }
    if opts.retry_policy.is_some() {
        unsupported.push("retry_policy");
    }
    if unsupported.is_empty() {
        return Ok(());
    }
    Err(anyhow!(
        "{service}.{rpc}: (temporal.v1.activity) sets runtime-affecting field(s) {} that the v1 Rust activity emit (activities=true) does not yet honour. Remove the field(s) or pin to a generator release that supports them.",
        unsupported.join(", "),
    ))
}

/// Default cross-language registration name for any annotated rpc.
///
/// Mirrors `protoreflect.FullName` semantics used by
/// `cludden/protoc-gen-go-temporal`: `"<package>.<Service>.<Rpc>"` (dots
/// only — *no* slash). The Go plugin defaults workflow / signal / query /
/// update / activity names to `string(method.Desc.FullName())`, so we must
/// produce the same string for mixed-language workers (Rust workflow, Go
/// signal sender; or vice versa) to find each other on the wire.
fn default_registered_name(package: &str, service: &str, rpc: &str) -> String {
    if package.is_empty() {
        format!("{service}.{rpc}")
    } else {
        format!("{package}.{service}.{rpc}")
    }
}

fn id_reuse_policy_from_proto(raw: i32) -> Option<IdReusePolicy> {
    match ProtoPolicy::try_from(raw).ok()? {
        ProtoPolicy::WorkflowIdReusePolicyUnspecified => None,
        ProtoPolicy::WorkflowIdReusePolicyAllowDuplicate => Some(IdReusePolicy::AllowDuplicate),
        ProtoPolicy::WorkflowIdReusePolicyAllowDuplicateFailedOnly => {
            Some(IdReusePolicy::AllowDuplicateFailedOnly)
        }
        ProtoPolicy::WorkflowIdReusePolicyRejectDuplicate => Some(IdReusePolicy::RejectDuplicate),
        ProtoPolicy::WorkflowIdReusePolicyTerminateIfRunning => {
            Some(IdReusePolicy::TerminateIfRunning)
        }
    }
}

fn id_conflict_policy_from_proto(raw: i32) -> Option<IdConflictPolicy> {
    match ProtoConflictPolicy::try_from(raw).ok()? {
        ProtoConflictPolicy::Unspecified => None,
        ProtoConflictPolicy::Fail => Some(IdConflictPolicy::Fail),
        ProtoConflictPolicy::UseExisting => Some(IdConflictPolicy::UseExisting),
        ProtoConflictPolicy::TerminateExisting => Some(IdConflictPolicy::TerminateExisting),
    }
}

fn retry_policy_from_proto(p: crate::temporal::v1::RetryPolicy) -> RetryPolicySpec {
    RetryPolicySpec {
        initial_interval: p.initial_interval.and_then(duration_from_proto),
        backoff_coefficient_bits: p.backoff_coefficient.to_bits(),
        max_interval: p.max_interval.and_then(duration_from_proto),
        max_attempts: p.max_attempts,
        non_retryable_error_types: p.non_retryable_error_types,
    }
}

fn duration_from_proto(d: prost_types::Duration) -> Option<Duration> {
    if d.seconds < 0 || d.nanos < 0 {
        return None;
    }
    let secs = u64::try_from(d.seconds).ok()?;
    let nanos = u32::try_from(d.nanos).ok()?;
    Some(Duration::new(secs, nanos))
}

/// Parse a cludden-style id template into segments, resolving each
/// `{{ .FieldName }}` reference against the workflow input descriptor.
///
/// Supports only the simple form `{{ .FieldName }}` (with optional
/// whitespace inside the braces). More complex Go-template syntax
/// (conditionals, functions, ranges) returns an error so users see the
/// limitation up front rather than at runtime.
///
/// Bloblang expressions (`${! ... }`) — used by cludden's Go plugin for
/// search-attribute mappings — are rejected here. They look like literal
/// text to the `{{...}}` scanner, which would otherwise let them through
/// as a static workflow ID and silently collide every execution.
fn parse_id_template(
    template: &str,
    input: &prost_reflect::MessageDescriptor,
) -> Result<Vec<IdTemplateSegment>> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        if open > 0 {
            let literal = &rest[..open];
            reject_bloblang(literal, template)?;
            out.push(IdTemplateSegment::Literal(literal.to_string()));
        }
        let after_open = &rest[open + 2..];
        let close = after_open
            .find("}}")
            .ok_or_else(|| anyhow!("unterminated `{{{{` in id template {template:?}"))?;
        let token = after_open[..close].trim();
        let field_name = token
            .strip_prefix('.')
            .ok_or_else(|| {
                anyhow!(
                    "id template token {token:?} must start with `.` (only field references are supported; \
                     conditionals / pipelines / functions are not implemented)"
                )
            })?
            .trim();
        if field_name.is_empty() {
            anyhow::bail!("id template token has no field name after `.`");
        }
        if !field_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            anyhow::bail!(
                "id template token {field_name:?} contains unsupported characters \
                 (only simple field references like `.Name` are supported)"
            );
        }
        let rust_field = field_name.to_snake_case();
        let known = input.fields().any(|f| f.name() == rust_field);
        if !known {
            anyhow::bail!(
                "id template references `{field_name}` (looked up as `{rust_field}`) \
                 but no such field exists on input message `{}`",
                input.full_name()
            );
        }
        out.push(IdTemplateSegment::Field(rust_field));
        rest = &after_open[close + 2..];
    }
    if !rest.is_empty() {
        reject_bloblang(rest, template)?;
        out.push(IdTemplateSegment::Literal(rest.to_string()));
    }
    Ok(out)
}

/// Bloblang `${...}` and `${! ...}` expressions are an unrelated templating
/// dialect cludden's Go plugin uses for search-attribute mappings. The id
/// template scanner only knows `{{...}}`, so a Bloblang expression slips
/// through as a literal — which would compile clean and ship, then collide
/// every workflow under the same literal ID at runtime. Refuse them up-front
/// so users see the limitation at codegen time.
fn reject_bloblang(literal: &str, template: &str) -> Result<()> {
    if literal.contains("${") {
        anyhow::bail!(
            "id template {template:?} contains a Bloblang expression (`${{...}}`); \
             only Go-template `{{{{ .Field }}}}` references are supported in workflow ids"
        );
    }
    Ok(())
}
