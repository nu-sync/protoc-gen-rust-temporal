# Phase 2 spike findings — Rust SDK worker shape

**Date:** 2026-05-12.
**Triggered by:** the design's Risks table:
> **Rust SDK worker primitives don't compose with generated traits.** `temporalio-sdk`'s `#[workflow]` macro is pre-1.0 and may assume hand-written types; codegen-driven impls could hit limitations.
> Mitigation: Phase 2 lead-in spike: prototype `register_<service>_activities` against `temporalio-sdk` 0.4 *before* committing to the trait shape in the implementation plan.

This is that spike. Findings below.

## What the design doc sketched

```rust
pub trait <Service>Activities: Send + Sync + 'static {
    async fn <activity>(&self, ctx: ActivityCtx, input: <Input>) -> Result<<Output>>;
}

pub fn register_<service>_activities(
    worker: &mut Worker,
    impl_: Arc<dyn <Service>Activities>,
);
```

## What `temporalio-sdk 0.4` actually requires

The SDK registers activities through **statically-typed marker structs** that impl two traits:

```rust
// from temporalio-common 0.4 (workflow_definition.rs:83+ish)
pub trait ActivityDefinition {
    type Input;
    type Output;
    fn name() -> &'static str;
}

// from temporalio-sdk 0.4 (activities.rs:358)
pub trait ExecutableActivity: ActivityDefinition {
    type Implementer: ActivityImplementer + Send + Sync + 'static;
    fn execute(
        receiver: Option<Arc<Self::Implementer>>,
        ctx: ActivityContext,
        input: Self::Input,
    ) -> BoxFuture<'static, Result<Self::Output, ActivityError>>;
}
```

Registration is:

```rust
worker.register_activity::<DoChunkMarker>(arc_my_impl);
// or
worker.register_activities(MyImpl::new());  // requires ActivityImplementer for MyImpl
```

The SDK's internal `ActivityInvocation` hashmap is `pub(crate)` — there is **no name-based dynamic registration path**.

## Why the design's sketch doesn't compile

`Arc<dyn <Service>Activities>` cannot satisfy `Worker::register_activity::<AD>(instance: Arc<AD::Implementer>)` because:
- `AD::Implementer` is an associated type chosen by the marker `AD`.
- The plugin doesn't know the user's concrete `MyImpl` type at codegen time.
- The marker's `Implementer` associated type therefore can't name `MyImpl`.

A generic marker (`pub struct DoChunkMarker<I>(PhantomData<I>);`) doesn't help either — `Worker::register_activity::<DoChunkMarker<MyImpl>>(...)` requires the user to spell `MyImpl` at the call site, which defeats the "5-line worker setup" goal.

## How temporalio-macros solves this

It generates **per-impl marker structs** at macro-expansion time. The user writes:

```rust
#[activity_definitions]
impl MyImpl {
    #[activity(name = "DoChunk")]
    async fn do_chunk(self: Arc<Self>, ctx: ActivityContext, input: ChunkInput) -> Result<ChunkOutput> { ... }
}
```

…and the macro emits:

```rust
mod my_impl_activities {
    pub struct DoChunk;  // ← marker scoped to *this* user impl
}
impl ActivityDefinition for my_impl_activities::DoChunk { type Input = ChunkInput; type Output = ChunkOutput; fn name() -> &'static str { "DoChunk" } }
impl ExecutableActivity for my_impl_activities::DoChunk { type Implementer = MyImpl; fn execute(recv, ctx, input) { recv.unwrap().do_chunk(ctx, input).boxed() } }
impl ActivityImplementer for MyImpl { fn register_all(self: Arc<Self>, defs) { defs.register_activity::<my_impl_activities::DoChunk>(self.clone()); } }
```

The marker `Implementer = MyImpl` is filled in because `MyImpl` is known at macro-expansion time.

## Options for Phase 2

### Option A — Plugin emits a declarative macro that the consumer invokes

Plugin generates:

```rust
pub trait JobServiceActivities: Send + Sync + 'static {
    async fn do_chunk(&self, ctx: ActivityContext, input: ChunkInput) -> Result<ChunkOutput>;
    // …per annotated activity…
}

#[macro_export]
macro_rules! __job_service_activity_impls {
    ($impl_type:ty) => {
        mod __job_service_markers {
            pub struct DoChunk;
        }
        impl temporal_runtime::ActivityDefinition for __job_service_markers::DoChunk { /* … */ }
        impl temporal_runtime::ExecutableActivity for __job_service_markers::DoChunk {
            type Implementer = $impl_type;
            fn execute(/* … */) { /* call $impl_type::do_chunk via the trait */ }
        }
        impl temporal_runtime::ActivityImplementer for $impl_type { fn register_all(/* … */) { /* … */ } }
    };
}

pub use __job_service_activity_impls as register_job_service_activity_impls;
```

Consumer:

```rust
struct MyImpl;
#[async_trait::async_trait]
impl JobServiceActivities for MyImpl { async fn do_chunk(&self, ctx, input) -> Result<…> { … } }

protoc_gen_rust_temporal::register_job_service_activity_impls!(MyImpl);

// In worker setup:
worker.register_activities(MyImpl::new());
```

Pros: closest to the macro pattern the SDK was designed around. One `register_activities!()` line is the "5-line" goal.
Cons: declarative macros are noisier to debug; emitting one from a proto plugin is unusual; the macro hides what's happening.

### Option B — Plugin emits the trait only; consumer writes their own markers

This is the design's documented fallback ("consumer writes a 5-line registration helper using a generated trait").

Plugin output:

```rust
pub trait JobServiceActivities: Send + Sync + 'static {
    async fn do_chunk(&self, ctx: ActivityContext, input: ChunkInput) -> Result<ChunkOutput>;
}
```

Consumer writes (cargo-cult-able from a doc snippet, but hand-written):

```rust
struct MyImpl;
#[async_trait::async_trait]
impl JobServiceActivities for MyImpl { … }

#[activity_definitions]
impl MyImpl {
    #[activity(name = "DoChunk")]
    async fn do_chunk(self: Arc<Self>, ctx: ActivityContext, input: ChunkInput) -> Result<ChunkOutput> {
        JobServiceActivities::do_chunk(&*self, ctx, input).await
    }
}

worker.register_activities(MyImpl::new());
```

Pros: zero new plugin complexity; uses the SDK's existing `#[activity_definitions]` macro untouched. Plugin's value is the trait surface (compile-time correctness of input/output types vs proto).
Cons: less DRY — consumer writes activity method signatures twice (once on the trait, once on the impl). PoC migration removes less hand-rolled code than the design promised.

### Option C — Bridge crate ships its own `BridgeActivity` registration helper

Add to the bridge crate:

```rust
pub struct BridgeActivity {
    name: &'static str,
    invoke: Arc<dyn Fn(/* payloads, ctx */) -> BoxFuture<'static, Result<Payload, ActivityError>> + Send + Sync>,
}

impl BridgeActivity {
    pub fn new<I: TemporalProtoMessage, O: TemporalProtoMessage, F, Fut>(
        name: &'static str,
        handler: F,
    ) -> Self
    where
        F: Fn(ActivityContext, I) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<O, anyhow::Error>> + Send + 'static,
    { /* … */ }
}

pub fn register_bridge_activities(worker: &mut Worker, defs: Vec<BridgeActivity>) { /* … */ }
```

Then plugin emits `register_<service>_activities(worker, impl_)` that internally builds a `Vec<BridgeActivity>` from the trait methods and registers them through `register_bridge_activities`.

**Problem:** still has to bridge `BridgeActivity` into `ActivityDefinitions` — and that hashmap is `pub(crate)`. So this option requires upstream SDK changes (exposing a name-based registration API), or a fork. Off the table for Phase 2 unless we want to lobby upstream first.

## Recommendation

**Option B** for Phase 2.0 (this patch release). Option A is more ambitious but the design doc explicitly named B as the fallback if "macros fight back" — they did. B preserves the seam (no SDK types leak into emit beyond the bridge crate's trait re-exports), keeps the plugin's value proposition (proto-typed trait surface + compile-time wire validation), and avoids declarative macros emitted from a proto plugin. The consumer-side cost is ~15 lines per service to wire `#[activity_definitions]` against the generated trait — documented once in the README on-ramp.

If Option A turns out to be worth the complexity, it lands as a Phase 2.1 patch (still inside `0.1.x`) without breaking B-style consumers — the macro is additive.

## Implications for the Phase 2 plan

If we adopt Option B:

1. **Plugin changes:** options parser learns `activities=true`; new `parse → validate → render` branches; emit a per-service `<Service>Activities` async trait + per-activity `<ACTIVITY_NAME>: &'static str` const. No facade types referenced. No registration fn.
2. **Bridge crate changes:** add a `worker` cargo feature that pulls in `temporalio-sdk = "=0.4.0"` and re-exports `ActivityContext`, `ActivityError`, `Worker`, plus the doc snippet for the `#[activity_definitions]` adapter pattern.
3. **Example:** new `activity_only` fixture; example crate gains a `worker` feature exercising the trait + macro-adapter pattern.
4. **CI:** new `verify-bridge` step adds `--features worker` to the cargo check matrix.
5. **PoC migration:** delete the hand-rolled `Activities` trait in `~/Development/job-queue`; rewrite the consumer's activity impl against the generated trait (no marker hand-rolling change, since the consumer already uses `#[activity_definitions]`).

If we adopt Option A, layer the macro on top of B's output and the consumer-side surface shrinks to `register_<service>_activity_impls!(MyImpl);` plus `worker.register_activities(MyImpl::new());`.

## Spike status

- ✅ Verified SDK shape against `temporalio-sdk 0.4.0` source under `~/.cargo/registry/`.
- ✅ Confirmed name-based dynamic registration is impossible without upstream changes.
- ✅ Confirmed `#[activity_definitions]` macro pattern is the SDK's intended ergonomics.
- ⏳ Not yet prototyped Option A or B end-to-end against the example crate.

The spike answers the design's risk question. The next concrete step is choosing A or B and writing the Phase 2 plan against that choice — neither of which I'll do without alignment, since this changes the design doc's emit sketch.
