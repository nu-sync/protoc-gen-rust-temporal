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
use protoc_gen_rust_temporal::{parse, validate};

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
    for service in &services {
        validate::validate(service).expect("validate");
    }
    services
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
    assert_eq!(wf.id_expression.as_deref(), Some("{{ .Name }}"));
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
    let err = validate::validate(&services[0]).unwrap_err().to_string();
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
    let err = validate::validate(&services[0]).unwrap_err().to_string();
    assert!(
        err.contains("task_queue"),
        "validation error should mention task_queue, got: {err}"
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
    let err = validate::validate(&services[0]).unwrap_err().to_string();
    assert!(
        err.contains("google.protobuf.Empty"),
        "validation error should mention Empty constraint, got: {err}"
    );
}
