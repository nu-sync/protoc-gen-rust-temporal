//! Default implementation of the `crate::temporal_runtime` facade that
//! `protoc-gen-rust-temporal` emits calls against. Backed by
//! `temporalio-client = "=0.4"` (exact-patch pinned — bridge crate minor
//! versions track SDK reshapes; plugin emit is unaffected).
//!
//! # Usage
//!
//! Add the dep and re-export from your crate's `lib.rs`:
//!
//! ```toml
//! [dependencies]
//! temporal-proto-runtime-bridge = "0.1"
//! ```
//!
//! ```ignore
//! pub use temporal_proto_runtime_bridge as temporal_runtime;
//! ```
//!
//! That's the whole wiring. The hand-written `temporal_runtime.rs` becomes
//! optional — only consumers who stub for tests or pin a vendored SDK keep
//! their own.
//!
//! See `docs/RUNTIME-API.md` for the contract this crate implements.

// Subsequent tasks fill this module in.
