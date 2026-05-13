//! Triggers `buf generate` if the generated sources are missing.
//!
//! `protoc-gen-rust-temporal` (and `protoc-gen-prost`) must be on `PATH`.
//! The example's `just gen` recipe builds the local plugin first and puts
//! `target/debug` on `PATH`; install `protoc-gen-prost` once with Cargo.
//! If the plugin is missing the build script still succeeds with a warning,
//! and the `include!` in lib.rs will fail until `just gen` runs cleanly.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../proto/jobs/v1/jobs.proto");
    println!("cargo:rerun-if-changed=../../proto/temporal/v1/temporal.proto");
    println!("cargo:rerun-if-changed=../../proto/temporal/api/enums/v1/workflow.proto");
    println!("cargo:rerun-if-changed=../../proto/buf.gen.yaml");

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let gen_marker = manifest_dir.join("src/gen/jobs.v1.rs");

    if gen_marker.exists() {
        return;
    }

    // Pre-flight: surface a clear error before buf produces a generic one.
    if which("protoc-gen-rust-temporal").is_none() {
        println!(
            "cargo:warning=protoc-gen-rust-temporal not found on PATH; run `cd examples/job-queue && just gen`"
        );
        return;
    }

    let proto_dir = manifest_dir.join("../../proto");
    let status = Command::new("buf")
        .arg("generate")
        .current_dir(&proto_dir)
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            println!(
                "cargo:warning=`buf generate` exited with {s}; jobs-proto will fail to compile until `just gen` succeeds"
            );
        }
        Err(e) => {
            println!(
                "cargo:warning=could not run buf ({e}); install buf + protoc-gen-rust-temporal + protoc-gen-prost, then `just gen`"
            );
        }
    }
}

fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
