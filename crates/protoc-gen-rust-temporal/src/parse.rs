//! `DescriptorPool` -> `ServiceModel` extraction. See SPEC.md Phase 1.
//!
//! The PoC in
//! `job-queue/crates/protoc-gen-rust-temporal-client/src/parse.rs`
//! targets the old schema (`repeated string signal`, field numbers
//! 7233001-7233003). cludden's schema uses nested `Signal { ref, start }` /
//! `Query { ref }` / `Update { ref }` messages at field numbers 7233-7237.
//! The Phase 1 reimplementation walks the new shape and validates that every
//! `ref` resolves to a method carrying the matching annotation.
