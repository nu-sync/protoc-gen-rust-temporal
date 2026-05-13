//! Integration tests for the `parse + validate` pipeline.
//!
//! Each test invokes the real `protoc` binary (the same one `prost-build`
//! uses at workspace build time) against a fixture `.proto`, then feeds the
//! resulting `FileDescriptorSet` into a `DescriptorPool` and runs the
//! plugin's parse + validate stages. That mirrors what the plugin sees in
//! production when `protoc` invokes it as a child process.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use prost_reflect::DescriptorPool;

use protoc_gen_rust_temporal::model::ServiceModel;
use protoc_gen_rust_temporal::{parse, render, validate};

const ANNOTATIONS_DIR: &str = "proto";

fn fixture_path(name: &str) -> PathBuf {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_root.join("tests").join("fixtures").join(name)
}

fn protoc_binary() -> PathBuf {
    if let Ok(p) = std::env::var("PROTOC") {
        return PathBuf::from(p);
    }
    PathBuf::from("protoc")
}

/// Compile `proto_root/input.proto` with cludden's schema reachable on the
/// import path, returning a descriptor pool plus the
/// `files_to_generate` set.
fn compile_fixture_at(proto_root: &Path, file: &str) -> (DescriptorPool, HashSet<String>) {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let annotations = crate_root.join(ANNOTATIONS_DIR);

    let tmp = tempfile::tempdir().expect("tempdir");
    let fds_path = tmp.path().join("out.fds");

    let status = Command::new(protoc_binary())
        .arg(format!("-I{}", proto_root.display()))
        .arg(format!("-I{}", annotations.display()))
        .arg(format!("--descriptor_set_out={}", fds_path.display()))
        .arg("--include_imports")
        .arg(file)
        .status()
        .expect("invoke protoc — install protoc or set $PROTOC");
    assert!(status.success(), "protoc failed: {status}");

    let bytes = std::fs::read(&fds_path).expect("read fds");
    let mut pool = DescriptorPool::new();
    pool.decode_file_descriptor_set(bytes.as_slice())
        .expect("decode_file_descriptor_set");

    let files_to_generate: HashSet<String> = std::iter::once(file.to_string()).collect();
    (pool, files_to_generate)
}

fn compile_fixture(name: &str) -> (DescriptorPool, HashSet<String>) {
    compile_fixture_at(&fixture_path(name), "input.proto")
}

/// Drop `source` into a temp dir as `input.proto` and run `protoc` against
/// it. Used by the negative tests.
fn compile_fixture_inline(source: &str) -> (DescriptorPool, HashSet<String>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(tmp.path().join("input.proto"), source).expect("write input.proto");
    let (pool, files_to_generate) = compile_fixture_at(tmp.path(), "input.proto");
    (pool, files_to_generate, tmp)
}

fn parse_and_validate(name: &str) -> Vec<ServiceModel> {
    let (pool, files_to_generate) = compile_fixture(name);
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let options = protoc_gen_rust_temporal::options::RenderOptions::default();
    for service in &services {
        validate::validate(service, &options).expect("validate");
    }
    services
}

fn load_fixture_options(name: &str) -> protoc_gen_rust_temporal::options::RenderOptions {
    let p = fixture_path(name).join("options.txt");
    if !p.exists() {
        return protoc_gen_rust_temporal::options::RenderOptions::default();
    }
    let s = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    protoc_gen_rust_temporal::options::parse_options(s.trim())
        .unwrap_or_else(|e| panic!("parse {}: {e}", p.display()))
}

#[test]
fn minimal_workflow_parses_and_validates() {
    let services = parse_and_validate("minimal_workflow");
    assert_eq!(services.len(), 1);
    let svc = &services[0];

    assert_eq!(svc.package, "jobs.v1");
    assert_eq!(svc.service, "JobService");
    assert_eq!(svc.default_task_queue.as_deref(), Some("jobs"));

    assert_eq!(svc.workflows.len(), 1);
    let wf = &svc.workflows[0];
    assert_eq!(wf.rpc_method, "RunJob");
    assert_eq!(wf.registered_name, "jobs.v1.JobService.RunJob");
    assert_eq!(wf.input_type.full_name, "jobs.v1.JobInput");
    assert_eq!(wf.output_type.full_name, "jobs.v1.JobOutput");
    {
        use protoc_gen_rust_temporal::model::IdTemplateSegment;
        let segments = wf.id_expression.as_deref().expect("id template parsed");
        assert_eq!(
            segments,
            &[IdTemplateSegment::Field("name".to_string())],
            "minimal_workflow's `{{{{ .Name }}}}` template should compile to a single Field segment"
        );
    }
    assert!(wf.id_reuse_policy.is_none());

    assert_eq!(wf.attached_signals.len(), 1);
    assert_eq!(wf.attached_signals[0].rpc_method, "CancelJob");
    assert!(wf.attached_signals[0].start);

    assert_eq!(wf.attached_queries.len(), 1);
    assert_eq!(wf.attached_queries[0].rpc_method, "GetStatus");

    assert_eq!(wf.attached_updates.len(), 1);
    assert_eq!(wf.attached_updates[0].rpc_method, "Reconfigure");

    assert_eq!(svc.signals.len(), 1);
    assert_eq!(svc.signals[0].rpc_method, "CancelJob");
    assert!(svc.signals[0].output_type.is_empty);

    assert_eq!(svc.queries.len(), 1);
    assert_eq!(svc.queries[0].rpc_method, "GetStatus");
    assert!(svc.queries[0].input_type.is_empty);

    assert_eq!(svc.updates.len(), 1);
    assert_eq!(svc.updates[0].rpc_method, "Reconfigure");
    assert!(svc.updates[0].validate);

    assert_eq!(svc.activities.len(), 1);
    assert_eq!(svc.activities[0].rpc_method, "ProcessChunk");
}

// Regression guard for cross-language interop with cludden's Go plugin.
// Default registration name must be the fully-qualified proto method name
// (`<package>.<Service>.<Rpc>`), matching the Go plugin's
// `string(method.Desc.FullName())` default — not the bare rpc name and
// *not* the `<package>.<Service>/<Rpc>` slash form. Mixed-language workers
// where one side has an explicit `name:` and the other relies on the
// default would silently never connect otherwise.
#[test]
fn default_registration_names_match_go_full_name() {
    let services = parse_and_validate("minimal_workflow");
    let svc = &services[0];
    assert_eq!(
        svc.workflows[0].registered_name,
        "jobs.v1.JobService.RunJob"
    );
    assert_eq!(
        svc.signals[0].registered_name,
        "jobs.v1.JobService.CancelJob"
    );
    assert_eq!(
        svc.queries[0].registered_name,
        "jobs.v1.JobService.GetStatus"
    );
    assert_eq!(
        svc.updates[0].registered_name,
        "jobs.v1.JobService.Reconfigure"
    );
    assert_eq!(
        svc.activities[0].registered_name,
        "jobs.v1.JobService.ProcessChunk"
    );
}

#[test]
fn explicit_name_overrides_default() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package ex.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              name: "custom.workflow.name"
              signal: [{ ref: "Cancel" }]
            };
          }
          rpc Cancel(In) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = { name: "custom.signal.name" };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let services = protoc_gen_rust_temporal::parse::parse(&pool, &files_to_generate)
        .expect("parse should succeed");
    let svc = &services[0];
    assert_eq!(svc.workflows[0].registered_name, "custom.workflow.name");
    assert_eq!(svc.signals[0].registered_name, "custom.signal.name");
}

#[test]
fn workflow_with_bad_signal_ref_fails_validation() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "NoSuchSignal" }]
            };
          }
        }
        message In {}
        message Out {}
        "#,
    );

    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let err = validate::validate(&services[0], &Default::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("NoSuchSignal"),
        "validation error should name the missing ref, got: {err}"
    );
}

#[test]
fn workflow_signal_ref_with_xns_is_rejected_at_parse() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Tick", xns: { task_queue: "other" } }]
            };
          }
          rpc Tick(In) returns (In) {
            option (temporal.v1.signal) = {};
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .expect_err("xns on signal ref must be rejected at parse")
        .to_string();
    assert!(
        err.contains("xns") && err.contains("signal[ref=Tick]"),
        "parse error must surface xns + signal ref name, got: {err}"
    );
}

#[test]
fn workflow_update_ref_with_cli_is_rejected_at_parse() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Touch", cli: {} }]
            };
          }
          rpc Touch(In) returns (Out) {
            option (temporal.v1.update) = {};
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .expect_err("cli on update ref must be rejected at parse")
        .to_string();
    assert!(
        err.contains("cli") && err.contains("update[ref=Touch]"),
        "parse error must surface cli + update ref name, got: {err}"
    );
}

#[test]
fn workflow_query_ref_with_xns_is_rejected_at_parse() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              query: [{ ref: "Status", xns: { task_queue: "other" } }]
            };
          }
          rpc Status(In) returns (Out) {
            option (temporal.v1.query) = {};
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .expect_err("xns on query ref must be rejected at parse")
        .to_string();
    assert!(
        err.contains("xns") && err.contains("query[ref=Status]"),
        "parse error must surface xns + query ref name, got: {err}"
    );
}

#[test]
fn workflow_without_task_queue_fails_validation() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {};
          }
        }
        message In {}
        message Out {}
        "#,
    );

    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let err = validate::validate(&services[0], &Default::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("task_queue"),
        "validation error should mention task_queue, got: {err}"
    );
}

/// Byte-for-byte golden test for the `minimal_workflow` fixture. Run with
/// `BLESS=1 cargo test --workspace --test parse_validate
/// minimal_workflow_render_golden` to rebless after intentional render
/// changes; the test will write `expected.rs` in place and pass.
#[test]
fn minimal_workflow_render_golden() {
    assert_golden("minimal_workflow");
}

#[test]
fn workflow_only_render_golden() {
    assert_golden("workflow_only");
}

#[test]
fn multiple_workflows_render_golden() {
    assert_golden("multiple_workflows");
}

#[test]
fn full_workflow_render_golden() {
    assert_golden("full_workflow");
}

#[test]
fn empty_input_workflow_render_golden() {
    assert_golden("empty_input_workflow");
}

#[test]
fn empty_output_query_update_render_golden() {
    assert_golden("empty_output_query_update");
}

/// Sanity: confirm the new empty-output fixture parses and validates
/// cleanly — so any future render-time breakage (e.g. dropping the
/// `_unit` dispatch in `render_query_method`) shows up here.
#[test]
fn empty_output_query_update_parses_and_validates() {
    let services = parse_and_validate("empty_output_query_update");
    assert_eq!(services.len(), 1);
    let svc = &services[0];
    assert_eq!(svc.queries.len(), 2);
    assert_eq!(svc.updates.len(), 3);
    for q in &svc.queries {
        assert!(
            q.output_type.is_empty,
            "fixture invariant: every query must have Empty output (got {})",
            q.output_type.full_name
        );
    }
    for u in &svc.updates {
        assert!(
            u.output_type.is_empty,
            "fixture invariant: every update must have Empty output (got {})",
            u.output_type.full_name
        );
    }
}

#[test]
fn activity_only_render_golden() {
    assert_golden("activity_only");
}

#[test]
fn activities_emit_render_golden() {
    assert_golden("activities_emit");
}

#[test]
fn activities_emit_renders_trait_and_consts() {
    let services = parse_and_validate("activities_emit");
    let opts = load_fixture_options("activities_emit");
    assert!(
        opts.activities,
        "fixture options.txt should enable activities"
    );
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub trait ChunkServiceActivities: Send + Sync + 'static"),
        "missing activities trait declaration: {source}"
    );
    assert!(
        source.contains("pub const PROCESS_ACTIVITY_NAME"),
        "missing Process name const"
    );
    assert!(
        source.contains("pub const HEARTBEAT_ACTIVITY_NAME"),
        "missing Heartbeat name const"
    );
    assert!(
        source.contains(
            "fn process(&self, ctx: temporal_runtime::ActivityContext, input: ChunkInput)"
        ),
        "Process trait method signature wrong: {source}"
    );
    assert!(
        source.contains("fn heartbeat(&self, ctx: temporal_runtime::ActivityContext, input: ())"),
        "Heartbeat (Empty input) trait method signature wrong: {source}"
    );
}

#[test]
fn activities_emit_off_by_default() {
    let services = parse_and_validate("activities_emit");
    // No options.txt-driven flag, no activities trait.
    let source = render::render(&services[0], &Default::default());
    assert!(!source.contains("pub trait ChunkServiceActivities"));
    assert!(!source.contains("_ACTIVITY_NAME"));
}

#[test]
fn workflows_emit_render_golden() {
    assert_golden("workflows_emit");
}

#[test]
fn workflows_emit_renders_handler_name_consts() {
    let services = parse_and_validate("workflows_emit");
    let opts = load_fixture_options("workflows_emit");
    assert!(
        opts.workflows,
        "fixture options.txt should enable workflows"
    );
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub const CANCEL_SIGNAL_NAME: &str = \"wf.v1.OrderService.Cancel\";"),
        "missing Cancel signal name const: {source}"
    );
    assert!(
        source.contains("pub const STATUS_QUERY_NAME: &str = \"wf.v1.OrderService.Status\";"),
        "missing Status query name const"
    );
    assert!(
        source.contains("pub const CONFIRM_UPDATE_NAME: &str = \"wf.v1.OrderService.Confirm\";"),
        "missing Confirm update name const"
    );
}

#[test]
fn cli_emit_render_golden() {
    assert_golden("cli_emit");
}

#[test]
fn cli_emit_renders_clap_subcommands() {
    let services = parse_and_validate("cli_emit");
    let opts = load_fixture_options("cli_emit");
    assert!(opts.cli, "fixture options.txt should enable cli");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub mod report_service_cli {"),
        "missing CLI module: {source}"
    );
    assert!(
        source.contains("#[derive(temporal_runtime::clap::Parser)]"),
        "missing Cli derive"
    );
    assert!(
        source.contains("StartGenerate(StartGenerateArgs),"),
        "missing StartGenerate subcommand variant"
    );
    assert!(
        source.contains("AttachAggregate(AttachAggregateArgs),"),
        "missing AttachAggregate subcommand variant"
    );
    assert!(
        source.contains("pub struct StartGenerateArgs {"),
        "missing StartGenerateArgs struct"
    );
}

#[test]
fn cli_emit_off_by_default() {
    let services = parse_and_validate("cli_emit");
    let source = render::render(&services[0], &Default::default());
    assert!(!source.contains("report_service_cli"));
    assert!(!source.contains("clap::Parser"));
}

#[test]
fn workflows_emit_off_by_default() {
    let services = parse_and_validate("workflows_emit");
    let source = render::render(&services[0], &Default::default());
    // Workflow-level consts always emit (existing behavior).
    assert!(source.contains("pub const RUN_WORKFLOW_NAME"));
    // The new per-rpc handler-name consts only emit when workflows=true.
    assert!(!source.contains("CANCEL_SIGNAL_NAME"));
    assert!(!source.contains("STATUS_QUERY_NAME"));
    assert!(!source.contains("CONFIRM_UPDATE_NAME"));
}

#[test]
fn activity_only_emits_no_workflow_surface() {
    let services = parse_and_validate("activity_only");
    let svc = &services[0];
    assert!(svc.workflows.is_empty());
    assert!(svc.signals.is_empty());
    assert!(svc.queries.is_empty());
    assert!(svc.updates.is_empty());
    assert_eq!(svc.activities.len(), 2);

    let source = render::render(svc, &Default::default());
    // No workflow constants, no handle struct, no _with_start free function.
    assert!(!source.contains("_WORKFLOW_NAME"));
    assert!(!source.contains("Handle {"));
    assert!(!source.contains("_with_start("));
    // The client struct still emits — keeps the import surface consistent
    // with services that have a mix of activities and workflows.
    assert!(source.contains("pub struct WorkerOnlyServiceClient"));
}

#[test]
fn multiple_workflows_parses_correctly() {
    let services = parse_and_validate("multiple_workflows");
    let svc = &services[0];
    assert_eq!(svc.workflows.len(), 2);
    assert_eq!(svc.workflows[0].rpc_method, "Alpha");
    assert_eq!(svc.workflows[1].rpc_method, "Beta");
    // Alpha falls back to service-default task_queue, Beta overrides.
    assert_eq!(svc.workflows[0].task_queue, None);
    assert_eq!(svc.workflows[1].task_queue.as_deref(), Some("multi-beta"));
    assert_eq!(svc.default_task_queue.as_deref(), Some("multi"));
}

#[test]
fn full_workflow_emits_both_with_start_paths() {
    let services = parse_and_validate("full_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub async fn bootstrap_with_start("),
        "missing signal-with-start emission"
    );
    assert!(
        source.contains("pub async fn reconfigure_with_start("),
        "missing update-with-start emission"
    );
    // Regular signal (without start: true) must still emit the handle method
    // but NOT a free function.
    assert!(source.contains("pub async fn cancel(&self,"));
    assert!(!source.contains("pub async fn cancel_with_start("));
}

#[test]
fn workflow_only_parses_and_validates() {
    let services = parse_and_validate("workflow_only");
    assert_eq!(services.len(), 1);
    let svc = &services[0];
    assert_eq!(svc.package, "solo.v1");
    assert_eq!(svc.service, "SoloService");
    // No service-level default — falls back to the workflow's own task_queue.
    assert!(svc.default_task_queue.is_none());
    assert_eq!(svc.workflows.len(), 1);
    let wf = &svc.workflows[0];
    assert_eq!(wf.task_queue.as_deref(), Some("solo-tq"));
    assert!(wf.attached_signals.is_empty());
    assert!(wf.attached_queries.is_empty());
    assert!(wf.attached_updates.is_empty());
    assert!(svc.signals.is_empty());
    assert!(svc.queries.is_empty());
    assert!(svc.updates.is_empty());
    assert!(svc.activities.is_empty());
    assert_eq!(
        wf.execution_timeout,
        Some(std::time::Duration::from_secs(3600))
    );
}

fn assert_golden(name: &str) {
    let services = parse_and_validate(name);
    let opts = load_fixture_options(name);
    let actual = render::render(&services[0], &opts);
    let golden_path = fixture_path(name).join("expected.rs");

    if std::env::var_os("BLESS").is_some() {
        std::fs::write(&golden_path, &actual).expect("write golden");
        return;
    }

    let expected = std::fs::read_to_string(&golden_path).unwrap_or_else(|_| {
        panic!(
            "missing golden file at {}. Run `BLESS=1 cargo test ... {name}` to create it.",
            golden_path.display()
        )
    });
    if actual != expected {
        panic!(
            "rendered output diverges from golden at {}. \
             Rebless with `BLESS=1 cargo test ... {name}`.\n\n--- expected ---\n{expected}\n--- actual ---\n{actual}",
            golden_path.display()
        );
    }
}

/// Smoke check on top of the golden — kept because it pinpoints which
/// fragment changed when the golden diffs.
#[test]
fn minimal_workflow_render_smoke() {
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());

    let must_contain = [
        "// Code generated by protoc-gen-rust-temporal. DO NOT EDIT.",
        "pub mod jobs_v1_job_service_temporal {",
        "use crate::jobs::v1::*;",
        "pub const RUN_JOB_WORKFLOW_NAME: &str = \"jobs.v1.JobService.RunJob\";",
        "pub const RUN_JOB_TASK_QUEUE: &str = \"jobs\";",
        "pub struct JobServiceClient {",
        "pub async fn run_job(",
        "pub fn run_job_handle(&self, workflow_id: impl Into<String>) -> RunJobHandle",
        "pub struct RunJobHandle {",
        "pub async fn result(&self) -> Result<JobOutput>",
        "pub async fn cancel_job(&self, input: CancelJobInput) -> Result<()>",
        "pub async fn get_status(&self) -> Result<JobStatusOutput>",
        "pub async fn reconfigure(&self, input: ReconfigureInput, wait_policy: temporal_runtime::WaitPolicy)",
        "pub async fn cancel_job_with_start(",
        "fn run_job_id(input: &JobInput) -> String",
        "run_job_id(&input)",
    ];
    for needle in must_contain {
        assert!(
            source.contains(needle),
            "rendered output is missing expected fragment: {needle:?}\n\n--- full output ---\n{source}"
        );
    }

    // Activity rpcs are validate-only — they must not produce client methods.
    assert!(
        !source.contains("process_chunk"),
        "activity-only method leaked into rendered client surface:\n{source}"
    );
}

/// Regression test for issue #1 (lazy ExtensionSet). buf v2 invokes the
/// plugin once per target proto in a module. When the target is the
/// vendored annotation schema itself — `temporal/v1/temporal.proto` —
/// the plugin used to die during `ExtensionSet::load()` because the
/// schema file declares the extensions but doesn't use them, and the
/// CodeGeneratorRequest for that single-target invocation may not be
/// shaped the way the extension lookup expects. Lazy-loading turns that
/// scenario into a no-op (empty output, no error).
#[test]
fn annotation_schema_as_target_is_a_noop() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let annotations = crate_root.join(ANNOTATIONS_DIR);
    let tmp = tempfile::tempdir().expect("tempdir");
    let fds_path = tmp.path().join("out.fds");
    let status = Command::new(protoc_binary())
        .arg(format!("-I{}", annotations.display()))
        .arg(format!("--descriptor_set_out={}", fds_path.display()))
        .arg("--include_imports")
        .arg("temporal/v1/temporal.proto")
        .status()
        .expect("invoke protoc");
    assert!(status.success(), "protoc dump failed: {status}");

    let bytes = std::fs::read(&fds_path).expect("read fds");
    let mut pool = DescriptorPool::new();
    pool.decode_file_descriptor_set(bytes.as_slice())
        .expect("decode fds");

    let files_to_generate: HashSet<String> =
        std::iter::once("temporal/v1/temporal.proto".to_string()).collect();
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    assert!(
        services.is_empty(),
        "annotation schema target should produce no services"
    );
}

#[test]
fn signal_returning_non_empty_fails_validation() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Cancel(In) returns (Out) {
            option (temporal.v1.signal) = {};
          }
        }
        message In {}
        message Out {}
        "#,
    );

    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let err = validate::validate(&services[0], &Default::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("google.protobuf.Empty"),
        "validation error should mention Empty constraint, got: {err}"
    );
}

#[test]
fn workflow_with_retry_policy_is_rejected() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              retry_policy: { max_attempts: 3 }
            };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("retry_policy") && err.contains("does not yet honour"),
        "expected unsupported-field diagnostic mentioning retry_policy, got: {err}"
    );
}

#[test]
fn workflow_with_multiple_unsupported_fields_lists_all() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:           "tq"
              search_attributes:    "string.foo = \"bar\""
              wait_for_cancellation: true
              enable_eager_start:    true
            };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    for field in [
        "search_attributes",
        "wait_for_cancellation",
        "enable_eager_start",
    ] {
        assert!(
            err.contains(field),
            "diagnostic should list {field}, got: {err}"
        );
    }
}

#[test]
fn update_with_id_template_is_rejected() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Patch" }]
            };
          }
          rpc Patch(In) returns (Out) {
            option (temporal.v1.update) = { id: "patch-{{ .Field }}" };
          }
        }
        message In { string field = 1; }
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("temporal.v1.update") && err.contains("id"),
        "expected update-id diagnostic, got: {err}"
    );
}

#[test]
fn update_with_deprecated_wait_policy_is_rejected() {
    // The deprecated `wait_policy` field on UpdateOptions still appears on
    // legacy protos ported from cludden's Go plugin. Silently ignoring it
    // would let a user lose their default WaitPolicy on the Rust client.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Patch" }]
            };
          }
          rpc Patch(In) returns (Out) {
            option (temporal.v1.update) = { wait_policy: WAIT_POLICY_ACCEPTED };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("wait_policy"),
        "expected deprecated-wait_policy diagnostic, got: {err}"
    );
}

#[test]
fn workflow_update_ref_with_conflict_policy_is_rejected() {
    // The bridge's update-with-start path hardcodes UseExisting; a per-
    // update conflict_policy override on WorkflowOptions.update[] would
    // be silently dropped. Refuse the proto rather than ship the wrong
    // policy.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Patch" workflow_id_conflict_policy: WORKFLOW_ID_CONFLICT_POLICY_FAIL }]
            };
          }
          rpc Patch(In) returns (Out) {
            option (temporal.v1.update) = {};
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("workflow_id_conflict_policy") && err.contains("Patch"),
        "expected nested-update conflict_policy diagnostic, got: {err}"
    );
}

#[test]
fn workflow_id_with_bloblang_expression_is_rejected() {
    // Bloblang `${! ... }` is cludden's search-attribute templating dialect
    // and looks like literal text to the `{{...}}` scanner. Without an
    // explicit reject, every workflow under such an annotation would ship
    // with the same literal ID and collide on every execution.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              id: "user-${! name.or(\"anon\") }"
            };
          }
        }
        message In { string name = 1; }
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate).unwrap_err();
    let full = format!("{err:#}");
    assert!(
        full.contains("Bloblang"),
        "expected Bloblang-rejection diagnostic, got: {full}"
    );
}

#[test]
fn activity_with_timeouts_is_rejected() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Work(In) returns (Out) {
            option (temporal.v1.activity) = {
              task_queue: "workers"
              start_to_close_timeout: { seconds: 30 }
            };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("task_queue") && err.contains("start_to_close_timeout"),
        "expected activity diagnostic listing both unsupported fields, got: {err}"
    );
}

#[test]
fn workflow_schema_defaults_apply_at_start() {
    // full_workflow declares id_reuse_policy + 3 timeouts on the proto;
    // the start path must fold those defaults in when the caller leaves
    // the StartOptions field as `None`, so mixed Rust/Go workers driving
    // the same workflow get the same effective options.
    let services = parse_and_validate("full_workflow");
    let source = render::render(&services[0], &Default::default());
    for fragment in [
        "let id_reuse_policy = opts.id_reuse_policy.or(Some(temporal_runtime::WorkflowIdReusePolicy::AllowDuplicateFailedOnly));",
        "let execution_timeout = opts.execution_timeout.or(Some(Duration::from_secs(7200)));",
        "let run_timeout = opts.run_timeout.or(Some(Duration::from_secs(3600)));",
        "let task_timeout = opts.task_timeout.or(Some(Duration::from_secs(60)));",
    ] {
        assert!(
            source.contains(fragment),
            "start path should fold proto default in: {fragment}\n--- source ---\n{source}"
        );
    }
    // Three start paths (Client::run, bootstrap_with_start, reconfigure_with_start)
    // each get their own resolution block — guard against accidental dedup.
    let occurrences = source
        .matches("let id_reuse_policy = opts.id_reuse_policy")
        .count();
    assert_eq!(
        occurrences, 3,
        "expected id_reuse_policy default applied in all 3 start paths, got {occurrences}"
    );
}

#[test]
fn workflow_without_schema_defaults_passes_through() {
    // minimal_workflow declares no id_reuse_policy / timeouts on the proto,
    // so the start path should pass `opts.<field>` through unchanged (no
    // synthesized default that the proto didn't request).
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("let id_reuse_policy = opts.id_reuse_policy;"),
        "no-default fields should still bind locals so the trailing call site stays uniform"
    );
    assert!(
        !source.contains("opts.id_reuse_policy.or(Some("),
        "no proto-level id_reuse_policy declared — must not synthesize a default"
    );
}
