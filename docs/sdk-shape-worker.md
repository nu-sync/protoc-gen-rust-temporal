# Pinned Temporal Rust SDK worker and test shape

**Verified against:**
- `temporalio-sdk` 0.4.0
- `temporalio-sdk-core` 0.4.0
- `temporalio-client` 0.4.0
- `temporalio-common` 0.4.0
- Workspace pin: `temporalio-sdk = "=0.4.0"` in `Cargo.toml`
- Probe date: 2026-05-13

Reference source root:
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`

## Probe command

The SDK dependency is optional behind the bridge crate's `worker` feature, so
plain `cargo metadata` does not include `temporalio-sdk`. Use:

```sh
cargo metadata --format-version=1 --locked --features temporal-proto-runtime-bridge/worker \
  | jq -r '.packages[] | select(.name|test("temporal")) | [.name,.version,.manifest_path] | @tsv'
```

Primary negative test for the test-environment API:

```sh
rg -n "TestWorkflowEnvironment|TestEnvironment|TestingEnvironment|WorkflowEnvironment|TestServer|test_server|test server|TimeSkipping|time_skipping|ephemeral" \
  ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/temporalio-sdk-0.4.0/src \
  ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/temporalio-sdk-core-0.4.0/src \
  ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/temporalio-client-0.4.0/src \
  ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/temporalio-common-0.4.0/src
```

## Test client finding

`temporalio-sdk` 0.4.0 does **not** expose a `TestWorkflowEnvironment`
equivalent.

There is no public high-level type that owns a test server, a client, workers,
time-skipping controls, and workflow execution helpers in the style of the Go
SDK's test suite. The only hits near this area are lower-level pieces:

- `temporalio-sdk-core-0.4.0/src/ephemeral_server/mod.rs` exists behind the
  `ephemeral-server` feature. It exposes `TemporalDevServerConfig`,
  `TestServerConfig`, `EphemeralServer`, `EphemeralExe`, and download helpers.
- `temporalio-client-0.4.0/src/grpc.rs` exposes raw `TestService` RPCs such as
  `lock_time_skipping`, `unlock_time_skipping`, `sleep`, `sleep_until`,
  `unlock_time_skipping_with_sleep`, and `get_current_time`.
- `temporalio-common` carries time-skipping protos and generated types.

Those pieces are not re-exported by `temporalio-sdk` and are not a typed
workflow test harness. A generator-level `test_client` emit would therefore
need to design and maintain a harness around `temporalio-sdk-core` plus
`temporalio-client`, including feature wiring, server lifecycle, namespace and
connection setup, worker startup, shutdown, and time-skipping policy.

**Scope decision implied by the probe:** do not include `test_client` emit in
the first worker-emit slice. Treat Phase 8 as blocked until the upstream Rust
SDK publishes a stable test environment, or until this project explicitly
accepts owning a separate test-harness facade.

## Worker construction API

`temporalio-sdk-0.4.0/src/lib.rs` defines `WorkerOptions` at lines 182-285
and `Worker` at lines 431-595.

Worker creation:

```rust
pub fn new(
    runtime: &CoreRuntime,
    client: Client,
    mut options: WorkerOptions,
) -> Result<Self, Box<dyn std::error::Error>>
```

`WorkerOptions` is built through a `bon` builder with
`WorkerOptions::new(task_queue)`. Relevant public fields include:

- `task_queue: String`
- `deployment_options: WorkerDeploymentOptions`
- `client_identity_override: Option<String>`
- `max_cached_workflows: usize`
- `tuner: Arc<dyn WorkerTuner + Send + Sync>`
- workflow/activity/nexus poller behavior
- `task_types: WorkerTaskTypes`
- sticky queue timeout
- heartbeat throttling intervals
- task-queue and worker activity rate limits
- workflow failure error sets
- `graceful_shutdown_period: Option<Duration>`
- `detect_nondeterministic_futures: bool`

`Worker::run(&mut self)` is the long-running poll loop. `Worker` also exposes
`shutdown_handle(&self)` and `task_queue(&self)`.

## Registration entry points

Registration can happen before worker construction through
`WorkerOptionsBuilder`, after construction through `Worker`, or on
`WorkerOptions` itself.

Builder-time entry points from `temporalio-sdk-0.4.0/src/lib.rs` lines
287-335:

```rust
pub fn register_activities<AI: ActivityImplementer>(self, instance: AI) -> Self
pub fn register_activity<AD>(self, instance: Arc<AD::Implementer>) -> Self
where
    AD: ActivityDefinition + ExecutableActivity,
    AD::Output: Send + Sync
pub fn register_workflow<WI: WorkflowImplementer>(self) -> Self
pub fn register_workflow_with_factory<W, F>(self, factory: F) -> Self
where
    W: WorkflowImplementation,
    <W::Run as WorkflowDefinition>::Input: Send,
    F: Fn() -> W + Send + Sync + 'static
```

Mutable `WorkerOptions` entry points from lines 343-386 have the same shape
but take `&mut self`.

Runtime `Worker` entry points from lines 551-591:

```rust
pub fn register_activities<AI: ActivityImplementer>(&mut self, instance: AI) -> &mut Self
pub fn register_activity<AD>(&mut self, instance: Arc<AD::Implementer>) -> &mut Self
where
    AD: ActivityDefinition + ExecutableActivity,
    AD::Output: Send + Sync
pub fn register_workflow<WI: WorkflowImplementer>(&mut self) -> &mut Self
pub fn register_workflow_with_factory<W, F>(&mut self, factory: F) -> &mut Self
where
    W: WorkflowImplementation,
    <W::Run as WorkflowDefinition>::Input: Send,
    F: Fn() -> W + Send + Sync + 'static
```

Workflow registration still depends on SDK macro-generated types:
`WorkflowImplementer`, `WorkflowImplementation`, and `WorkflowDefinition` in
`temporalio-sdk-0.4.0/src/workflows.rs`. This confirms the existing design
constraint from `docs/sdk-shape.md`: generated code should own names and typed
contracts, while the consumer owns the `#[workflow]` struct and
`#[workflow_methods]` impl.

Activity registration depends on SDK macro-generated activity marker types:
`ActivityImplementer`, `ActivityDefinition`, and `ExecutableActivity` in
`temporalio-sdk-0.4.0/src/activities.rs`. A generated trait alone cannot be
registered directly with `Worker::register_activities`; consumers still need an
adapter produced by the SDK's `#[activities]` macro, or this project
needs a separate facade-owned adapter layer.

## Activity context shape

`temporalio-sdk-0.4.0/src/activities.rs` defines:

```rust
#[derive(Clone)]
pub struct ActivityContext {
    worker: Arc<CoreWorker>,
    cancellation_token: CancellationToken,
    heartbeat_details: Vec<Payload>,
    header_fields: HashMap<String, Payload>,
    info: ActivityInfo,
}
```

Activities receive the context as the first argument after any `self` receiver.
The SDK examples use:

```rust
async fn echo(_ctx: ActivityContext, input: String) -> Result<String, ActivityError>
```

The existing bridge re-export of `ActivityContext`, `ActivityError`, and
`Worker` behind the `worker` feature is consistent with this shape.

## v1 implication

For worker emit, keep the first slice narrow:

- Emit type-level proto contracts, constants, and opt-in trait surfaces.
- Route all generated references through `crate::temporal_runtime`.
- Do not emit a full worker builder, interceptors, versioning controls, or a
  test-client harness in v1.
- Preserve the byte-identical payload path by not changing client encoding or
  `TemporalProtoMessage` implementations.
