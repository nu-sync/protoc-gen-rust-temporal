//! Compile generated fixture output against a tiny `temporal_runtime` facade.
//!
//! Golden tests catch textual drift, but they can still miss generated code
//! that no longer type-checks against the documented runtime surface. This
//! test renders representative fixtures into a temporary crate, adds minimal
//! prost-like message structs plus a stub runtime facade, and runs `cargo
//! check`. It deliberately does not start Temporal.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use heck::ToSnakeCase;
use prost_reflect::{DescriptorPool, FieldDescriptor, Kind, MessageDescriptor};
use protoc_gen_rust_temporal::{parse, render, validate};

const ANNOTATIONS_DIR: &str = "proto";

fn crate_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_path(name: &str) -> PathBuf {
    crate_root().join("tests").join("fixtures").join(name)
}

fn protoc_binary() -> PathBuf {
    std::env::var_os("PROTOC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("protoc"))
}

#[test]
fn generated_surfaces_compile_against_stub_runtime() {
    let cases = [
        // Full default client surface: signal/query/update, with-start,
        // activity validation, id templates, and timeout defaults.
        "full_workflow",
        // Empty workflow input/output start + result helpers.
        "empty_input_workflow",
        // Empty query/update outputs and update-with-start unit output.
        "empty_output_query_update",
        // activities=true + workflows=true worker registration surface.
        "worker_full",
        // cli=true clap derive surface.
        "cli_emit",
    ];

    let tmp = tempfile::tempdir().expect("tempdir");
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir).expect("mkdir src");
    fs::write(
        tmp.path().join("Cargo.toml"),
        r#"
[package]
name = "generated-surface-check"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive", "env"] }
"#,
    )
    .expect("write Cargo.toml");

    let mut source = String::new();
    source.push_str("#![allow(dead_code, unused_imports, clippy::all)]\n\n");
    source.push_str(RUNTIME_STUB);

    for case in cases {
        let (pool, files_to_generate) = compile_fixture(case);
        let options = load_fixture_options(case);
        source.push_str(&format!("\n// ---- fixture: {case} ----\n"));
        source.push_str(&render_proto_stubs(&pool, &files_to_generate));

        let services = parse::parse(&pool, &files_to_generate).expect("parse");
        for service in &services {
            validate::validate(service, &options).expect("validate");
            source.push_str(&render::render(service, &options));
        }
    }

    fs::write(src_dir.join("lib.rs"), source).expect("write lib.rs");

    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .arg("check")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(tmp.path().join("Cargo.toml"))
        .output()
        .expect("run cargo check");

    assert!(
        output.status.success(),
        "generated surface temp crate failed to compile\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn compile_fixture(name: &str) -> (DescriptorPool, HashSet<String>) {
    let fixture_dir = fixture_path(name);
    let annotations = crate_root().join(ANNOTATIONS_DIR);
    let tmp = tempfile::tempdir().expect("tempdir");
    let fds_path = tmp.path().join("out.fds");

    let status = Command::new(protoc_binary())
        .arg(format!("-I{}", fixture_dir.display()))
        .arg(format!("-I{}", annotations.display()))
        .arg(format!("--descriptor_set_out={}", fds_path.display()))
        .arg("--include_imports")
        .arg("input.proto")
        .status()
        .expect("invoke protoc");
    assert!(status.success(), "protoc failed for {name}: {status}");

    let bytes = fs::read(&fds_path).expect("read fds");
    let mut pool = DescriptorPool::new();
    pool.decode_file_descriptor_set(bytes.as_slice())
        .expect("decode fds");
    let files_to_generate = std::iter::once("input.proto".to_string()).collect();
    (pool, files_to_generate)
}

fn load_fixture_options(name: &str) -> protoc_gen_rust_temporal::options::RenderOptions {
    let path = fixture_path(name).join("options.txt");
    if !path.exists() {
        return Default::default();
    }
    let raw =
        fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    protoc_gen_rust_temporal::options::parse_options(raw.trim())
        .unwrap_or_else(|err| panic!("parse {}: {err}", path.display()))
}

fn render_proto_stubs(pool: &DescriptorPool, files_to_generate: &HashSet<String>) -> String {
    let mut by_package: BTreeMap<String, Vec<MessageDescriptor>> = BTreeMap::new();
    for file_name in files_to_generate {
        let file = pool
            .get_file_by_name(file_name)
            .unwrap_or_else(|| panic!("descriptor file {file_name} not found"));
        let package = file
            .file_descriptor_proto()
            .package
            .clone()
            .unwrap_or_default();
        by_package
            .entry(package)
            .or_default()
            .extend(file.messages().filter(|m| !m.is_map_entry()));
    }

    let mut out = String::new();
    for (package, messages) in by_package {
        open_package_modules(&mut out, &package);
        for message in messages {
            render_message_stub(&mut out, &package, &message);
        }
        close_package_modules(&mut out, &package);
    }
    out
}

fn open_package_modules(out: &mut String, package: &str) {
    for segment in package.split('.').filter(|s| !s.is_empty()) {
        let _ = writeln!(out, "pub mod {segment} {{");
    }
}

fn close_package_modules(out: &mut String, package: &str) {
    for _ in package.split('.').filter(|s| !s.is_empty()) {
        let _ = writeln!(out, "}}");
    }
}

fn render_message_stub(out: &mut String, package: &str, message: &MessageDescriptor) {
    let _ = writeln!(
        out,
        "#[derive(Clone, Debug, Default, PartialEq)]\npub struct {} {{",
        message.name()
    );
    for field in message.fields() {
        if field.containing_oneof().is_some() {
            continue;
        }
        let rust_name = field.name().to_snake_case();
        let rust_type = rust_type_for_field(package, &field);
        let _ = writeln!(out, "    pub {rust_name}: {rust_type},");
    }
    let _ = writeln!(out, "}}\n");
}

fn rust_type_for_field(current_package: &str, field: &FieldDescriptor) -> String {
    if field.is_map() {
        let Kind::Message(entry) = field.kind() else {
            unreachable!("map fields are represented as message entries")
        };
        return format!(
            "::std::collections::BTreeMap<{}, {}>",
            singular_type_for_field(current_package, &entry.map_entry_key_field()),
            singular_type_for_field(current_package, &entry.map_entry_value_field()),
        );
    }

    if field.is_list() {
        return format!("Vec<{}>", singular_type_for_field(current_package, field));
    }

    let ty = singular_type_for_field(current_package, field);
    if matches!(field.kind(), Kind::Message(_)) && field.supports_presence() {
        format!("Option<{ty}>")
    } else {
        ty
    }
}

fn singular_type_for_field(current_package: &str, field: &FieldDescriptor) -> String {
    match field.kind() {
        Kind::Double => "f64".to_string(),
        Kind::Float => "f32".to_string(),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => "i32".to_string(),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => "i64".to_string(),
        Kind::Uint32 | Kind::Fixed32 => "u32".to_string(),
        Kind::Uint64 | Kind::Fixed64 => "u64".to_string(),
        Kind::Bool => "bool".to_string(),
        Kind::String => "String".to_string(),
        Kind::Bytes => "Vec<u8>".to_string(),
        Kind::Enum(_) => "i32".to_string(),
        Kind::Message(message) => message_type_path(current_package, &message),
    }
}

fn message_type_path(current_package: &str, message: &MessageDescriptor) -> String {
    if message.full_name() == "google.protobuf.Empty" {
        return "()".to_string();
    }
    if message.package_name() == current_package {
        return message.name().to_string();
    }

    let mut path = String::from("crate");
    for segment in message.package_name().split('.').filter(|s| !s.is_empty()) {
        path.push_str("::");
        path.push_str(segment);
    }
    path.push_str("::");
    path.push_str(message.name());
    path
}

const RUNTIME_STUB: &str = r#"
pub mod temporal_runtime {
    pub use clap;

    #[derive(Clone, Debug, Default)]
    pub struct TemporalClient;

    impl TemporalClient {
        pub fn namespace(&self) -> String {
            "stub-namespace".to_string()
        }
    }

    #[derive(Clone, Debug, Default)]
    pub struct WorkflowHandle;

    impl WorkflowHandle {
        pub fn workflow_id(&self) -> &str {
            "stub-workflow-id"
        }
        pub fn run_id(&self) -> Option<&str> {
            None
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum WorkflowIdReusePolicy {
        AllowDuplicate,
        AllowDuplicateFailedOnly,
        RejectDuplicate,
        TerminateIfRunning,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum WorkflowIdConflictPolicy {
        Fail,
        UseExisting,
        TerminateExisting,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct RetryPolicy {
        pub initial_interval: Option<std::time::Duration>,
        pub max_interval: Option<std::time::Duration>,
        pub max_attempts: i32,
        pub non_retryable_error_types: Vec<String>,
    }
    impl RetryPolicy {
        pub fn new() -> Self { Self::default() }
        pub fn set_backoff_coefficient(&mut self, _value: f64) {}
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum WaitPolicy {
        Admitted,
        Accepted,
        Completed,
    }

    pub trait TemporalProtoMessage {
        const MESSAGE_TYPE: &'static str;
    }

    /// Stub of `temporal-proto-runtime`'s `TypedProtoMessage<T>` so the
    /// generated `ActivityDefinition::{Input, Output}` types resolve.
    pub struct TypedProtoMessage<T: TemporalProtoMessage>(pub T);
    impl<T: TemporalProtoMessage> From<T> for TypedProtoMessage<T> {
        fn from(t: T) -> Self {
            Self(t)
        }
    }
    impl<T: TemporalProtoMessage + Default> Default for TypedProtoMessage<T> {
        fn default() -> Self {
            Self(T::default())
        }
    }
    impl<T: TemporalProtoMessage> TypedProtoMessage<T> {
        pub fn into_inner(self) -> T {
            self.0
        }
    }

    /// Stub of `temporal_runtime::ProtoEmpty` — the wire-format marker for
    /// `google.protobuf.Empty`. Generated code spells it into marker
    /// Input/Output associated types when the proto declares Empty.
    #[derive(Default)]
    pub struct ProtoEmpty {}
    impl TemporalProtoMessage for ProtoEmpty {
        const MESSAGE_TYPE: &'static str = "google.protobuf.Empty";
    }

    pub type ActivityContext = ();

    pub fn random_workflow_id() -> String {
        "stub-random-workflow-id".to_string()
    }

    pub fn attach_handle(_client: &TemporalClient, _workflow_id: String) -> WorkflowHandle {
        WorkflowHandle
    }

    /// Stub Payload — generated surface only needs the type to exist.
    /// Bridge surfaces this as `ProtoPayload` (a real
    /// `temporalio-common` re-export); the stub names them differently
    /// to mirror the visibility distinction.
    pub struct ProtoPayload;
    pub use ProtoPayload as Payload;

    pub fn encode_search_attribute_string(_value: &str) -> ProtoPayload {
        ProtoPayload
    }
    pub fn encode_search_attribute_int(_value: i64) -> ProtoPayload {
        ProtoPayload
    }
    pub fn encode_search_attribute_bool(_value: bool) -> ProtoPayload {
        ProtoPayload
    }

    pub async fn connect(_url: &str, _namespace: &str) -> anyhow::Result<TemporalClient> {
        Ok(TemporalClient)
    }

    pub async fn start_workflow_proto<I: TemporalProtoMessage>(
        _client: &TemporalClient,
        _workflow_name: &'static str,
        _workflow_id: &str,
        _task_queue: &str,
        _input: &I,
        _id_reuse_policy: Option<WorkflowIdReusePolicy>,
        _id_conflict_policy: Option<WorkflowIdConflictPolicy>,
        _execution_timeout: Option<std::time::Duration>,
        _run_timeout: Option<std::time::Duration>,
        _task_timeout: Option<std::time::Duration>,
        _enable_eager_workflow_start: bool,
        _retry_policy: Option<RetryPolicy>,
        _search_attributes: Option<::std::collections::HashMap<String, ProtoPayload>>,
    ) -> anyhow::Result<WorkflowHandle> {
        Ok(WorkflowHandle)
    }

    pub async fn start_workflow_proto_empty(
        _client: &TemporalClient,
        _workflow_name: &'static str,
        _workflow_id: &str,
        _task_queue: &str,
        _id_reuse_policy: Option<WorkflowIdReusePolicy>,
        _id_conflict_policy: Option<WorkflowIdConflictPolicy>,
        _execution_timeout: Option<std::time::Duration>,
        _run_timeout: Option<std::time::Duration>,
        _task_timeout: Option<std::time::Duration>,
        _enable_eager_workflow_start: bool,
        _retry_policy: Option<RetryPolicy>,
        _search_attributes: Option<::std::collections::HashMap<String, ProtoPayload>>,
    ) -> anyhow::Result<WorkflowHandle> {
        Ok(WorkflowHandle)
    }

    pub async fn wait_result_proto<O: TemporalProtoMessage + Default>(
        _handle: &WorkflowHandle,
    ) -> anyhow::Result<O> {
        Ok(O::default())
    }

    pub async fn wait_result_unit(_handle: &WorkflowHandle) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn cancel_workflow(_handle: &WorkflowHandle, _reason: &str) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn terminate_workflow(_handle: &WorkflowHandle, _reason: &str) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn signal_proto<I: TemporalProtoMessage>(
        _handle: &WorkflowHandle,
        _name: &str,
        _input: &I,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn signal_unit(_handle: &WorkflowHandle, _name: &str) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn query_proto<I: TemporalProtoMessage, O: TemporalProtoMessage + Default>(
        _handle: &WorkflowHandle,
        _name: &str,
        _input: &I,
    ) -> anyhow::Result<O> {
        Ok(O::default())
    }

    pub async fn query_proto_empty<O: TemporalProtoMessage + Default>(
        _handle: &WorkflowHandle,
        _name: &str,
    ) -> anyhow::Result<O> {
        Ok(O::default())
    }

    pub async fn query_unit<I: TemporalProtoMessage>(
        _handle: &WorkflowHandle,
        _name: &str,
        _input: &I,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn query_proto_empty_unit(
        _handle: &WorkflowHandle,
        _name: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn update_proto<I: TemporalProtoMessage, O: TemporalProtoMessage + Default>(
        _handle: &WorkflowHandle,
        _name: &str,
        _input: &I,
        _wait_policy: WaitPolicy,
    ) -> anyhow::Result<O> {
        Ok(O::default())
    }

    pub async fn update_proto_empty<O: TemporalProtoMessage + Default>(
        _handle: &WorkflowHandle,
        _name: &str,
        _wait_policy: WaitPolicy,
    ) -> anyhow::Result<O> {
        Ok(O::default())
    }

    pub async fn update_unit<I: TemporalProtoMessage>(
        _handle: &WorkflowHandle,
        _name: &str,
        _input: &I,
        _wait_policy: WaitPolicy,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn update_proto_empty_unit(
        _handle: &WorkflowHandle,
        _name: &str,
        _wait_policy: WaitPolicy,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn signal_with_start_workflow_proto<W: TemporalProtoMessage, S: TemporalProtoMessage>(
        _client: &TemporalClient,
        _workflow_name: &'static str,
        _workflow_id: &str,
        _task_queue: &str,
        _workflow_input: &W,
        _signal_name: &str,
        _signal_input: &S,
        _id_reuse_policy: Option<WorkflowIdReusePolicy>,
        _execution_timeout: Option<std::time::Duration>,
        _run_timeout: Option<std::time::Duration>,
        _task_timeout: Option<std::time::Duration>,
    ) -> anyhow::Result<WorkflowHandle> {
        Ok(WorkflowHandle)
    }

    pub async fn update_with_start_workflow_proto<
        W: TemporalProtoMessage,
        U: TemporalProtoMessage,
        O: TemporalProtoMessage + Default,
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
        _execution_timeout: Option<std::time::Duration>,
        _run_timeout: Option<std::time::Duration>,
        _task_timeout: Option<std::time::Duration>,
        _id_conflict_policy: Option<WorkflowIdConflictPolicy>,
    ) -> anyhow::Result<(WorkflowHandle, O)> {
        Ok((WorkflowHandle, O::default()))
    }

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
        _execution_timeout: Option<std::time::Duration>,
        _run_timeout: Option<std::time::Duration>,
        _task_timeout: Option<std::time::Duration>,
        _id_conflict_policy: Option<WorkflowIdConflictPolicy>,
    ) -> anyhow::Result<WorkflowHandle> {
        Ok(WorkflowHandle)
    }

    pub mod worker {
        #[derive(Debug, Default)]
        pub struct Worker;

        pub trait ActivityImplementer {}
        pub trait WorkflowImplementer {}

        /// Stub of the SDK's ActivityDefinition trait so the generated
        /// per-activity marker structs + impls type-check against the
        /// stub runtime.
        pub trait ActivityDefinition {
            type Input;
            type Output;
            fn name() -> &'static str
            where
                Self: Sized;
        }

        pub trait WorkflowDefinition {
            type Input;
            type Output;
            fn name(&self) -> &str;
        }

        pub trait WorkflowImplementation: Sized + 'static {
            type Run: WorkflowDefinition;
        }

        pub trait SignalDefinition {
            type Workflow: WorkflowDefinition;
            type Input;
            fn name(&self) -> &str;
        }

        #[derive(Debug, Clone, Copy)]
        pub struct SignalExternalOk;
        pub type SignalExternalWfResult = ::std::result::Result<SignalExternalOk, &'static str>;

        pub struct ExternalWorkflowHandle;
        impl ExternalWorkflowHandle {
            pub async fn signal<S: SignalDefinition>(
                &self,
                _signal: S,
                _input: S::Input,
            ) -> SignalExternalWfResult {
                Ok(SignalExternalOk)
            }
        }

        #[derive(Debug, Clone, Copy)]
        pub enum ParentClosePolicy {
            Terminate,
            Abandon,
            RequestCancel,
        }
        impl From<ParentClosePolicy> for i32 {
            fn from(p: ParentClosePolicy) -> i32 {
                p as i32
            }
        }

        #[derive(Debug, Clone, Copy, Default)]
        pub enum ChildWorkflowCancellationType {
            #[default]
            Abandon,
            TryCancel,
            WaitCancellationCompleted,
            WaitCancellationRequested,
        }

        #[derive(Debug, Default)]
        pub struct ChildWorkflowOptions {
            pub parent_close_policy: i32,
            pub cancel_type: ChildWorkflowCancellationType,
        }

        #[derive(Debug)]
        pub struct ChildWorkflowStartError;

        #[derive(Debug)]
        pub struct StartedChildWorkflow<WD: WorkflowDefinition>(::std::marker::PhantomData<WD>);

        #[derive(Debug, Default)]
        pub struct ContinueAsNewOptions;

        #[derive(Debug)]
        pub enum WorkflowTermination {}

        #[derive(Debug, Clone, Copy)]
        pub enum ActivityCancellationType {
            TryCancel,
            WaitCancellationCompleted,
            Abandon,
        }

        #[derive(Debug, Clone, Copy)]
        pub enum ActivityCloseTimeouts {
            ScheduleToClose(std::time::Duration),
            StartToClose(std::time::Duration),
            Both {
                start_to_close: std::time::Duration,
                schedule_to_close: std::time::Duration,
            },
        }

        // Stub builder enough to support the per-activity factory emit:
        // takes a close-timeout kicker via `with_*` constructors, then
        // chains optional setters and finishes with `.build()`.
        #[derive(Debug)]
        pub struct ActivityOptionsBuilder;
        impl ActivityOptionsBuilder {
            pub fn task_queue<S: Into<String>>(self, _v: S) -> Self {
                self
            }
            pub fn schedule_to_start_timeout(self, _d: std::time::Duration) -> Self {
                self
            }
            pub fn heartbeat_timeout(self, _d: std::time::Duration) -> Self {
                self
            }
            pub fn retry_policy(self, _rp: super::RetryPolicy) -> Self {
                self
            }
            pub fn cancellation_type(self, _t: ActivityCancellationType) -> Self {
                self
            }
            pub fn build(self) -> ActivityOptions {
                ActivityOptions
            }
        }
        #[derive(Debug, Default)]
        pub struct ActivityOptions;
        impl ActivityOptions {
            pub fn with_close_timeouts(_t: ActivityCloseTimeouts) -> ActivityOptionsBuilder {
                ActivityOptionsBuilder
            }
            pub fn with_start_to_close_timeout(_d: std::time::Duration) -> ActivityOptionsBuilder {
                ActivityOptionsBuilder
            }
            pub fn with_schedule_to_close_timeout(
                _d: std::time::Duration,
            ) -> ActivityOptionsBuilder {
                ActivityOptionsBuilder
            }
        }

        #[derive(Debug, Default)]
        pub struct LocalActivityOptions;

        #[derive(Debug)]
        pub struct ActivityExecutionError;

        #[derive(Debug)]
        pub struct WorkflowContext<W>(::std::marker::PhantomData<W>);
        impl<W> WorkflowContext<W> {
            pub async fn start_activity<AD: ActivityDefinition>(
                &self,
                _activity: AD,
                _input: impl Into<AD::Input>,
                _opts: ActivityOptions,
            ) -> ::std::result::Result<AD::Output, ActivityExecutionError>
            where
                AD::Output: Default,
            {
                Ok(AD::Output::default())
            }
            pub async fn start_local_activity<AD: ActivityDefinition>(
                &self,
                _activity: AD,
                _input: impl Into<AD::Input>,
                _opts: LocalActivityOptions,
            ) -> ::std::result::Result<AD::Output, ActivityExecutionError>
            where
                AD::Output: Default,
            {
                Ok(AD::Output::default())
            }
            pub async fn child_workflow<WD: WorkflowDefinition>(
                &self,
                _wf: WD,
                _input: impl Into<WD::Input>,
                _opts: ChildWorkflowOptions,
            ) -> ::std::result::Result<StartedChildWorkflow<WD>, ChildWorkflowStartError> {
                Ok(StartedChildWorkflow(::std::marker::PhantomData))
            }
            pub fn continue_as_new(
                &self,
                _input: &<<W as WorkflowImplementation>::Run as WorkflowDefinition>::Input,
                _opts: ContinueAsNewOptions,
            ) -> ::std::result::Result<::std::convert::Infallible, WorkflowTermination>
            where
                W: WorkflowImplementation,
            {
                unreachable!("stub")
            }
            pub fn external_workflow(
                &self,
                _workflow_id: impl Into<String>,
                _run_id: Option<String>,
            ) -> ExternalWorkflowHandle {
                ExternalWorkflowHandle
            }
        }

        impl Worker {
            pub fn register_activities<I>(&mut self, _impl: I) -> &mut Self
            where
                I: ActivityImplementer,
            {
                self
            }

            pub fn register_workflow<W>(&mut self) -> &mut Self
            where
                W: WorkflowImplementer,
            {
                self
            }
        }
    }
}
"#;
