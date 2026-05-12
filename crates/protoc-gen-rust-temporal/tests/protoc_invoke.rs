//! End-to-end test that drives the plugin through real `protoc`.
//!
//! The in-process tests in `parse_validate.rs` exercise the
//! `parse → validate → render` pipeline against a `DescriptorPool` built
//! from a `FileDescriptorSet`. That skips the actual plugin protocol —
//! stdin framing of the `CodeGeneratorRequest`, stdout framing of the
//! `CodeGeneratorResponse`, the `--<name>_out` flag, and `protoc`'s
//! handling of `CodeGeneratorResponse.error`. This test runs the full
//! contract: invoke `protoc` with `--plugin=...` pointing at our compiled
//! binary, ask it to emit the generated file, and diff the on-disk
//! output against the in-process golden.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use prost_reflect::DescriptorPool;

use protoc_gen_rust_temporal::{parse, render, validate};

const ANNOTATIONS_DIR: &str = "proto";

fn crate_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn plugin_binary() -> PathBuf {
    // Cargo sets CARGO_BIN_EXE_<name> for binaries declared in the same
    // package as the integration test. This is the canonical way to
    // resolve a sibling binary without hardcoding `target/debug/...`.
    PathBuf::from(env!("CARGO_BIN_EXE_protoc-gen-rust-temporal"))
}

fn protoc() -> PathBuf {
    std::env::var_os("PROTOC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("protoc"))
}

#[test]
fn minimal_workflow_via_protoc_matches_in_process_render() {
    let fixture_dir = crate_root()
        .join("tests")
        .join("fixtures")
        .join("minimal_workflow");
    let annotations = crate_root().join(ANNOTATIONS_DIR);

    let tmp = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp.path().join("out");
    fs::create_dir_all(&out_dir).expect("mkdir out");

    let plugin = plugin_binary();
    assert!(
        plugin.exists(),
        "plugin binary missing at {} — did `cargo test` build the bin?",
        plugin.display()
    );

    let status = Command::new(protoc())
        .arg(format!(
            "--plugin=protoc-gen-rust-temporal={}",
            plugin.display()
        ))
        .arg(format!("-I{}", fixture_dir.display()))
        .arg(format!("-I{}", annotations.display()))
        .arg(format!("--rust-temporal_out={}", out_dir.display()))
        .arg("input.proto")
        .status()
        .expect("invoke protoc");
    assert!(status.success(), "protoc failed: {status}");

    // Plugin emits `<stem>_temporal.rs` for each input proto. The fixture's
    // stem is `input` so we look there.
    let on_disk = fs::read_to_string(out_dir.join("input_temporal.rs"))
        .expect("read plugin output from disk");

    let in_process = render_in_process(&fixture_dir);

    assert_eq!(
        on_disk, in_process,
        "protoc-invoked plugin output diverges from in-process render. \
         This usually means the stdin/stdout framing or the \
         CodeGeneratorResponse encoding regressed."
    );
}

#[test]
fn validation_errors_surface_through_protoc() {
    // A workflow without task_queue and without service-level default
    // should make protoc exit non-zero with the validate.rs error text
    // in its stderr.
    let tmp = tempfile::tempdir().expect("tempdir");
    let proto_path = tmp.path().join("input.proto");
    fs::write(
        &proto_path,
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
    )
    .expect("write bad proto");

    let out_dir = tmp.path().join("out");
    fs::create_dir_all(&out_dir).expect("mkdir out");

    let output = Command::new(protoc())
        .arg(format!(
            "--plugin=protoc-gen-rust-temporal={}",
            plugin_binary().display()
        ))
        .arg(format!("-I{}", tmp.path().display()))
        .arg(format!(
            "-I{}",
            crate_root().join(ANNOTATIONS_DIR).display()
        ))
        .arg(format!("--rust-temporal_out={}", out_dir.display()))
        .arg("input.proto")
        .output()
        .expect("invoke protoc");

    assert!(
        !output.status.success(),
        "expected protoc to fail on missing task_queue, but it succeeded"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("task_queue"),
        "protoc stderr should surface the validation error mentioning task_queue, got:\n{stderr}"
    );
}

#[test]
fn version_flag_prints_package_version() {
    let output = Command::new(plugin_binary())
        .arg("--version")
        .output()
        .expect("invoke plugin");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let expected = format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    assert!(
        stdout.contains(&expected),
        "expected --version output to contain {expected:?}, got: {stdout}"
    );
}

fn render_in_process(fixture_dir: &Path) -> String {
    let annotations = crate_root().join(ANNOTATIONS_DIR);
    let tmp = tempfile::tempdir().expect("tempdir");
    let fds_path = tmp.path().join("out.fds");
    let status = Command::new(protoc())
        .arg(format!("-I{}", fixture_dir.display()))
        .arg(format!("-I{}", annotations.display()))
        .arg(format!("--descriptor_set_out={}", fds_path.display()))
        .arg("--include_imports")
        .arg("input.proto")
        .status()
        .expect("invoke protoc");
    assert!(status.success(), "protoc fds dump failed");

    let bytes = fs::read(&fds_path).expect("read fds");
    let mut pool = DescriptorPool::new();
    pool.decode_file_descriptor_set(bytes.as_slice())
        .expect("decode fds");

    let files: HashSet<String> = std::iter::once("input.proto".to_string()).collect();
    let services = parse::parse(&pool, &files).expect("parse");
    for s in &services {
        validate::validate(s).expect("validate");
    }
    render::render(&services[0])
}
