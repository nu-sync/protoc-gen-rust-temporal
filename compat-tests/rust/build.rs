//! Compile the fixture protos used by the audit. Keeps the fixture types
//! decoupled from the main plugin's `temporal.v1.*` schema (these fixtures
//! describe a hypothetical consumer's `jobs.v1.*` types).

use std::io::Result;

fn main() -> Result<()> {
    let proto_root = "proto";
    println!("cargo:rerun-if-changed={proto_root}");
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc not available");
    let protoc_include =
        protoc_bin_vendored::include_path().expect("vendored protobuf includes not available");

    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config.compile_protos(
        &["proto/jobs/v1/jobs.proto"],
        &[proto_root, protoc_include.to_str().unwrap()],
    )?;
    Ok(())
}
