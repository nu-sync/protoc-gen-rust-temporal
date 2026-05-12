//! Internal representation produced by `parse.rs` and consumed by `render.rs`.
//!
//! Phase 0 is intentionally empty; Phase 1 lifts `ServiceModel` /
//! `WorkflowModel` / `SignalModel` / `QueryModel` / `UpdateModel` /
//! `ActivityModel` from the PoC in
//! `job-queue/crates/protoc-gen-rust-temporal-client/src/model.rs`, adapted to
//! cludden's nested `Signal { ref, start, ... }` / `Query { ref, ... }` /
//! `Update { ref, ... }` shape.
