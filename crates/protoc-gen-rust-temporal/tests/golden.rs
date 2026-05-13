//! Golden render checks for worker-side emit fixtures.
//!
//! This target exists so the handoff's focused command
//! `cargo test -p protoc-gen-rust-temporal --test golden` has a stable
//! entry point. The broader `parse_validate` test target still owns the full
//! parser and validation regression suite.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use prost_reflect::DescriptorPool;
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
    assert!(status.success(), "protoc failed: {status}");

    let bytes = std::fs::read(&fds_path).expect("read fds");
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
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    protoc_gen_rust_temporal::options::parse_options(raw.trim())
        .unwrap_or_else(|err| panic!("parse {}: {err}", path.display()))
}

fn assert_golden(name: &str) {
    let (pool, files_to_generate) = compile_fixture(name);
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let opts = load_fixture_options(name);
    for service in &services {
        validate::validate(service, &opts).expect("validate");
    }
    let actual = render::render(&services[0], &opts);
    let golden_path = fixture_path(name).join("expected.rs");

    if std::env::var_os("BLESS").is_some() {
        std::fs::write(&golden_path, &actual).expect("write golden");
        return;
    }

    let expected = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|err| panic!("read {}: {err}", golden_path.display()));
    assert_eq!(actual, expected, "golden drift in fixture `{name}`");
}

#[test]
fn worker_emit_golden_fixtures_match() {
    for fixture in [
        "worker_workflow_only",
        "worker_activities_only",
        "worker_full",
    ] {
        assert_golden(fixture);
    }
}
