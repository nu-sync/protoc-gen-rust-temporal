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
without writing the facade yourself. The stub at
`examples/job-queue-integration/src/temporal_runtime.rs` stays the
canonical override reference for power users who need to substitute a
custom transport, vendored SDK, or test stub.

The canonical stub implementation lives in
[`examples/job-queue-integration/src/temporal_runtime.rs`](../examples/job-queue-integration/src/temporal_runtime.rs).
That file is a workspace member that `cargo check`s clean — copying it
into a new consumer crate and replacing the `todo!()` bodies with real
SDK calls is the supported on-ramp.

## Versioning

This document is **pinned to a plugin version**. Each row notes the
plugin version in which the call site was introduced; nothing here is
removed without a major bump (post-1.0) or a deprecation cycle (pre-1.0).

Current pin: **protoc-gen-rust-temporal 0.1.0**.

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
| `query_proto<I, O>` | `handle.<query>()` with **non-Empty** input | `async fn(&WorkflowHandle, query_name: &str, input: &I) -> Result<O>` where both: `TemporalProtoMessage` | 0.1.0 |
| `query_proto_empty<O>` | `handle.<query>()` with **Empty** input | `async fn(&WorkflowHandle, query_name: &str) -> Result<O>` where `O: TemporalProtoMessage` | 0.1.0 |
| `update_proto<I, O>` | `handle.<update>()` with **non-Empty** input | `async fn(&WorkflowHandle, update_name: &str, input: &I, wait_policy: WaitPolicy) -> Result<O>` | 0.1.0 |
| `update_proto_empty<O>` | `handle.<update>()` with **Empty** input | `async fn(&WorkflowHandle, update_name: &str, wait_policy: WaitPolicy) -> Result<O>` | 0.1.0 |
| `signal_with_start_workflow_proto<W, S>` | `<signal>_with_start` free fn (signal has `start: true`) | `async fn(client, workflow_name, workflow_id, task_queue, workflow_input: &W, signal_name, signal_input: &S, …) -> Result<WorkflowHandle>` | 0.1.0 |
| `update_with_start_workflow_proto<W, U, O>` | `<update>_with_start` free fn (update has `start: true`) | `async fn(client, …, workflow_input: &W, update_name, update_input: &U, wait_policy, …) -> Result<(WorkflowHandle, O)>` | 0.1.0 |

### Removed since 0.0.x

| Function | Removed in | Replacement |
|---|---|---|
| `eval_id_expression(template: &str) -> String` | 0.1.0 | Plugin now materialises the `id` template into a private `<rpc>_id(input: &Input) -> String` function alongside the start method. Generated code calls `<rpc>_id(&input)` directly; the runtime no longer sees the template string. Consumers can delete any local `eval_id_expression` they had. |

## Note on `Empty` inputs and `_with_start`

`signal_with_start` and `update_with_start` free functions take **both**
the workflow input and the signal/update input. Supporting any of those
inputs being `google.protobuf.Empty` would require a combinatorial
explosion of runtime functions (`signal_with_start_proto_empty_workflow`,
`signal_with_start_proto_empty_signal`, …) which is not worth it for
v1.

The plugin's validate step **rejects** workflows that combine
`signal: [{ start: true }]` or `update: [{ start: true }]` with any
Empty side, asking users to wrap the empty payload in a single-field
message instead. So in practice, generated code never calls
`signal_with_start_workflow_proto` or `update_with_start_workflow_proto`
with `&()` arguments.

## Phase 2 — Activities (opt-in via `activities=true`)

When invoked with `--rust-temporal_opt=activities=true`, the plugin emits, per
service with activity-annotated methods:

| Symbol | Shape |
|---|---|
| `<METHOD>_ACTIVITY_NAME` | `pub const &'static str` per annotated activity. Value matches what the activity is registered under server-side (defaults to the rpc method name). |
| `<Service>Activities` | `pub trait <Service>Activities: Send + Sync + 'static` with one method per activity. Signature: `fn <method>(&self, ctx: temporal_runtime::ActivityContext, input: <Input>) -> impl Future<Output = Result<<Output>>> + Send`. |

The trait method takes `&self`. The consumer's adapter (which wires the trait
to a `temporalio-sdk` Worker via `#[activity_definitions]`) does the
`Arc<Self>` dance — see the `temporal-proto-runtime-bridge` README for the
pattern.

Required runtime symbols (only when `activities=true` is set):

| Symbol | Provided by | Notes |
|---|---|---|
| `temporal_runtime::ActivityContext` | bridge crate `worker` feature | Re-exported from `temporalio_sdk::activities::ActivityContext`. |

The plugin does NOT emit a `register_<service>_activities(...)` function in
Phase 2 — the consumer-side adapter pattern handles registration. This is the
trait-only emit per the [Phase 2 spike findings](../docs/superpowers/specs/2026-05-12-phase-2-spike-findings.md).

## Phase 3.0 — Workflow handler name consts (opt-in via `workflows=true`)

When invoked with `--rust-temporal_opt=workflows=true`, the plugin emits, per
service with at least one signal / query / update rpc:

| Symbol | Shape |
|---|---|
| `<METHOD>_SIGNAL_NAME` | `pub const &'static str` per signal-annotated rpc. Value is the cross-language registration name (defaults to the rpc method name). |
| `<METHOD>_QUERY_NAME` | Same shape, for query-annotated rpcs. |
| `<METHOD>_UPDATE_NAME` | Same shape, for update-annotated rpcs. |

No workflow trait is emitted in Phase 3.0 — that's the Option C cut from
the [Phase 3 spike findings](../docs/superpowers/specs/2026-05-12-phase-3-spike-findings.md).
Consumers wire their hand-rolled `#[workflow]` setup to reference these consts
instead of string literals, keeping registration names in sync with the proto
without an adapter prototype yet.

Trait emit lands in Phase 3.1 once the consume-self adapter shape is
verified end-to-end against `temporalio-sdk`'s `#[workflow]` macro.

## Future direction (post-1.0)

The current contract is structural — the compiler tells consumers what
they're missing, but there's no nominal `trait TemporalRuntime`
constraining the facade. A trait would shrink this document to "impl
this trait" and give consumers IDE-level discoverability of every
required function. Tracked as a v0.x → v1.0 design decision; the cost is
a more intrusive consumer API, the benefit is stronger compile-time
errors.
