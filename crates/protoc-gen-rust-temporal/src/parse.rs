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

use anyhow::{Result, anyhow};
use prost::Message;
use prost_reflect::{
    DescriptorPool, DynamicMessage, ExtensionDescriptor, MethodDescriptor, ServiceDescriptor, Value,
};

use crate::model::{
    ActivityModel, IdReusePolicy, ProtoType, QueryModel, QueryRef, ServiceModel, SignalModel,
    SignalRef, UpdateModel, UpdateRef, WorkflowModel,
};
use crate::temporal::v1::{
    ActivityOptions, IdReusePolicy as ProtoPolicy, QueryOptions, ServiceOptions, SignalOptions,
    UpdateOptions, WorkflowOptions,
};

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
                workflows.push(workflow_from(&method, *opts, &package, &service_name));
            }
            MethodKind::Signal(opts) => {
                signals.push(signal_from(&method, opts));
            }
            MethodKind::Query(opts) => {
                queries.push(query_from(&method, opts));
            }
            MethodKind::Update(opts) => {
                updates.push(update_from(&method, opts));
            }
            MethodKind::Activity(opts) => {
                activities.push(activity_from(&method, *opts));
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
    Ok((!parsed.task_queue.is_empty()).then_some(parsed.task_queue))
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

    // A single rpc is expected to carry at most one Temporal annotation.
    // First-match wins; validate.rs would reject a method that lands in two
    // buckets, but in practice it cannot — only one extension field number
    // can be set on a given MethodOptions.
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
) -> WorkflowModel {
    let rpc_method = method.name().to_string();
    let registered_name = if opts.name.is_empty() {
        default_registered_name(package, service_name, &rpc_method)
    } else {
        opts.name
    };

    WorkflowModel {
        rpc_method,
        registered_name,
        input_type: ProtoType::new(method.input().full_name()),
        output_type: ProtoType::new(method.output().full_name()),
        task_queue: (!opts.task_queue.is_empty()).then_some(opts.task_queue),
        id_expression: (!opts.id.is_empty()).then_some(opts.id),
        id_reuse_policy: id_reuse_policy_from_proto(opts.id_reuse_policy),
        execution_timeout: opts.execution_timeout.and_then(duration_from_proto),
        run_timeout: opts.run_timeout.and_then(duration_from_proto),
        task_timeout: opts.task_timeout.and_then(duration_from_proto),
        aliases: opts.aliases,
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
    }
}

fn signal_from(method: &MethodDescriptor, opts: SignalOptions) -> SignalModel {
    let rpc_method = method.name().to_string();
    let registered_name = if opts.name.is_empty() {
        rpc_method.clone()
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

fn query_from(method: &MethodDescriptor, opts: QueryOptions) -> QueryModel {
    let rpc_method = method.name().to_string();
    let registered_name = if opts.name.is_empty() {
        rpc_method.clone()
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

fn update_from(method: &MethodDescriptor, opts: UpdateOptions) -> UpdateModel {
    let rpc_method = method.name().to_string();
    let registered_name = if opts.name.is_empty() {
        rpc_method.clone()
    } else {
        opts.name
    };
    UpdateModel {
        rpc_method,
        registered_name,
        input_type: ProtoType::new(method.input().full_name()),
        output_type: ProtoType::new(method.output().full_name()),
        validate: opts.validate,
    }
}

fn activity_from(method: &MethodDescriptor, opts: ActivityOptions) -> ActivityModel {
    let rpc_method = method.name().to_string();
    let registered_name = if opts.name.is_empty() {
        rpc_method.clone()
    } else {
        opts.name
    };
    ActivityModel {
        rpc_method,
        registered_name,
        input_type: ProtoType::new(method.input().full_name()),
        output_type: ProtoType::new(method.output().full_name()),
    }
}

fn default_registered_name(package: &str, service: &str, rpc: &str) -> String {
    if package.is_empty() {
        format!("{service}/{rpc}")
    } else {
        format!("{package}.{service}/{rpc}")
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

fn duration_from_proto(d: prost_types::Duration) -> Option<Duration> {
    if d.seconds < 0 || d.nanos < 0 {
        return None;
    }
    let secs = u64::try_from(d.seconds).ok()?;
    let nanos = u32::try_from(d.nanos).ok()?;
    Some(Duration::new(secs, nanos))
}
