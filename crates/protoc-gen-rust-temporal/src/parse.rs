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
    ActivityModel, IdConflictPolicy, IdReusePolicy, IdTemplateSegment, ParentClosePolicyKind,
    ProtoType, QueryModel, QueryRef, RetryPolicySpec, ServiceModel, SignalModel, SignalRef,
    UpdateModel, UpdateRef, WaitPolicyKind, WorkflowModel,
};
use crate::temporal::api::enums::v1::WorkflowIdConflictPolicy as ProtoConflictPolicy;
use crate::temporal::v1::ParentClosePolicy as ProtoParentClosePolicy;
use crate::temporal::v1::{
    ActivityOptions, IdReusePolicy as ProtoPolicy, QueryOptions, ServiceOptions, SignalOptions,
    UpdateOptions, WaitPolicy as ProtoWaitPolicy, WorkflowOptions,
};
use heck::ToSnakeCase;

const SERVICE_EXT: &str = "temporal.v1.service";
const SERVICE_CLI_EXT: &str = "temporal.v1.cli";
const WORKFLOW_EXT: &str = "temporal.v1.workflow";
const ACTIVITY_EXT: &str = "temporal.v1.activity";
const SIGNAL_EXT: &str = "temporal.v1.signal";
const QUERY_EXT: &str = "temporal.v1.query";
const UPDATE_EXT: &str = "temporal.v1.update";

struct ExtensionSet {
    service: ExtensionDescriptor,
    service_cli: ExtensionDescriptor,
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
            service_cli: get_ext(pool, SERVICE_CLI_EXT)?,
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

/// Resolve a fully-qualified rpc reference (e.g.
/// `other.v1.OtherService.Cancel`) against the descriptor pool. Returns
/// `Ok(())` if the target exists *and* carries an extension of the
/// expected `kind` (`"signal"`, `"query"`, or `"update"`). Returns a
/// diagnostic anyhow Err otherwise. Used by parse.rs to validate
/// cross-service refs early — the validate.rs "not yet emitted"
/// rejection still fires for these, but at least now we know the target
/// is real and well-annotated.
fn resolve_cross_service_ref(
    pool: &DescriptorPool,
    ext: &ExtensionSet,
    parent_service: &str,
    parent_rpc: &str,
    target_path: &str,
    kind: &'static str,
) -> Result<crate::model::CrossServiceTarget> {
    // Walk every service in the pool and look for a method whose
    // fully-qualified name matches `target_path`. prost-reflect doesn't
    // expose a direct `get_method_by_name`, but services()+methods() is
    // O(n) over a typically-small pool and runs once per cross-service
    // ref at codegen time.
    let target_method = pool
        .services()
        .flat_map(|svc| svc.methods().collect::<Vec<_>>())
        .find(|m| m.full_name() == target_path);
    let target_method = target_method.ok_or_else(|| {
        anyhow!(
            "{parent_service}.{parent_rpc}: cross-service {kind} ref `{target_path}` doesn't resolve to any rpc in the descriptor pool — check the spelling and confirm the target proto is in the buf module's import graph"
        )
    })?;
    let target_opts = target_method.options();
    let expected_ext = match kind {
        "signal" => &ext.signal,
        "query" => &ext.query,
        "update" => &ext.update,
        _ => unreachable!("only signal/query/update refs cross services"),
    };
    if !target_opts.has_extension(expected_ext) {
        return Err(anyhow!(
            "{parent_service}.{parent_rpc}: cross-service {kind} ref `{target_path}` resolves to a real rpc but the target method does not carry `(temporal.v1.{kind})`; either add the annotation on the target or fix the ref"
        ));
    }
    // Decode the target annotation to pull out an override `name`. When
    // unset, the registered name is the proto method's full name
    // (matches the same-service default).
    let registered_name = {
        let value = target_opts.get_extension(expected_ext);
        let bytes = encode_message_value(&value)?;
        let override_name: Option<String> = match kind {
            "signal" => {
                let parsed = SignalOptions::decode(bytes.as_slice())?;
                (!parsed.name.is_empty()).then_some(parsed.name)
            }
            "query" => {
                let parsed = QueryOptions::decode(bytes.as_slice())?;
                (!parsed.name.is_empty()).then_some(parsed.name)
            }
            "update" => {
                let parsed = UpdateOptions::decode(bytes.as_slice())?;
                (!parsed.name.is_empty()).then_some(parsed.name)
            }
            _ => unreachable!(),
        };
        override_name.unwrap_or_else(|| target_method.full_name().to_string())
    };
    Ok(crate::model::CrossServiceTarget {
        registered_name,
        input_type: ProtoType::new(target_method.input().full_name()),
        output_type: ProtoType::new(target_method.output().full_name()),
    })
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
            if let Some(model) = parse_service(pool, &file, &service, &ext)? {
                out.push(model);
            }
        }
    }
    Ok(out)
}

fn parse_service(
    pool: &DescriptorPool,
    file: &prost_reflect::FileDescriptor,
    service: &ServiceDescriptor,
    ext: &ExtensionSet,
) -> Result<Option<ServiceModel>> {
    let package = file.package_name().to_string();
    let service_name = service.name().to_string();

    let default_task_queue = service_default_task_queue(service, &ext.service)?;
    let cli_options = service_cli_spec(service, &ext.service_cli)?;

    let mut workflows = Vec::new();
    let mut signals = Vec::new();
    let mut queries = Vec::new();
    let mut updates = Vec::new();
    let mut activities = Vec::new();

    for method in service.methods() {
        for kind in method_kinds(&method, ext)? {
            match kind {
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
            }
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

    // R1 — cross-service ref resolution. Walks attached signal/query/
    // update refs that contain a dot, resolves the target through the
    // DescriptorPool, and stashes the target metadata
    // (registered_name + I/O types) on the ref so render.rs can emit a
    // typed Handle method without re-traversing the pool. Parse errors
    // surface typos (unresolved) and wrong-kind targets (resolved but
    // missing `(temporal.v1.{kind})`) before validate.rs's downstream
    // checks.
    for wf in workflows.iter_mut() {
        for sref in wf.attached_signals.iter_mut() {
            if sref.rpc_method.contains('.') {
                sref.cross_service = Some(resolve_cross_service_ref(
                    pool,
                    ext,
                    &service_name,
                    &wf.rpc_method,
                    &sref.rpc_method,
                    "signal",
                )?);
            }
        }
        for qref in wf.attached_queries.iter_mut() {
            if qref.rpc_method.contains('.') {
                qref.cross_service = Some(resolve_cross_service_ref(
                    pool,
                    ext,
                    &service_name,
                    &wf.rpc_method,
                    &qref.rpc_method,
                    "query",
                )?);
            }
        }
        for uref in wf.attached_updates.iter_mut() {
            if uref.rpc_method.contains('.') {
                uref.cross_service = Some(resolve_cross_service_ref(
                    pool,
                    ext,
                    &service_name,
                    &wf.rpc_method,
                    &uref.rpc_method,
                    "update",
                )?);
            }
        }
    }

    Ok(Some(ServiceModel {
        package,
        service: service_name,
        source_file: file.name().to_string(),
        default_task_queue,
        cli_options,
        workflows,
        signals,
        queries,
        updates,
        activities,
    }))
}

/// Parse the service-level `(temporal.v1.cli)` annotation, if present.
/// Cludden's plugin uses this distinct extension (separate from
/// `(temporal.v1.service)`) to override the top-level CLI binary's
/// surface — name, about text, aliases, and an `ignore` flag that
/// suppresses CLI emit entirely.
fn service_cli_spec(
    service: &ServiceDescriptor,
    service_cli_ext: &ExtensionDescriptor,
) -> Result<Option<crate::model::ServiceCliSpec>> {
    let opts: DynamicMessage = service.options();
    if !opts.has_extension(service_cli_ext) {
        return Ok(None);
    }
    let value = opts.get_extension(service_cli_ext);
    let bytes = encode_message_value(&value)?;
    let parsed = crate::temporal::v1::CliOptions::decode(bytes.as_slice())?;
    Ok(Some(crate::model::ServiceCliSpec {
        ignore: parsed.ignore,
        name: (!parsed.name.is_empty()).then_some(parsed.name),
        usage: (!parsed.usage.is_empty()).then_some(parsed.usage),
        aliases: parsed.aliases,
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
}

/// R1 — co-annotation support. Different `temporal.v1.*` extensions live
/// at different extension field numbers, so a single MethodOptions can
/// carry more than one simultaneously. Cludden's Go plugin supports
/// useful combinations (workflow+activity, signal+activity,
/// update+activity); we mirror that surface by parsing every present
/// extension into its own `MethodKind`. Combinations involving
/// `workflow + signal`, `workflow + query`, etc. are still refused
/// because the emit shape doesn't model them (a single rpc as both a
/// top-level workflow client method *and* a workflow-attached
/// signal/query handler would collide on the generated symbol).
fn method_kinds(method: &MethodDescriptor, ext: &ExtensionSet) -> Result<Vec<MethodKind>> {
    let opts: DynamicMessage = method.options();
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

    // Refuse combinations the emit can't model. `activity` pairs cleanly
    // with `workflow` / `signal` / `update` because activity emit lives
    // in a separate trait surface that doesn't share symbols with the
    // client / handler emit. Everything else collapses to a no-go.
    let has_activity = declared.contains(&"activity");
    let primary_count = declared.iter().filter(|d| **d != "activity").count();
    if primary_count > 1 {
        return Err(anyhow!(
            "{}.{}: rpc carries multiple non-activity Temporal annotations ({}) — only `activity` may co-occur with another kind; the other primary kinds share generated symbols and would collide.",
            method.parent_service().name(),
            method.name(),
            declared.join(" + "),
        ));
    }
    let _ = has_activity; // currently only inspected via primary_count

    let mut out = Vec::new();
    if opts.has_extension(&ext.workflow) {
        out.push(decode_kind::<WorkflowOptions>(
            &opts.get_extension(&ext.workflow),
        )?);
    }
    if opts.has_extension(&ext.activity) {
        out.push(decode_kind::<ActivityOptions>(
            &opts.get_extension(&ext.activity),
        )?);
    }
    if opts.has_extension(&ext.signal) {
        out.push(decode_kind::<SignalOptions>(
            &opts.get_extension(&ext.signal),
        )?);
    }
    if opts.has_extension(&ext.query) {
        out.push(decode_kind::<QueryOptions>(
            &opts.get_extension(&ext.query),
        )?);
    }
    if opts.has_extension(&ext.update) {
        out.push(decode_kind::<UpdateOptions>(
            &opts.get_extension(&ext.update),
        )?);
    }
    Ok(out)
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
    reject_unsupported_workflow_options(&opts, service_name, &rpc_method, &method.input())?;
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

    let cli_ignore = opts.cli.as_ref().is_some_and(|c| c.ignore);
    let cli_name = opts
        .cli
        .as_ref()
        .and_then(|c| (!c.name.is_empty()).then(|| c.name.clone()));
    let cli_aliases = opts
        .cli
        .as_ref()
        .map(|c| c.aliases.clone())
        .unwrap_or_default();
    let cli_usage = opts
        .cli
        .as_ref()
        .and_then(|c| (!c.usage.is_empty()).then(|| c.usage.clone()));
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
        parent_close_policy: parent_close_policy_from_proto(opts.parent_close_policy),
        wait_for_cancellation: opts.wait_for_cancellation,
        search_attributes: parse_search_attributes_spec(&opts.search_attributes, &method.input()),
        retry_policy: opts.retry_policy.map(retry_policy_from_proto),
        execution_timeout: opts.execution_timeout.and_then(duration_from_proto),
        run_timeout: opts.run_timeout.and_then(duration_from_proto),
        task_timeout: opts.task_timeout.and_then(duration_from_proto),
        aliases: opts.aliases,
        cli_ignore,
        cli_name,
        cli_aliases,
        cli_usage,
        enable_eager_workflow_start,
        attached_signals: opts
            .signal
            .into_iter()
            .map(|s| {
                let (cli_name, cli_aliases, cli_usage) = match s.cli.as_ref() {
                    Some(c) => (
                        (!c.name.is_empty()).then(|| c.name.clone()),
                        c.aliases.clone(),
                        (!c.usage.is_empty()).then(|| c.usage.clone()),
                    ),
                    None => (None, Vec::new(), None),
                };
                SignalRef {
                    rpc_method: s.r#ref,
                    start: s.start,
                    cross_service: None,
                    cli_name,
                    cli_aliases,
                    cli_usage,
                }
            })
            .collect(),
        attached_queries: opts
            .query
            .into_iter()
            .map(|q| QueryRef {
                rpc_method: q.r#ref,
                cross_service: None,
            })
            .collect(),
        attached_updates: opts
            .update
            .into_iter()
            .map(|u| {
                let (cli_name, cli_aliases, cli_usage) = match u.cli.as_ref() {
                    Some(c) => (
                        (!c.name.is_empty()).then(|| c.name.clone()),
                        c.aliases.clone(),
                        (!c.usage.is_empty()).then(|| c.usage.clone()),
                    ),
                    None => (None, Vec::new(), None),
                };
                UpdateRef {
                    rpc_method: u.r#ref,
                    start: u.start,
                    validate: u.validate,
                    cross_service: None,
                    id_conflict_policy: id_conflict_policy_from_proto(
                        u.workflow_id_conflict_policy,
                    ),
                    cli_name,
                    cli_aliases,
                    cli_usage,
                }
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
    let (cli_name, cli_aliases, cli_usage) = match opts.cli.as_ref() {
        Some(c) => (
            (!c.name.is_empty()).then(|| c.name.clone()),
            c.aliases.clone(),
            (!c.usage.is_empty()).then(|| c.usage.clone()),
        ),
        None => (None, Vec::new(), None),
    };
    SignalModel {
        rpc_method,
        registered_name,
        input_type: ProtoType::new(method.input().full_name()),
        output_type: ProtoType::new(method.output().full_name()),
        cli_name,
        cli_aliases,
        cli_usage,
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
    let default_options = activity_options_spec_from_proto(&opts);
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
        default_options,
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
    input: &prost_reflect::MessageDescriptor,
) -> Result<()> {
    let mut unsupported: Vec<&'static str> = Vec::new();
    // R7 slices 1 + 2 + 3a: the empty-map literal (`root = {}`),
    // primitive literal maps (`root = { "Key": "v", … }`), and
    // `this.<field>` references for `string`-typed input fields all
    // parse here. Anything else (richer Bloblang, non-string field
    // refs, missing fields) falls through and lands in the standard
    // "does not yet honour" diagnostic below.
    if !opts.search_attributes.is_empty()
        && parse_search_attributes_spec(&opts.search_attributes, input).is_none()
    {
        unsupported.push("search_attributes");
    }
    if !opts.typed_search_attributes.is_empty() {
        unsupported.push("typed_search_attributes");
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
/// emit drops. `workflow_id_conflict_policy` (R5) and `cli` (R6) both
/// thread through now; only `xns` remains rejected.
///
/// **Why this lives in parse, not model:** rejection must run against the
/// raw `WorkflowOptions.Update` proto, before [`workflow_from`] projects
/// it into the narrower [`crate::model::UpdateRef`].
fn reject_unsupported_workflow_update_ref(
    refs: &[crate::temporal::v1::workflow_options::Update],
    service: &str,
    rpc: &str,
) -> Result<()> {
    for r in refs {
        let mut unsupported: Vec<&'static str> = Vec::new();
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

/// All `ActivityOptions` runtime fields now fold into the
/// `<activity>_default_options()` factory under `activities=true`. This
/// function stays as the rejection sink for future fields the schema
/// grows.
fn reject_unsupported_activity_options(
    _opts: &ActivityOptions,
    _service: &str,
    _rpc: &str,
) -> Result<()> {
    Ok(())
}

/// R7 slice 1 + 2 — recognise the subset of Bloblang search-attribute
/// expressions the plugin currently supports:
///
/// * Slice 1 — empty map: `root = {}`
/// * Slice 2 — non-empty literal map:
///   `root = { "Key1": "value", "Key2": 42, "Key3": true }`
///
/// `<value>` is a string literal, signed-integer literal, or boolean
/// (`true` / `false`). Field references (`this.<field>`) and richer
/// Bloblang surface fall through to `None` and the caller surfaces the
/// standard unsupported-field diagnostic.
fn parse_search_attributes_spec(
    raw: &str,
    input: &prost_reflect::MessageDescriptor,
) -> Option<crate::model::SearchAttributesSpec> {
    use crate::model::SearchAttributesSpec;

    let s = raw.trim();
    // Strip the `root =` prefix (whitespace-tolerant).
    let body = s.strip_prefix("root")?.trim_start();
    let body = body.strip_prefix('=')?.trim_start();
    // Body should be a brace-delimited block.
    let body = body.strip_prefix('{')?.trim_start();
    let body = body.strip_suffix('}').or_else(|| body.strip_suffix(" }"))?;
    let body = body.trim();
    if body.is_empty() {
        return Some(SearchAttributesSpec::Empty);
    }

    // Split into entries on commas. We don't allow commas inside string
    // literals here; cludden's examples don't use them and the
    // simplification keeps the lexer trivial. If slice 3 needs richer
    // strings the lexer graduates.
    let mut entries = Vec::new();
    for entry in body.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            // Trailing comma — tolerate.
            continue;
        }
        // Each entry is `"key": value`.
        let (key_part, value_part) = entry.split_once(':')?;
        let key_part = key_part.trim();
        let value_part = value_part.trim();
        // Key must be a quoted string.
        let key = key_part
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))?;
        // Reject keys containing escape sequences for slice 2 simplicity.
        if key.contains('\\') {
            return None;
        }
        let value = parse_search_attribute_literal(value_part, input)?;
        entries.push((key.to_string(), value));
    }
    Some(SearchAttributesSpec::Static(entries))
}

fn parse_search_attribute_literal(
    raw: &str,
    input: &prost_reflect::MessageDescriptor,
) -> Option<crate::model::SearchAttributeLiteral> {
    use crate::model::SearchAttributeLiteral;
    use heck::ToSnakeCase;
    let raw = raw.trim();
    if raw == "true" {
        return Some(SearchAttributeLiteral::Bool(true));
    }
    if raw == "false" {
        return Some(SearchAttributeLiteral::Bool(false));
    }
    if let Some(inner) = raw.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        if inner.contains('\\') {
            // Slice 2 keeps the string-literal lexer minimal (no
            // escapes); fall through so the caller surfaces the
            // standard unsupported-field diagnostic.
            return None;
        }
        return Some(SearchAttributeLiteral::String(inner.to_string()));
    }
    if let Ok(n) = raw.parse::<i64>() {
        return Some(SearchAttributeLiteral::Int(n));
    }
    // R7 slice 3a: `this.<field>` references resolve against the
    // workflow's input message. Only singular `string` fields graduate
    // — int / bool / repeated land in slice 3b. Anything else falls
    // through and the caller surfaces the standard
    // unsupported-`search_attributes` diagnostic.
    if let Some(field_token) = raw.strip_prefix("this.") {
        let field_token = field_token.trim();
        if field_token.is_empty()
            || !field_token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return None;
        }
        let rust_field = field_token.to_snake_case();
        let descriptor = input.fields().find(|f| f.name() == rust_field)?;
        if descriptor.is_list() || descriptor.is_map() {
            return None;
        }
        return match descriptor.kind() {
            prost_reflect::Kind::String => Some(SearchAttributeLiteral::StringField(rust_field)),
            prost_reflect::Kind::Int64 => Some(SearchAttributeLiteral::IntField(rust_field)),
            prost_reflect::Kind::Bool => Some(SearchAttributeLiteral::BoolField(rust_field)),
            _ => None,
        };
    }
    None
}

fn activity_options_spec_from_proto(
    opts: &ActivityOptions,
) -> Option<crate::model::ActivityOptionsSpec> {
    let task_queue = (!opts.task_queue.is_empty()).then(|| opts.task_queue.clone());
    let schedule_to_close_timeout = opts.schedule_to_close_timeout.and_then(duration_from_proto);
    let schedule_to_start_timeout = opts.schedule_to_start_timeout.and_then(duration_from_proto);
    let start_to_close_timeout = opts.start_to_close_timeout.and_then(duration_from_proto);
    let heartbeat_timeout = opts.heartbeat_timeout.and_then(duration_from_proto);
    let retry_policy = opts.retry_policy.clone().map(retry_policy_from_proto);
    let wait_for_cancellation = opts.wait_for_cancellation;
    // The SDK's `ActivityOptions` requires `close_timeouts` (a non-Option
    // enum) at construction time, so we can't build a factory unless the
    // proto declares at least one of the close-timeout variants. If only
    // `task_queue` / `heartbeat_timeout` / `retry_policy` are declared
    // without a close timeout, fall through to no factory — the caller
    // can still build ActivityOptions by hand.
    if start_to_close_timeout.is_none() && schedule_to_close_timeout.is_none() {
        return None;
    }
    Some(crate::model::ActivityOptionsSpec {
        task_queue,
        schedule_to_close_timeout,
        schedule_to_start_timeout,
        start_to_close_timeout,
        heartbeat_timeout,
        retry_policy,
        wait_for_cancellation,
    })
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

fn parent_close_policy_from_proto(raw: i32) -> Option<ParentClosePolicyKind> {
    match ProtoParentClosePolicy::try_from(raw).ok()? {
        ProtoParentClosePolicy::Unspecified => None,
        ProtoParentClosePolicy::Terminate => Some(ParentClosePolicyKind::Terminate),
        ProtoParentClosePolicy::Abandon => Some(ParentClosePolicyKind::Abandon),
        ProtoParentClosePolicy::RequestCancel => Some(ParentClosePolicyKind::RequestCancel),
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
