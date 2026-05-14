//! Regression test for the build-script marker-path bug.
//!
//! `build.rs` skips regeneration when its marker file already exists. The
//! marker has to point at a path inside the actual `buf generate` output,
//! otherwise the script falls through every cargo invocation and re-runs
//! the plugin against whatever stale `protoc-gen-rust-temporal` is on
//! PATH — clobbering checked-in good output.
//!
//! This test pins the invariant: the marker path must be an actually-
//! generated file that `lib.rs` consumes via `include!`.

use std::path::PathBuf;

#[test]
fn build_script_marker_path_exists_in_generated_tree() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Mirror of `gen_marker` in build.rs — kept in lockstep by hand.
    // If this test fails, either the buf.gen.yaml output layout moved
    // or the marker path drifted; fix the build script first, not
    // this test.
    let marker = manifest.join("src/gen/jobs/v1/jobs_temporal.rs");
    assert!(
        marker.exists(),
        "build-script marker `{}` does not exist — build.rs would re-run buf generate \
         on every cargo invocation, silently clobbering checked-in generated code with \
         the output of whatever stale `protoc-gen-rust-temporal` is on PATH",
        marker.display()
    );
}
