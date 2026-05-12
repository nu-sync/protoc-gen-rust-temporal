# Phase 3 spike findings — Rust SDK workflow shape

**Date:** 2026-05-12.
**Triggered by:** the design's Risks table mitigation (same row as Phase 2):
> Phase 2 lead-in spike: prototype `register_<service>_activities` against `temporalio-sdk` 0.4 *before* committing to the trait shape in the implementation plan.

Phase 3 (workflows) hits an even more macro-driven SDK surface than activities. This doc captures the findings before the Phase 3 plan is written.

## What the design doc sketched

```rust
pub trait <Workflow>: Sized + Send + 'static {
    type Input;
    type Output;
    async fn run(self, ctx: WorkflowCtx, input: Self::Input) -> Result<Self::Output>;
    async fn on_<signal>(&mut self, ctx: &WorkflowCtx, input: <SigInput>) -> Result<()>;
    fn on_<query>(&self, ctx: &WorkflowCtx, input: <QInput>) -> Result<<QOutput>>;
    async fn on_<update>(&mut self, ctx: &WorkflowCtx, input: <UInput>) -> Result<<UOutput>>;
}

pub fn register_<service>_workflows(worker, constructors);

pub async fn <Workflow>::execute_child(
    ctx: &WorkflowCtx, input: <Input>, opts: <Workflow>ChildOptions,
) -> Result<<Output>>;
```

## What `temporalio-sdk 0.4` actually requires

The SDK's workflow registration trait (`temporalio-sdk 0.4` `src/workflows.rs:144`):

```rust
pub trait WorkflowImplementation: Sized + 'static {
    type Run: WorkflowDefinition;
    const HAS_INIT: bool;
    const INIT_TAKES_INPUT: bool;
    fn name() -> &'static str;
    fn init(ctx: WorkflowContextView, input: Option<...>) -> Self;
    fn run(ctx: WorkflowContext<Self>, input: Option<...>) -> LocalBoxFuture<'static, Result<Payload, WorkflowTermination>>;
    fn dispatch_signal(ctx, name, payloads, converter) -> Option<LocalBoxFuture<...>>;
    fn dispatch_query(&self, ctx, name, payloads, converter) -> Option<Result<Payload, _>>;
    fn dispatch_update(ctx, name, payloads, converter) -> Option<LocalBoxFuture<...>>;
    fn validate_update(&self, ctx, name, payloads, converter) -> Option<Result<(), _>>;
}
```

Plus the macros generate `WorkflowImplementer` (which calls `defs.register_workflow::<T>()`).

The macro pair `#[workflow]` + `#[workflow_methods]` plus per-method `#[run]`, `#[init]`, `#[signal]`, `#[query]`, `#[update]` is the SDK's ergonomic surface. They expand to:
1. The user's struct impls `WorkflowImplementation`
2. A nested `Run` marker type impls `WorkflowDefinition` with `name = #[run(name = "...")]`
3. Per-signal/query/update, generate the dispatcher arms inside the `dispatch_*` methods

## Known SDK shape quirks (already in `docs/sdk-shape.md`)

Already-documented:
1. Workflow registration name goes on `#[run(name = ...)]`, NOT on `#[workflow]`.
2. `#[workflow]` attribute is on the **struct**, not on an impl block.
3. `#[run]` takes NO user input parameter — input flows through `#[init]`.
4. Return type is `WorkflowResult<T>`, not `Result<T, WorkflowError>`.
5. `WorkflowContextView` is **not** generic; `WorkflowContext<W>` is.
6. Signal/query handler signatures take the deserialized input directly (not a wrapper).
7. `futures-util` is a required transitive dep (macro hygiene quirk).
8. Workflow struct must not be `pub` — use `pub(crate)` (the macro generates a crate-private `Run` type).

## Why the design's sketch doesn't compile (same fight-back as Phase 2)

`pub trait <Workflow>: Sized + Send + 'static` with method-level `async fn run(self, ctx, input)` cannot be wired to the SDK's `WorkflowImplementation` because:

- The SDK's `WorkflowImplementation::run` is a free function returning `LocalBoxFuture<'static, Result<Payload, WorkflowTermination>>` (Payload, not the typed `Output`). Serialization happens inside the macro-generated wrapper, not at the trait boundary.
- The SDK's `init` consumes `WorkflowContextView` (immutable view) and produces `Self`. The plugin's trait method `async fn run(self, ...)` doesn't expose a place for init.
- The signal/query/update dispatch functions are **typed by `Self::Run as WorkflowDefinition`** — the marker, not the user type. The marker comes from the `#[run]` macro expansion and is per-impl.

Same fundamental problem as activities: the SDK's static-dispatch model needs concrete types the plugin can't generate at codegen time.

## Options for Phase 3

### Option B (recommended, same as Phase 2)

**Plugin emits the trait surface only.** Per workflow, emit:

```rust
pub const RUN_BATCH_WORKFLOW_NAME: &str = "acts.v1.ChunkService/RunBatch";
pub const RUN_BATCH_TASK_QUEUE: &str = "chunks";

pub trait RunBatch: Sized + Send + 'static {
    fn run(
        self,
        ctx: temporal_runtime::WorkflowContext<Self>,
        input: BatchInput,
    ) -> impl ::std::future::Future<Output = Result<BatchOutput>> + Send;

    // For each attached signal/query/update ref, a handler:
    fn on_cancel(
        &mut self,
        ctx: &mut temporal_runtime::SyncWorkflowContext<Self>,
        input: CancelInput,
    );

    fn on_status(
        &self,
        ctx: &temporal_runtime::WorkflowContextView,
    ) -> StatusOutput;

    fn on_reconfigure(
        &mut self,
        ctx: temporal_runtime::WorkflowContext<Self>,
        input: ReconfigureInput,
    ) -> impl ::std::future::Future<Output = Result<ReconfigureOutput>> + Send;
}
```

Consumer writes (per workflow, ~30 LOC):

```rust
#[workflow(name = generated::RUN_BATCH_WORKFLOW_NAME)]
pub(crate) struct MyRunBatch { /* state */ }

#[workflow_methods]
impl MyRunBatch {
    #[init]
    fn new(_ctx: &WorkflowContextView, _input: BatchInput) -> Self {
        Self { /* … */ }
    }

    #[run]
    async fn run(ctx: WorkflowContext<Self>) -> WorkflowResult<BatchOutput> {
        // call into the trait via ctx.state(...).run(ctx, input)
        // (the adapter shape needs experimentation; the trait method takes
        //  `self` by value, the SDK's run does not — there's a wrapper to write)
    }

    #[signal(name = "Cancel")]
    fn cancel(&mut self, ctx: &mut SyncWorkflowContext<Self>, input: CancelInput) {
        generated::RunBatch::on_cancel(self, ctx, input);
    }

    // …queries, updates, …
}
```

**Open complexity:** The trait's `run(self, ctx, input)` consumes Self. The SDK's `run` takes `WorkflowContext<Self>` (which already holds Self via Rc<RefCell>). Wiring "consume self" to "borrow from RefCell" requires careful design. The adapter probably needs the trait's `run` to take `&mut self` (or a different shape), or the trait shape needs to skip `run` and only emit handler-method names — letting the user write `run` directly in the macro.

### Option C (simpler, less DRY)

Plugin emits **only** name consts + a struct of type tags (one per workflow):

```rust
pub const RUN_BATCH_WORKFLOW_NAME: &str = "acts.v1.ChunkService/RunBatch";
pub const RUN_BATCH_TASK_QUEUE: &str = "chunks";

// Per-signal/query/update name consts on the workflow:
pub mod run_batch {
    pub const CANCEL_SIGNAL_NAME: &str = "Cancel";
    pub const STATUS_QUERY_NAME: &str = "Status";
    pub const RECONFIGURE_UPDATE_NAME: &str = "Reconfigure";
}
```

No trait emit. Consumer hand-writes the entire `#[workflow]` setup, references the name consts to keep registration in sync with the proto. This is what cludden-parity-design.md's *fallback* hints at, ratcheted down further than Phase 2's trait emit because workflow trait shape has unresolved consume-self questions.

Pros: zero design risk; deeply simple plugin; works against any future SDK shape.
Cons: the plugin's value drops — at the bare minimum a few name consts. No type-checked handler signatures.

### Option A (declarative macro emit, same as Phase 2's deferred alternative)

Plugin emits a macro the consumer invokes:

```rust
generated::workflow_impls!(MyRunBatch, MyImpl::run, MyImpl::on_cancel, ...);
```

The macro generates the full `#[workflow]` + `#[workflow_methods]` setup wired to the user's `MyImpl` methods. Heaviest plugin lift, lightest consumer surface. Deferred per Phase 2's same recommendation.

## Recommendation

Looking at the Phase 2 trait-only approach against the consume-self question:

- The Phase 2 trait was straightforward (`fn process(&self, ctx, input) -> impl Future<...> + Send`) because activities are stateless from the SDK's perspective — the `Implementer` is the user's state struct, the activity method just calls a method on it.
- Phase 3 workflows are stateful — the workflow IS the state. `run(self, ctx, input)` consumes Self because the SDK's `init` produces the Self that `run` then uses. The trait method's `self` by value is the natural fit.

**Recommendation: Option C** for Phase 3.0, with Phase 3.1 reserved for an Option B retry once the consume-self adapter shape is prototyped end-to-end against the actual SDK macros.

This is **more conservative** than Phase 2 because:
- The Phase 3 trait surface has more shape questions than Phase 2 (`self` vs `&mut self` on handlers; how the trait's `run` cooperates with the SDK macro's `run`; `WorkflowContext<Self>` vs `WorkflowContext<MyImpl>` — the trait's `Self` is the trait impl, the macro's `Self` is the workflow struct, and those may or may not be the same type).
- An incorrect trait surface in Phase 3 would force two breaking changes: one to fix the trait, one to migrate consumer code.
- Option C ships name consts immediately (real value: keeps registration names in sync with the proto). Trait emit can land in Phase 3.1 after a real-world prototype.

## Implications

If Option C is chosen:

1. **Plugin emit:** only constants — `<WORKFLOW>_WORKFLOW_NAME`, `<WORKFLOW>_TASK_QUEUE` (already emitted today as `<RPC>_WORKFLOW_NAME` + `<RPC>_TASK_QUEUE`), and per attached ref: `<workflow>::<SIGNAL>_SIGNAL_NAME`, etc.
2. **No new trait surface.** The `workflows=true` plugin flag enables only the per-ref name consts (the workflow-level consts already emit unconditionally).
3. **Bridge crate:** no new feature needed — the `worker` feature already re-exports `Worker::register_workflow`. The bridge crate could optionally add `WorkflowContext`, `WorkflowContextView`, `SyncWorkflowContext`, `WorkflowResult`, `WorkflowError` re-exports for consumer convenience.
4. **PoC migration:** smaller win — consumer keeps their hand-rolled `#[workflow]` setup but references the generated name consts instead of string literals. Maybe 10 LOC removed per workflow.

If Option B is chosen (deferred to Phase 3.1):

1. Plugin emits a per-workflow trait. The trait's `run` signature is **TBD** pending an end-to-end adapter prototype that demonstrates wiring through `#[workflow]`. Until that prototype exists, the trait shape is a guess.
2. New bridge re-exports: `WorkflowContext<W>`, `WorkflowContextView`, `SyncWorkflowContext<W>`, `WorkflowResult`, `WorkflowError`, plus probably `WorkflowImplementation` and `WorkflowImplementer`.
3. PoC migration: workflow trait method bodies are reusable across `#[workflow]` impls. Bigger win, but only after prototype confirms feasibility.

## Spike status

- ✅ Verified SDK shape against `temporalio-sdk 0.4.0` source.
- ✅ Confirmed `WorkflowImplementation` is macro-generated; static-dispatch only.
- ✅ Confirmed Phase 2's Option B repeats are not directly applicable due to the consume-self trait shape question.
- ⏳ End-to-end adapter prototype between a trait shape and the `#[workflow]` macro: **not done**. Would take its own spike session.

## Recommendation summary

| Phase 3 cut | Plugin emit | Bridge changes | Consumer code change | Design risk |
|---|---|---|---|---|
| **3.0 — Option C** | Name consts only | Optional convenience re-exports | Hand-rolled `#[workflow]` referencing generated consts | Low |
| **3.1 — Option B** | Workflow trait | New WF context/result re-exports | Trait impl + thin `#[workflow]` adapter | Medium-high (needs adapter prototype) |
| **3.2 — Option A** | Trait + invocation macro | Same as 3.1 | One macro line per workflow + method bodies | High (declarative macros from proto plugin are unusual) |

Phase 3.0 ships immediate value (string-literal elimination) without locking in a trait shape. Phase 3.1 is a fast-follow once an adapter prototype lands. Phase 3.2 is an opt-in ergonomic top layer.

Same recommendation as Phase 2 spike: cut what's safe now, defer the higher-design-risk pieces to follow-up patch releases inside `0.1.x`.
