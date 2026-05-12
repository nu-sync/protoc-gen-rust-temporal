# temporal-proto-runtime-bridge

Default bridge crate for [`protoc-gen-rust-temporal`]-generated clients. Implements
every symbol in [`docs/RUNTIME-API.md`] against `temporalio-client 0.4`.

## Wiring

```toml
[dependencies]
temporal-proto-runtime-bridge = "0.1"
```

```rust,ignore
// In your crate's lib.rs:
pub use temporal_proto_runtime_bridge as temporal_runtime;
```

That single re-export satisfies every `crate::temporal_runtime::*` reference the
plugin emits.

## SDK pinning

This crate pins `temporalio-client = "=0.4.0"` (exact patch). When the SDK ships
a breaking 0.5, we cut `temporal-proto-runtime-bridge 0.2`; the plugin's emit
shape does not change, so consumers bump the bridge crate version and recompile.

## Override

Drop the `pub use` line and write your own `mod temporal_runtime;` against the
facade — for tests, vendored SDKs, or custom transport. The
[`examples/job-queue-integration`] crate ships a `todo!()`-bodied stub that's
the canonical override reference.

[`protoc-gen-rust-temporal`]: https://github.com/nu-sync/protoc-gen-rust-temporal
[`docs/RUNTIME-API.md`]: ../../docs/RUNTIME-API.md
[`examples/job-queue-integration`]: ../../examples/job-queue-integration
