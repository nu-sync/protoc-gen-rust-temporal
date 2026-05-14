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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use prost::Message;
use prost_reflect::DescriptorPool;
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse};

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
        .unwrap_or_else(|| {
            protoc_bin_vendored::protoc_bin_path().expect("vendored protoc not available")
        })
}

fn protoc_include_path() -> PathBuf {
    protoc_bin_vendored::include_path().expect("vendored protobuf includes not available")
}

#[test]
fn fixture_outputs_via_protoc_match_in_process_render() {
    for fixture in [
        "minimal_workflow",
        "activities_emit",
        "workflows_emit",
        "worker_full",
        "cli_emit",
    ] {
        assert_fixture_via_protoc_matches_in_process_render(fixture);
    }
}

fn assert_fixture_via_protoc_matches_in_process_render(fixture: &str) {
    let fixture_dir = fixture_dir(fixture);
    let annotations = crate_root().join(ANNOTATIONS_DIR);
    let options = fixture_options(fixture);

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
        .arg(format!("-I{}", protoc_include_path().display()))
        .arg(format!("--rust-temporal_out={}", out_dir.display()))
        .args(
            options
                .as_ref()
                .map(|opt| format!("--rust-temporal_opt={opt}")),
        )
        .arg("input.proto")
        .status()
        .expect("invoke protoc");
    assert!(
        status.success(),
        "protoc failed for fixture `{fixture}`: {status}"
    );

    // Plugin emits `<stem>_temporal.rs` for each input proto. The fixture's
    // stem is `input` so we look there.
    let on_disk = fs::read_to_string(out_dir.join("input_temporal.rs"))
        .expect("read plugin output from disk");

    let in_process = render_in_process(&fixture_dir, options.as_deref());

    assert_eq!(
        on_disk, in_process,
        "protoc-invoked plugin output diverges from in-process render for fixture `{fixture}`. \
         This usually means the stdin/stdout framing or the \
         CodeGeneratorResponse encoding/options plumbing regressed."
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
        .arg(format!("-I{}", protoc_include_path().display()))
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

#[test]
fn invalid_plugin_option_surfaces_in_code_generator_response() {
    let response = invoke_plugin_raw(
        &CodeGeneratorRequest {
            parameter: Some("activities=yes".to_string()),
            ..Default::default()
        }
        .encode_to_vec(),
    );

    let error = response.error.expect("invalid option should set error");
    assert!(
        error.contains("activities") && error.contains("true|false"),
        "diagnostic should identify the bad option: {error}"
    );
}

#[test]
fn malformed_request_bytes_surface_in_code_generator_response() {
    // Field 15 (`proto_file`) claims a five-byte length but supplies one byte.
    // This pins the plugin binary's behavior: malformed stdin must become a
    // CodeGeneratorResponse.error, not a panic or invalid stdout.
    let response = invoke_plugin_raw(&[0x7a, 0x05, 0x01]);
    let error = response
        .error
        .expect("malformed request bytes should set error");
    assert!(
        error.contains("decode CodeGeneratorRequest") || error.contains("truncated"),
        "diagnostic should mention request decoding/truncation: {error}"
    );
}

#[test]
fn empty_file_to_generate_produces_empty_success_response() {
    let response = invoke_plugin_raw(&CodeGeneratorRequest::default().encode_to_vec());
    assert_eq!(response.error, None);
    assert!(
        response.file.is_empty(),
        "no requested files should produce no output files"
    );
}

#[test]
fn multiple_input_proto_files_emit_one_temporal_file_each() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(
        tmp.path().join("alpha.proto"),
        r#"
        syntax = "proto3";
        package multi.alpha.v1;
        import "temporal/v1/temporal.proto";

        service AlphaService {
          rpc Run(AlphaInput) returns (AlphaOutput) {
            option (temporal.v1.workflow) = { task_queue: "alpha" };
          }
        }
        message AlphaInput {}
        message AlphaOutput {}
        "#,
    )
    .expect("write alpha proto");
    fs::write(
        tmp.path().join("beta.proto"),
        r#"
        syntax = "proto3";
        package multi.beta.v1;
        import "temporal/v1/temporal.proto";

        service BetaService {
          rpc Run(BetaInput) returns (BetaOutput) {
            option (temporal.v1.workflow) = { task_queue: "beta" };
          }
        }
        message BetaInput {}
        message BetaOutput {}
        "#,
    )
    .expect("write beta proto");

    let out_dir = tmp.path().join("out");
    fs::create_dir_all(&out_dir).expect("mkdir out");

    let status = Command::new(protoc())
        .arg(format!(
            "--plugin=protoc-gen-rust-temporal={}",
            plugin_binary().display()
        ))
        .arg(format!("-I{}", tmp.path().display()))
        .arg(format!(
            "-I{}",
            crate_root().join(ANNOTATIONS_DIR).display()
        ))
        .arg(format!("-I{}", protoc_include_path().display()))
        .arg(format!("--rust-temporal_out={}", out_dir.display()))
        .args(["alpha.proto", "beta.proto"])
        .status()
        .expect("invoke protoc");
    assert!(status.success(), "protoc failed: {status}");

    let alpha =
        fs::read_to_string(out_dir.join("alpha_temporal.rs")).expect("read alpha_temporal.rs");
    let beta = fs::read_to_string(out_dir.join("beta_temporal.rs")).expect("read beta_temporal.rs");
    assert!(alpha.contains("AlphaServiceClient"));
    assert!(beta.contains("BetaServiceClient"));
}

fn fixture_dir(name: &str) -> PathBuf {
    crate_root().join("tests").join("fixtures").join(name)
}

fn fixture_options(name: &str) -> Option<String> {
    let path = fixture_dir(name).join("options.txt");
    path.exists().then(|| {
        fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
            .trim()
            .to_string()
    })
}

fn invoke_plugin_raw(raw: &[u8]) -> CodeGeneratorResponse {
    let mut child = Command::new(plugin_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn plugin");
    child
        .stdin
        .as_mut()
        .expect("plugin stdin")
        .write_all(raw)
        .expect("write plugin stdin");
    let output = child.wait_with_output().expect("wait for plugin");
    assert!(
        output.status.success(),
        "plugin process should encode errors into CodeGeneratorResponse, status: {}",
        output.status
    );
    CodeGeneratorResponse::decode(output.stdout.as_slice()).expect("decode plugin response")
}

fn render_in_process(fixture_dir: &Path, raw_options: Option<&str>) -> String {
    let annotations = crate_root().join(ANNOTATIONS_DIR);
    let tmp = tempfile::tempdir().expect("tempdir");
    let fds_path = tmp.path().join("out.fds");
    let status = Command::new(protoc())
        .arg(format!("-I{}", fixture_dir.display()))
        .arg(format!("-I{}", annotations.display()))
        .arg(format!("-I{}", protoc_include_path().display()))
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
    let options = raw_options
        .map(protoc_gen_rust_temporal::options::parse_options)
        .transpose()
        .expect("parse fixture options")
        .unwrap_or_default();
    for s in &services {
        validate::validate(s, &options).expect("validate");
    }
    render::render(&services[0], &options)
}
