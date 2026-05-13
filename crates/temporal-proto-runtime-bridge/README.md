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

## Worker side (Phase 2+, opt-in)

The plugin's worker emit (activities, workflows) gives you typed contracts and
thin `register_*` helpers. Activity bodies and workflow bodies remain
consumer-owned because `temporalio-sdk` registers macro-generated concrete
types, not arbitrary name/function pairs. Enable the bridge crate's `worker`
feature to get the SDK types re-exported alongside the client surface:

```toml
[dependencies]
temporal-proto-runtime-bridge = { version = "0.1", features = ["worker"] }
```

Adapter pattern (for a service with `Process(ChunkInput) -> ChunkOutput`):

```rust,ignore
use std::sync::Arc;
use anyhow::Result;
use temporal_runtime::ActivityContext;
use temporal_runtime::worker::Worker;

// 1. Impl the plugin-generated trait on your state struct.
pub struct MyImpl { /* shared deps here */ }

impl crate::generated::ChunkServiceActivities for MyImpl {
    fn process(
        &self,
        ctx: ActivityContext,
        input: ChunkInput,
    ) -> impl std::future::Future<Output = Result<ChunkOutput>> + Send {
        async move {
            // your activity body
            Ok(ChunkOutput { hash: 42 })
        }
    }
    // …one per activity in the trait…
}

// 2. Adapt via the SDK macro. This generates ActivityDefinition +
//    ExecutableActivity impls per method, tied to `MyImpl`.
#[temporalio_macros::activities]
impl MyImpl {
    #[activity(name = crate::generated::PROCESS_ACTIVITY_NAME)]
    async fn process_adapter(
        self: Arc<Self>,
        ctx: ActivityContext,
        input: ChunkInput,
    ) -> Result<ChunkOutput> {
        crate::generated::ChunkServiceActivities::process(&*self, ctx, input).await
    }
}

// 3. Register on the worker through the generated helper.
fn register(worker: &mut Worker, impl_: MyImpl) {
    crate::generated::register_chunk_service_activities(worker, impl_);
}
```

Why the adapter exists: see `docs/superpowers/specs/2026-05-12-phase-2-spike-findings.md`.
The SDK's static-dispatch activity registration needs per-impl marker types
that the plugin can't generate at codegen time (the user's concrete type
isn't visible). The adapter is the documented 15-LOC bridge.

[`protoc-gen-rust-temporal`]: https://github.com/nu-sync/protoc-gen-rust-temporal
[`docs/RUNTIME-API.md`]: ../../docs/RUNTIME-API.md
[`examples/job-queue-integration`]: ../../examples/job-queue-integration
