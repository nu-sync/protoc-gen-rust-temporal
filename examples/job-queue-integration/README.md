# Example: job-queue consumer

The smallest Rust crate that takes a cludden-annotated proto and produces a
typed `JobServiceClient` — no Temporal SDK boot, no real worker, just the
wiring `protoc-gen-rust-temporal` expects.

The actual end-to-end demo lives at
[`/Users/wcygan/Development/job-queue`](../../../job-queue) (the PoC); this
crate is the **published-plugin** form of that wiring, the one Phase 5 of
the [main SPEC](../../SPEC.md) migrates job-queue onto.

## Run it locally

The facade in `src/temporal_runtime.rs` is intentionally stubbed with
`todo!()` bodies, so "running" the example here means **proving the
wiring compiles** end-to-end against both (a) the workspace's path-dep
runtime crate and (b) the published `temporal-proto-runtime` on
crates.io. Anything beyond that needs a real Temporal Server and a real
SDK facade (see `temporal_runtime` below).

Prereqs:

- Rust 1.88+ (`rust-toolchain.toml` pins this; `rustup` auto-installs).
- `protoc` on `PATH` (for `regen` / `regen-check`).
- [`just`](https://just.systems) on `PATH` (for the recipes below).

### Step-by-step

```bash
cd examples/job-queue-integration

# 1. List recipes.
just

# 2. Compile against the workspace path-dep (fast, ~1s after first build).
just build

# 3. CI-equivalent: fmt + clippy + build.
just all

# 4. Verify the checked-in snapshot at src/gen/ matches what the
#    plugin would emit right now. Fails if drift exists.
just regen-check

# 5. Verify the example compiles against the *published* runtime crate
#    (downloads `temporal-proto-runtime` from crates.io into a temp dir,
#    swaps the path dep, vendors the annotation schema, and builds).
just verify-published
```

If all five pass, the example is healthy.

### Other recipes

| Recipe | What it does |
|---|---|
| `just check` | `cargo check` — fastest type-check |
| `just clippy` | Same flags as CI (`-D warnings`) |
| `just fmt` / `fmt-check` | Format / verify formatting |
| `just regen` | Rebuild the snapshot at `src/gen/` from the current plugin source |
| `just clean` | `cargo clean -p job-queue-integration-example` |

`just regen` is what you run after editing the proto or changing the
plugin's emit; commit the resulting diff so `just regen-check` stays
green in CI.

## Layout

```
job-queue-integration/
├── Cargo.toml
├── justfile                            # local-testing recipes
├── buf.gen.yaml
├── build.rs                            # prost-build for jobs.v1.* messages
├── proto/jobs/v1/jobs.proto            # cludden-annotated service
└── src/
    ├── lib.rs                          # include!s the plugin output + re-exports the runtime facade
    ├── temporal_runtime.rs             # consumer-supplied bridge — see below
    └── gen/jobs/v1/jobs_temporal.rs    # checked-in plugin output (reference snapshot)
```

The `src/gen/jobs/v1/jobs_temporal.rs` file is **the actual output** of
`protoc-gen-rust-temporal` against the example's `jobs.proto`, checked
in as a documentation artifact so you can read what the plugin emits
without running it. `just regen` regenerates it.

## buf.gen.yaml shape

```yaml
version: v2
plugins:
  # Headline (not yet published): BSR remote plugin.
  # - remote: buf.build/nu-sync/rust-temporal
  #   out: src/gen

  # Current path: locally-installed binary built by `just regen`.
  - local: protoc-gen-rust-temporal
    out: src/gen
```

The BSR remote plugin (`buf.build/nu-sync/rust-temporal`) is not yet
published; use the local form above (or invoke `protoc` directly, as
`just regen` does) until it lands.

## Cargo.toml shape (for a downstream consumer)

```toml
[dependencies]
anyhow = "1"
prost = "0.13"
prost-types = "0.13"
# `sdk` feature pulls `temporalio-common` and emits the
# TemporalSerializable / TemporalDeserializable impls on
# TypedProtoMessage<T>. Enable this once you replace the stub bodies
# in temporal_runtime.rs with real SDK calls.
temporal-proto-runtime = { version = "0.1", features = ["sdk"] }
# Plus whatever you pin for the Temporal SDK proper:
# temporalio-client = "0.4"
# temporalio-sdk    = "0.4"
```

This example itself uses a workspace path-dep (without the `sdk`
feature) because its facade is stubbed; `just verify-published`
exercises the registry-version path.

## Vendoring the annotation schema

`proto/jobs/v1/jobs.proto` imports `temporal/v1/temporal.proto`, so
`prost-build` needs that file on its include path during parse. The
published `protoc-gen-rust-temporal` crate does not ship it as a
fetchable proto artifact — copy
`crates/protoc-gen-rust-temporal/proto/temporal/v1/temporal.proto`
(and its transitive `temporal/api/enums/v1/workflow.proto`) into your
own consumer crate's proto tree.

This example's `build.rs` reaches across the workspace at
`../../crates/protoc-gen-rust-temporal/proto` to do that; outside this
repo you'd vendor the file once and point `prost-build` at the local
copy (which is what `just verify-published` does internally).

## The `temporal_runtime` facade — what the consumer wires up

Generated code calls into `crate::temporal_runtime::*`. The plugin does not
ship that module — the consumer does — so generated output stays stable
across upstream SDK churn. The required surface is small:

```rust
// src/temporal_runtime.rs
pub struct TemporalClient { /* your SDK client handle */ }
pub struct WorkflowHandle { /* your SDK workflow handle */ }

pub enum WorkflowIdReusePolicy { AllowDuplicate, AllowDuplicateFailedOnly,
                                  RejectDuplicate, TerminateIfRunning }
pub enum WaitPolicy { Admitted, Accepted, Completed }

pub fn attach_handle(client: &TemporalClient, workflow_id: String) -> WorkflowHandle;
pub fn random_workflow_id() -> String;
pub fn eval_id_expression(expr: &str) -> String;

pub async fn start_workflow_proto<I: TemporalProtoMessage>(
    client: &TemporalClient,
    workflow_name: &'static str,
    workflow_id: &str,
    task_queue: &str,
    input: &I,
    id_reuse_policy: Option<WorkflowIdReusePolicy>,
    execution_timeout: Option<std::time::Duration>,
    run_timeout: Option<std::time::Duration>,
    task_timeout: Option<std::time::Duration>,
) -> anyhow::Result<WorkflowHandle>;

// Empty-input variant — emitted for workflows whose input is
// `google.protobuf.Empty`. Same as above without the payload arg.
pub async fn start_workflow_proto_empty(
    client: &TemporalClient,
    workflow_name: &'static str,
    workflow_id: &str,
    task_queue: &str,
    id_reuse_policy: Option<WorkflowIdReusePolicy>,
    execution_timeout: Option<std::time::Duration>,
    run_timeout: Option<std::time::Duration>,
    task_timeout: Option<std::time::Duration>,
) -> anyhow::Result<WorkflowHandle>;

pub async fn wait_result_proto<O: TemporalProtoMessage>(
    handle: &WorkflowHandle,
) -> anyhow::Result<O>;
pub async fn wait_result_unit(handle: &WorkflowHandle) -> anyhow::Result<()>;

pub async fn signal_proto<I: TemporalProtoMessage>(
    handle: &WorkflowHandle, signal_name: &str, input: &I,
) -> anyhow::Result<()>;
pub async fn signal_unit(handle: &WorkflowHandle, signal_name: &str) -> anyhow::Result<()>;

pub async fn query_proto<I: TemporalProtoMessage, O: TemporalProtoMessage>(
    handle: &WorkflowHandle, query_name: &str, input: &I,
) -> anyhow::Result<O>;
pub async fn query_proto_empty<O: TemporalProtoMessage>(
    handle: &WorkflowHandle, query_name: &str,
) -> anyhow::Result<O>;

pub async fn update_proto<I: TemporalProtoMessage, O: TemporalProtoMessage>(
    handle: &WorkflowHandle, update_name: &str, input: &I, wait_policy: WaitPolicy,
) -> anyhow::Result<O>;
pub async fn update_proto_empty<O: TemporalProtoMessage>(
    handle: &WorkflowHandle, update_name: &str, wait_policy: WaitPolicy,
) -> anyhow::Result<O>;

pub async fn signal_with_start_workflow_proto<W: TemporalProtoMessage, S: TemporalProtoMessage>(
    client: &TemporalClient,
    workflow_name: &'static str,
    workflow_id: &str,
    task_queue: &str,
    workflow_input: &W,
    signal_name: &str,
    signal_input: &S,
    id_reuse_policy: Option<WorkflowIdReusePolicy>,
    execution_timeout: Option<std::time::Duration>,
    run_timeout: Option<std::time::Duration>,
    task_timeout: Option<std::time::Duration>,
) -> anyhow::Result<WorkflowHandle>;

pub async fn update_with_start_workflow_proto<
    W: TemporalProtoMessage,    // workflow input
    U: TemporalProtoMessage,    // update input
    O: TemporalProtoMessage,    // update output
>(
    /* same start args, plus update_name + update_input + wait_policy */
) -> anyhow::Result<(WorkflowHandle, O)>;
```

The PoC's
[`crates/jobs-proto/src/temporal_runtime.rs`](../../../job-queue/crates/jobs-proto/src/temporal_runtime.rs)
is the reference implementation against `temporalio-client` 0.4.

## Why a consumer-owned facade

Two reasons:

1. **SDK version isolation.** `temporalio-sdk` is pre-1.0 and reshapes
   frequently. Pinning the SDK call sites inside the generated module
   would force a plugin release every time the SDK moves an import path.
   Pushing them into a consumer-owned module isolates the churn.
2. **Test substitutability.** A consumer can stub the facade for unit
   tests without bringing up a Temporal Server. The PoC's tests do this.

The runtime crate (`temporal-proto-runtime`) ships only the wire-format
trait + wrapper, not the SDK call sites — that's intentional.
