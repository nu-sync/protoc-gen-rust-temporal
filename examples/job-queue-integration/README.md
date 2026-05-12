# Example: job-queue consumer

This directory shows the **canonical consumer setup** for
`protoc-gen-rust-temporal`. It is the smallest possible Rust crate that
takes a cludden-annotated proto and ends up with a typed
`JobServiceClient` — no Temporal SDK boot, no real worker, just the wiring
the plugin expects.

The actual end-to-end demo lives at
[`/Users/wcygan/Development/job-queue`](../../../job-queue) (the PoC); this
example is the **published-plugin** form of that wiring, the one Phase 5 of
the [main SPEC](../../SPEC.md) migrates job-queue onto.

## Layout

```
job-queue-integration/
├── Cargo.toml
├── buf.gen.yaml
├── build.rs                 # invokes `buf generate` for the plugin (optional)
├── proto/
│   └── jobs/v1/
│       └── jobs.proto       # cludden-annotated service
└── src/
    ├── lib.rs               # `include!`s the plugin output + re-exports the runtime facade
    ├── jobs/v1/mod.rs       # prost output for jobs.v1.* messages
    └── temporal_runtime.rs  # consumer-supplied bridge — see below
```

## buf.gen.yaml shape

```yaml
version: v2
plugins:
  # Headline: BSR remote plugin. Requires no local install.
  - remote: buf.build/nu-sync/rust-temporal
    out: src/gen

  # Fallback: locally-installed binary. Useful while iterating on the plugin.
  # - local: protoc-gen-rust-temporal
  #   out: src/gen
```

## Cargo.toml shape

```toml
[dependencies]
anyhow = "1"
prost = "0.13"
prost-types = "0.13"
temporal-proto-runtime = "0.0"
# plus whatever you pin for the Temporal SDK:
# temporalio-client = "0.4"
# temporalio-sdk    = "0.4"
```

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

pub async fn update_with_start_workflow_proto<U: TemporalProtoMessage, O: TemporalProtoMessage>(
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
