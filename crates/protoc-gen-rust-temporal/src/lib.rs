//! `protoc-gen-rust-temporal` library entry point.
//!
//! The binary in `main.rs` is a thin stdin/stdout shell; the actual
//! `CodeGeneratorRequest -> CodeGeneratorResponse` pipeline lives here so
//! tests (and the eventual golden harness) can exercise it without spawning a
//! subprocess.
//!
//! Phase 0 ships only the plumbing: `run_with_pool` accepts a descriptor pool
//! that already has `temporal.v1.*` extensions attached (per `main.rs`'s
//! descriptor-pool extraction trick), plus the set of files the caller asked
//! the plugin to generate. Phase 1 wires up `parse → validate → render`.

use std::collections::HashSet;

use anyhow::Result;
use prost_reflect::DescriptorPool;
use prost_types::compiler::code_generator_response::File;

pub mod model;
pub mod parse;
mod render;
pub mod validate;

/// Generated prost types for cludden's `temporal.v1.*` annotation schema and
/// the transitively-referenced `temporal.api.enums.v1` enums. The parser uses
/// `prost-reflect` against the descriptor pool, but these types are exposed
/// for downstream introspection (e.g. tests round-tripping options).
pub mod temporal {
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/temporal.v1.rs"));
    }
    pub mod api {
        pub mod enums {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/temporal.api.enums.v1.rs"));
            }
        }
    }
}

/// Run the plugin pipeline against a fully-populated descriptor pool.
///
/// `files_to_generate` is the set of `.proto` file paths the plugin was asked
/// to emit code for (mirrors `CodeGeneratorRequest::file_to_generate`).
pub fn run_with_pool(
    pool: &DescriptorPool,
    files_to_generate: &HashSet<String>,
) -> Result<Vec<File>> {
    let services = parse::parse(pool, files_to_generate)?;
    for service in &services {
        validate::validate(service)?;
    }
    // Phase 2 wires `render` in here.
    Ok(Vec::new())
}
