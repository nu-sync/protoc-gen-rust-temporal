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
    assert_eq!(wf.registered_name, "jobs.v1.JobService/RunJob");
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
        "pub const RUN_JOB_WORKFLOW_NAME: &str = \"jobs.v1.JobService/RunJob\";",
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
