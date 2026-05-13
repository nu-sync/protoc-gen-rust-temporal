# Runtime API contract

The plugin-generated client surface calls into a consumer-supplied
`crate::temporal_runtime` module. This document enumerates **every
function the generated code calls**, when it gets emitted, and the
signature it expects. If a function is missing from the consumer's
facade, the generated code will fail to compile with a clear error
pointing at the call site.

**Default implementation:** [`temporal-proto-runtime-bridge`](../crates/temporal-proto-runtime-bridge/)
ships a concrete impl of every function documented below, backed by
`temporalio-client 0.4`. Add it as a dep and `pub use temporal_proto_runtime_bridge as temporal_runtime;`
in your `lib.rs` to wire the plugin's generated code to the real SDK
without writing the facade yourself. The primary example at
[`examples/job-queue`](../examples/job-queue/) uses that bridge from a
real worker, HTTP API, and CLI. Power users who need to substitute a
custom transport, vendored SDK, or test stub can provide their own
`crate::temporal_runtime` module against the contract below.

## Versioning

This document is **pinned to a plugin version**. Each row notes the
plugin version in which the call site was introduced; nothing here is
removed without a major bump (post-1.0) or a deprecation cycle (pre-1.0).

Current pin: **protoc-gen-rust-temporal 0.1.1**.

## Roadmap relationship

This document describes only the runtime symbols emitted today. The active
project roadmap is majority parity with `protoc-gen-go-temporal`, which will
add facade symbols for worker implementation helpers, workflow-side activity
execution, broader client operations, and runtime option coverage. Each new
emitted symbol must be added here in the same change that introduces it. See
[`ROADMAP.md`](../ROADMAP.md) for priority order and current unsupported
features.

## Types

The facade must define (or re-export) the following types. Generated
code references them by their unqualified name through
`crate::temporal_runtime::*`.

| Type | Role |
|---|---|
| `TemporalClient` | Opaque handle on the Temporal client connection. Constructed by the consumer; the plugin only forwards `&TemporalClient` to runtime functions. |
| `WorkflowHandle` | Opaque handle on a running / attachable workflow execution. Must expose `workflow_id(&self) -> &str`. |
| `WorkflowIdReusePolicy` | Enum with variants `AllowDuplicate`, `AllowDuplicateFailedOnly`, `RejectDuplicate`, `TerminateIfRunning`. Matches cludden's `IDReusePolicy`. |
| `WaitPolicy` | Enum with variants `Admitted`, `Accepted`, `Completed`. Used by update calls. |
| `TemporalProtoMessage` | Re-exported from [`temporal-proto-runtime`](https://crates.io/crates/temporal-proto-runtime). The plugin emits `impl temporal_runtime::TemporalProtoMessage for <Ty>` for every prost message type the client surface touches; consumers do not write these by hand. |
| `TypedProtoMessage<T>` | Re-exported from [`temporal-proto-runtime`](https://crates.io/crates/temporal-proto-runtime). Enable that crate's `sdk` feature to get the `TemporalSerializable` / `TemporalDeserializable` impls — otherwise the orphan rule blocks consumers from adding them. |

## Functions

Indented functions are called *only* when the listed annotation triggers
them. Otherwise every function is unconditionally referenced from any
service that has at least one workflow.

| Function | Trigger | Signature | Since |
|---|---|---|---|
| `attach_handle` | every workflow (used by `<rpc>_handle`) | `fn(client: &TemporalClient, workflow_id: String) -> WorkflowHandle` | 0.1.0 |
| `random_workflow_id` | a workflow with **no** proto-level `id` template | `fn() -> String` | 0.1.0 |
| `start_workflow_proto<I>` | workflow start, **non-Empty** input | `async fn(client, workflow_name: &'static str, workflow_id: &str, task_queue: &str, input: &I, id_reuse_policy: Option<WorkflowIdReusePolicy>, execution_timeout: Option<Duration>, run_timeout: Option<Duration>, task_timeout: Option<Duration>) -> Result<WorkflowHandle>` where `I: TemporalProtoMessage` | 0.1.0 |
| `start_workflow_proto_empty` | workflow start, **Empty** input | same as above without the `input` arg or `I` generic | 0.1.0 |
| `wait_result_proto<O>` | workflow handle `result()`, **non-Empty** output | `async fn(&WorkflowHandle) -> Result<O>` where `O: TemporalProtoMessage` | 0.1.0 |
| `wait_result_unit` | workflow handle `result()`, **Empty** output | `async fn(&WorkflowHandle) -> Result<()>` | 0.1.0 |
| `signal_proto<I>` | `handle.<signal>()` for signals with **non-Empty** input | `async fn(&WorkflowHandle, signal_name: &str, input: &I) -> Result<()>` | 0.1.0 |
| `signal_unit` | `handle.<signal>()` for signals with **Empty** input | `async fn(&WorkflowHandle, signal_name: &str) -> Result<()>` | 0.1.0 |
| `query_proto<I, O>` | `handle.<query>()` with **non-Empty** input and **non-Empty** output | `async fn(&WorkflowHandle, query_name: &str, input: &I) -> Result<O>` where both: `TemporalProtoMessage` | 0.1.0 |
| `query_proto_empty<O>` | `handle.<query>()` with **Empty** input, **non-Empty** output | `async fn(&WorkflowHandle, query_name: &str) -> Result<O>` where `O: TemporalProtoMessage` | 0.1.0 |
| `query_unit<I>` | `handle.<query>()` with **non-Empty** input, **Empty** output | `async fn(&WorkflowHandle, query_name: &str, input: &I) -> Result<()>` where `I: TemporalProtoMessage` | 0.1.1 |
| `query_proto_empty_unit` | `handle.<query>()` with **Empty** input **and** **Empty** output | `async fn(&WorkflowHandle, query_name: &str) -> Result<()>` | 0.1.1 |
| `update_proto<I, O>` | `handle.<update>()` with **non-Empty** input and **non-Empty** output | `async fn(&WorkflowHandle, update_name: &str, input: &I, wait_policy: WaitPolicy) -> Result<O>` | 0.1.0 |
| `update_proto_empty<O>` | `handle.<update>()` with **Empty** input, **non-Empty** output | `async fn(&WorkflowHandle, update_name: &str, wait_policy: WaitPolicy) -> Result<O>` | 0.1.0 |
| `update_unit<I>` | `handle.<update>()` with **non-Empty** input, **Empty** output | `async fn(&WorkflowHandle, update_name: &str, input: &I, wait_policy: WaitPolicy) -> Result<()>` where `I: TemporalProtoMessage` | 0.1.1 |
| `update_proto_empty_unit` | `handle.<update>()` with **Empty** input **and** **Empty** output | `async fn(&WorkflowHandle, update_name: &str, wait_policy: WaitPolicy) -> Result<()>` | 0.1.1 |
| `signal_with_start_workflow_proto<W, S>` | `<signal>_with_start` free fn (signal has `start: true`) | `async fn(client, workflow_name, workflow_id, task_queue, workflow_input: &W, signal_name, signal_input: &S, …) -> Result<WorkflowHandle>` | 0.1.0 |
| `update_with_start_workflow_proto<W, U, O>` | `<update>_with_start` free fn, **non-Empty** update output | `async fn(client, …, workflow_input: &W, update_name, update_input: &U, wait_policy, …) -> Result<(WorkflowHandle, O)>` | 0.1.0 |
| `update_with_start_workflow_proto_unit<W, U>` | `<update>_with_start` free fn, **Empty** update output | `async fn(client, …, workflow_input: &W, update_name, update_input: &U, wait_policy, …) -> Result<WorkflowHandle>` | 0.1.1 |

### Removed since 0.0.x

| Function | Removed in | Replacement |
|---|---|---|
| `eval_id_expression(template: &str) -> String` | 0.1.0 | Plugin now materialises the `id` template into a private `<rpc>_id(input: &Input) -> String` function alongside the start method. Generated code calls `<rpc>_id(&input)` directly; the runtime no longer sees the template string. Consumers can delete any local `eval_id_expression` they had. |

## `Empty`-input contract for `_empty` variants

The plugin emits `start_workflow_proto_empty` / `signal_unit` /
`query_proto_empty` / `query_unit` / `query_proto_empty_unit` /
`update_proto_empty` / `update_unit` / `update_proto_empty_unit` /
`wait_result_unit` / `update_with_start_workflow_proto_unit` whenever a
workflow / signal / query / update has a `google.protobuf.Empty` input or
output. Those variants exist purely so the generated call site doesn't need
to express `()` as a `TemporalProtoMessage` — they do **not** mean "send no
payload."

A correct bridge implementation MUST encode the wire-format triple from
[`WIRE-FORMAT.md`](../WIRE-FORMAT.md) on every Empty boundary:

| Slot                | Value                                |
|---------------------|--------------------------------------|
| `metadata.encoding` | `"binary/protobuf"`                  |
| `metadata.messageType` | `"google.protobuf.Empty"`         |
| `data`              | `[]` (Empty has zero wire bytes)     |

This is what cludden's Go SDK `ProtoPayloadConverter` produces for an
`Empty` message and what mixed-language workflows expect to see on the
wire. Sending a payload-less `RawValue` (i.e. `vec![]` with no `Payload`
inside) looks like "no input" on the wire, which silently breaks
Go ↔ Rust interop. The default bridge crate
(`temporal-proto-runtime-bridge`) implements this — see
`encode_empty_payload` and the
`empty_payload_carries_the_full_triple` regression test.

## Note on `Empty` inputs and `_with_start`

`signal_with_start` and `update_with_start` free functions take **both**
the workflow input and the signal/update input. Supporting any of those
inputs being `google.protobuf.Empty` would require a combinatorial
explosion of runtime functions (`signal_with_start_proto_empty_workflow`,
`signal_with_start_proto_empty_signal`, …) which is not worth it for
v1.

The plugin's validate step **rejects** workflows that combine
`signal: [{ start: true }]` or `update: [{ start: true }]` with any
Empty **input** side, asking users to wrap the empty payload in a
single-field message instead. So in practice, generated code never calls
`signal_with_start_workflow_proto` or
`update_with_start_workflow_proto{,_unit}` with `&()` workflow-input or
signal/update-input arguments.

Empty **update outputs** on `_with_start` are supported — render dispatches
to the `_unit` variant, which validates the canonical Empty payload server-
side. The typed variant can't be reused because `()` does not implement
`TemporalProtoMessage` and so cannot satisfy the `O` generic.

## Phase 2 — Activities (opt-in via `activities=true`)

When invoked with `--rust-temporal_opt=activities=true`, the plugin emits, per
service with activity-annotated methods:

| Symbol | Shape |
|---|---|
| `<METHOD>_ACTIVITY_NAME` | `pub const &'static str` per annotated activity. Value matches what the activity is registered under server-side (defaults to the rpc method name). |
| `<Service>Activities` | `pub trait <Service>Activities: Send + Sync + 'static` with one method per activity. Signature: `fn <method>(&self, ctx: temporal_runtime::ActivityContext, input: <Input>) -> impl Future<Output = Result<<Output>>> + Send`. |
| `register_<service>_activities<I>` | `pub fn(&mut temporal_runtime::worker::Worker, I) -> &mut temporal_runtime::worker::Worker` where `I: <Service>Activities + temporal_runtime::worker::ActivityImplementer`. Delegates to `worker.register_activities(impl_)`. |

The trait method takes `&self`. The consumer's adapter (which wires the trait
to a `temporalio-sdk` Worker via `#[activities]`) does the SDK marker
generation. The generated register helper intentionally requires both the
generated trait and the SDK macro-produced `ActivityImplementer`, so the
compiler checks the proto-shaped trait and the SDK registration shape at the
same call site. See the `temporal-proto-runtime-bridge` README for the
adapter pattern.

Required runtime symbols (only when `activities=true` is set):

| Symbol | Provided by | Notes |
|---|---|---|
| `temporal_runtime::ActivityContext` | bridge crate `worker` feature | Re-exported from `temporalio_sdk::activities::ActivityContext`. |
| `temporal_runtime::worker::Worker` | bridge crate `worker` feature | Re-exported from `temporalio_sdk::Worker`. |
| `temporal_runtime::worker::ActivityImplementer` | bridge crate `worker` feature | Re-exported from `temporalio_sdk::activities::ActivityImplementer`. |

## Phase 3.0 — Workflow contracts (opt-in via `workflows=true`)

When invoked with `--rust-temporal_opt=workflows=true`, the plugin emits, per
service with at least one workflow rpc:

| Symbol | Shape |
|---|---|
| `<METHOD>_SIGNAL_NAME` | `pub const &'static str` per signal-annotated rpc. Value is the cross-language registration name (defaults to the rpc method name). |
| `<METHOD>_QUERY_NAME` | Same shape, for query-annotated rpcs. |
| `<METHOD>_UPDATE_NAME` | Same shape, for update-annotated rpcs. |
| `<Workflow>Definition` | `pub trait` with associated `Input` / `Output` types and default associated consts for `WORKFLOW_NAME`, `TASK_QUEUE`, and attached signal/query/update names. Consumers implement this trait on their SDK `#[workflow]` struct. |
| `register_<workflow>_workflow<W>` | `pub fn(&mut temporal_runtime::worker::Worker) -> &mut temporal_runtime::worker::Worker` where `W: temporal_runtime::worker::WorkflowImplementer + <Workflow>Definition<Input = <Input>, Output = <Output>>`. Delegates to `worker.register_workflow::<W>()`. |

The consumer still owns the `temporalio-sdk` `#[workflow]` /
`#[workflow_methods]` body. The generated trait does not define `run`,
signal, query, or update methods because the SDK's macro-generated
`WorkflowImplementation` shape is static and type-specific. The trait is a
proto contract for names and input/output types, and the register helper ties
that contract to the SDK's `WorkflowImplementer` at compile time.

Required runtime symbols (only when `workflows=true` is set):

| Symbol | Provided by | Notes |
|---|---|---|
| `temporal_runtime::worker::Worker` | bridge crate `worker` feature | Re-exported from `temporalio_sdk::Worker`. |
| `temporal_runtime::worker::WorkflowImplementer` | bridge crate `worker` feature | Re-exported from `temporalio_sdk::workflows::WorkflowImplementer`. |

## Phase 4.0 — CLI scaffold (opt-in via `cli=true`)

When invoked with `--rust-temporal_opt=cli=true`, the plugin emits, per service
with at least one workflow rpc, a sibling `<service>_cli` module containing:

| Symbol | Shape |
|---|---|
| `<service>_cli::Cli` | `#[derive(clap::Parser)]` entry point with a single `command: Command` field. |
| `<service>_cli::Command` | `#[derive(clap::Subcommand)]` enum with `Start<Workflow>` + `Attach<Workflow>` variants per workflow rpc. |
| `<service>_cli::Start<Workflow>Args` | `#[derive(clap::Args)]` struct. Carries `--input-file`, optional `--workflow-id`, optional `--wait`. |
| `<service>_cli::Attach<Workflow>Args` | `#[derive(clap::Args)]` struct. Carries a positional `workflow_id` + optional `--wait`. |

Required runtime symbols (only when `cli=true`):

| Symbol | Provided by |
|---|---|
| `temporal_runtime::clap` | bridge crate `cli` feature (re-exports the `clap` crate). |

Phase 4.0 ships the parser structure only. **No `Cli::run` impl is emitted.**
Consumers parse the CLI, match on `Command`, and call into the generated
`<Service>Client` themselves. Phase 4.1 will add `Cli::run(self, &client)`
once the JSON-input → prost-message deserialize path is decided (the open
question is `pbjson` vs plain `prost::Message::decode` from a binary file).

## Phase 8 — Test client

No `test_client` emit is produced for `temporalio-sdk` 0.4.0. The SDK probe in
`docs/sdk-shape-worker.md` found no `TestWorkflowEnvironment` equivalent to
wrap. The closest upstream pieces are low-level `temporalio-sdk-core`
`ephemeral_server` support and raw `temporalio-client` TestService RPCs, which
would require this project to own a separate test harness.

## Future direction

The current contract is structural — the compiler tells consumers what
they're missing, but there's no nominal `trait TemporalRuntime`
constraining the facade. A trait would shrink this document to "impl
this trait" and give consumers IDE-level discoverability of every
required function. Tracked as a v0.x → v1.0 design decision; the cost is
a more intrusive consumer API, the benefit is stronger compile-time
errors.

Before that decision, roadmap phases should keep using free functions and
re-exported SDK types under `crate::temporal_runtime` unless a phase-specific
design note explains why a trait is required.
