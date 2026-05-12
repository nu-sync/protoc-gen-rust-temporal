//! `ServiceModel` -> Rust source emission. See SPEC.md Phase 2.
//!
//! Phase 2 ports the renderer from the PoC and extends it with `update` +
//! `signal_with_start` emit. Outputs are written into the generated module
//! tree using `temporal-proto-runtime::TypedProtoMessage` to enforce the
//! `binary/protobuf` wire format.
