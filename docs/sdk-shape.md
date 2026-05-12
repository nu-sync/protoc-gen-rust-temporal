# Pinned Temporal Rust SDK shape

**Verified against:**
- `temporalio-sdk` 0.4.0
- `temporalio-macros` 0.4.0
- `temporalio-common` 0.4.0
- `temporalio-client` 0.4.0
- `temporalio-sdk-core` 0.4.0
- Rust toolchain: stable (≥1.94 at time of verification — `temporalio-sdk-core` requires Rust ≥1.88)

Reference example (from the SDK's own test code):
`temporalio-sdk-0.4.0/src/lib.rs` around line 1413.

## Significant deviations from the original design spec

The 2026-05-11 design spec describes shapes from a hypothesized alpha (`0.1.0-alpha.1`). The current released 0.4.0 differs in important ways. Use this file as the canonical reference; treat the spec's Rust code samples as illustrative.

### 0. Workflow registration name goes on `#[run]`, NOT `#[workflow]`

`temporalio-macros` 0.4.0 reads the cross-language registration name from
`#[run(name = "...")]`. The corresponding `name = "..."` on `#[workflow]`
is silently ignored — the workflow registers under its Rust impl type name
instead, and a TS-side `client.workflow.start("email.v1.EmailService/...")`
will fail with "Workflow type ... not found" at runtime.

```rust
// CORRECT
#[workflow]
pub(crate) struct SendStripeReceipt { /* ... */ }

#[workflow_methods]
impl SendStripeReceipt {
    #[run(name = "email.v1.EmailService/SendStripeReceipt")]
    async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<MyOutput> { /* ... */ }
}
```

(The spec showed this on `#[workflow]`; only `#[signal(name = ...)]` and
`#[query(name = ...)]` take the name attribute as the spec described — the
workflow registration name is the outlier.)

### 1. `#[workflow]` attribute goes on the struct, not on an impl block

```rust
// CORRECT
#[workflow(name = "my.v1.MyWorkflow/Run")]
struct MyWorkflow {
    counter: u32,
}

#[workflow_methods]
impl MyWorkflow { /* ... */ }
```

The spec showed `#[workflow] impl X {}` — that does not compile.

### 2. `#[run]` takes NO user input parameter; input flows through `#[init]`

```rust
// CORRECT
#[init]
fn new(_ctx: &WorkflowContextView, input: MyInput) -> Self { /* ... */ }

#[run]
async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<MyOutput> {
    Ok(/* ... */)
}
```

The spec showed `async fn run(ctx, _input: T)` — the input arg goes on `#[init]`, not on `#[run]`. The `#[init]` receives the deserialized input and constructs `Self`; the run fn reads it via `ctx.state(...)`.

### 3. Return type is `WorkflowResult<T>`, not `Result<T, WorkflowError>`

```rust
pub type WorkflowResult<T> = Result<T, WorkflowTermination>;
```

The spec's `Result<T, WorkflowError>` is wrong. `WorkflowError` exists at `temporalio_sdk::workflows::WorkflowError` but is not the run-fn return error type.

### 4. `WorkflowContextView` is not generic

```rust
pub struct WorkflowContextView { /* no type params */ }
```

The spec's `WorkflowContextView<Self>` does not compile. Use bare `WorkflowContextView`.

`SyncWorkflowContext<W>` and `WorkflowContext<W>` DO take a single type parameter (the workflow type).

### 5. Signal/query handler signatures

```rust
// signal — third arg is the deserialized input directly (no wrapper type)
#[signal(name = "increment")]
fn increment_counter(&mut self, _ctx: &mut SyncWorkflowContext<Self>, amount: u32) { /* ... */ }

// query — second arg is &WorkflowContextView (no generic)
#[query]
fn get_counter(&self, _ctx: &WorkflowContextView) -> u32 { self.counter }
```

The spec wrapped signal/query inputs in `TypedProtoMessage<T>` for the data converter. With the SDK's 0.4.0 trait shape that wrapping must be inlined through a different layer (TBD when we wire the actual demo workflow).

### 6. `futures-util` is a required transitive dep

`#[workflow_methods]` expands to code that calls `.boxed_local()` from `futures_util::future::FutureExt`. The macro doesn't re-export the trait, so consuming crates must:

```toml
[dependencies]
futures-util = "0.3"
```

```rust
#[allow(unused_imports)]
use futures_util::FutureExt as _;
```

This is a macro hygiene quirk in `temporalio-macros` 0.4.0.

### 7. Workflow struct must not be `pub`

```rust
// CORRECT
#[workflow(name = "...")]
pub(crate) struct MyWorkflow { /* ... */ }
```

The `#[workflow_methods]` macro generates an internal `Run` type which is crate-private; making the workflow struct `pub` triggers `E0446: crate-private type Run in public interface`. The SDK's own example uses bare `struct` (private to the test module). For cross-module use within the worker crate, `pub(crate)` is the maximum visibility that compiles.

## Verified compile shape (probe)

See `crates/worker/src/lib.rs` at the SDK-shape-spike commit for the minimal compiling example.

## Plan-level impact

These findings invalidate parts of the spec/plan:

- **Spec §7 (Rust worker)** — workflow input flows through `#[init]`, not `#[run]`. Demo workflow needs `#[init]` that takes `SendStripeReceiptInput` and stores fields on `Self`.
- **Spec §5 (Rust data converter)** — `TypedProtoMessage<T>` wrapper still works for `to_payload`/`from_payload`, but workflows/activities should consume the unwrapped proto type directly. The data converter glue may need to operate at a different level than the spec implied. Defer this decision until Task 13 (data converter implementation); revisit then with `TemporalSerializable` source open.
- **Plan Task 14 (workflow code)** — rewrite the workflow to match shapes 1, 2, 3, 5 above.

These notes are the canonical truth where they conflict with the spec or plan.
