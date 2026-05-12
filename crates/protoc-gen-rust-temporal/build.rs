use std::io::Result;
use std::path::PathBuf;
use std::process::Command;

/// BSR module commit (immutable) for cludden's annotation schema. Updating
/// this is the explicit re-pin act. The corresponding human-readable label
/// is `v1.22.1`; we pin to the digest because BSR labels are mutable and the
/// commit isn't.
const CLUDDEN_BSR_COMMIT: &str =
    "buf.build/cludden/protoc-gen-go-temporal:6d988a28838c46ebb99eaa042cf2a607";

fn main() -> Result<()> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR set by cargo"));
    let schema_dir = out_dir.join("cludden-schema");

    // Re-run when this build script itself changes (i.e. when we re-pin).
    println!("cargo:rerun-if-changed=build.rs");
    // Allow opt-out for offline / air-gapped builds: VENDORED_SCHEMA=1
    // falls back to the in-tree proto directory.
    println!("cargo:rerun-if-env-changed=VENDORED_SCHEMA");

    let proto_root: PathBuf = if std::env::var_os("VENDORED_SCHEMA").is_some() {
        // Hermetic fallback for offline CI / sandboxed builds: use the
        // checked-in copy under `proto/`. Should match the BSR commit at
        // re-pin time (verified during pin updates).
        println!("cargo:warning=VENDORED_SCHEMA set; using in-tree proto/ instead of BSR fetch");
        println!("cargo:rerun-if-changed=proto");
        PathBuf::from("proto")
    } else {
        // Default path: `buf export <bsr-commit>` into OUT_DIR, then point
        // prost-build at the exported tree. The fetch is idempotent — buf
        // caches the module under ~/.cache/buf.
        if schema_dir.exists() {
            // Wipe stale exports so re-pins always reflect the new commit
            // (buf export refuses to write into a non-empty dir).
            std::fs::remove_dir_all(&schema_dir)?;
        }
        std::fs::create_dir_all(&schema_dir)?;
        let status = Command::new("buf")
            .args(["export", CLUDDEN_BSR_COMMIT, "-o"])
            .arg(&schema_dir)
            .status()
            .map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!(
                        "failed to invoke `buf` ({e}). \
                         Install buf <https://buf.build/docs/installation> \
                         or set VENDORED_SCHEMA=1 to fall back to the \
                         in-tree proto/."
                    ),
                )
            })?;
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "buf export failed with status {status} for {CLUDDEN_BSR_COMMIT}"
            )));
        }
        schema_dir
    };

    let temporal_proto = proto_root.join("temporal/v1/temporal.proto");
    let enums_dir = proto_root.join("temporal/api/enums/v1");

    let mut config = prost_build::Config::new();
    config.compile_protos(
        &[temporal_proto.to_str().unwrap()],
        &[proto_root.to_str().unwrap(), enums_dir.to_str().unwrap()],
    )?;
    Ok(())
}
