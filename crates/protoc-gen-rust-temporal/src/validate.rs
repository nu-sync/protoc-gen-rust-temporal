//! Cross-method invariants applied after parsing.
//!
//! Phase 1 will enforce:
//! - Every `WorkflowOptions.signal[].ref` resolves to a method with a
//!   matching `(temporal.v1.signal)` annotation, and the signal's response
//!   type is `google.protobuf.Empty`.
//! - Every `WorkflowOptions.query[].ref` resolves similarly.
//! - Every `WorkflowOptions.update[].ref` resolves similarly.
//! - Activity-annotated methods do not collide with workflow / signal /
//!   query / update names within the same service.
