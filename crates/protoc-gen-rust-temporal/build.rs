use std::io::Result;

fn main() -> Result<()> {
    let proto_root = "proto";
    println!("cargo:rerun-if-changed={proto_root}");

    let mut config = prost_build::Config::new();
    config.compile_protos(
        &["proto/temporal/v1/temporal.proto"],
        &[proto_root, "proto/temporal/api/enums/v1"],
    )?;
    Ok(())
}
