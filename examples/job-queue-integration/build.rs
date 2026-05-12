//! Generate prost types for the example's `jobs.v1.*` messages.
//!
//! Only the example's own proto produces output. `temporal/v1/temporal.proto`
//! is on the include path so prost-build can resolve the
//! `option (temporal.v1.workflow) = {…}` syntax during parse, but the
//! annotation schema's types are not emitted here — the plugin already
//! consumes them; the example only needs the `jobs.v1.*` message types.

use std::io::Result;

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=proto");
    println!("cargo:rerun-if-changed=../../crates/protoc-gen-rust-temporal/proto");

    let mut config = prost_build::Config::new();
    config.compile_protos(
        &["proto/jobs/v1/jobs.proto"],
        &["proto", "../../crates/protoc-gen-rust-temporal/proto"],
    )?;
    Ok(())
}
