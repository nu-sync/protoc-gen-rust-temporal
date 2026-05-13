# protoc-gen-rust-temporal — SPEC

**Status:** Implemented baseline plus historical delivery plan
**Date:** 2026-05-12
**Author:** wcygan
**Target repo:** `github.com/nu-sync/protoc-gen-rust-temporal`

## TL;DR

A `protoc` plugin that reads `temporal.v1.*` method options from a proto
service and emits typed **Rust** Temporal client and worker-side code. Schema is
**not** ours -- we consume the annotation set published by
[`cludden/protoc-gen-go-temporal`](https://github.com/cludden/protoc-gen-go-temporal)
on BSR at `buf.build/cludden/protoc-gen-go-temporal`. We supply the Rust code
generator; cludden owns the schema.

Sibling project: [`nu-sync/protoc-gen-ts-temporal`](../protoc-gen-ts-temporal).
Same schema, same wire format, different emit target. Together they let a proto
annotated for `protoc-gen-go-temporal` produce Go (cludden), TS, and Rust
clients with **zero proto changes**.

The current implementation is a working baseline, not the final scope. The
active direction is majority parity with cludden's Go generator; see
[`ROADMAP.md`](./ROADMAP.md) for priority order and current unsupported
features.

## Permanent constraints and current limits

- Authoring a new annotation schema. Field numbers, message shapes, semantics: all cludden.
- Emitting business logic for workflow or activity bodies. Worker code remains
  hand-written against `temporalio-sdk`; the plugin can emit worker-side
  contracts, adapters, helpers, and registration glue that route through
  `crate::temporal_runtime`.
- Bundling a Temporal SDK. Generated code routes through a consumer-supplied
  `crate::temporal_runtime`; the default bridge depends on the pinned
  Temporal SDK crates.
- Supporting JSON payloads. Generated clients speak `binary/protobuf` and reject anything else (see Wire Format).
- **Current Bloblang limit.** cludden's Go plugin compiles `WorkflowOptions.id`
  as Bloblang and evaluates the expression at workflow-start time against the
  input message. The Rust plugin currently accepts only the simple
  `{{ .FieldName }}` subset and materializes it into a private
  `<rpc>_id(input: &Input) -> String` function at codegen time. Bloblang
  expressions are rejected by `parse_id_template` with a clear diagnostic. Full
  Bloblang support is roadmap work, not a permanent non-goal.

## Reference implementation

The original [`/Users/wcygan/Development/job-queue`](../job-queue)
proof-of-concept held a prototype plugin
(`crates/<prototype-plugin>/`, ~780 LOC) and a working
multi-consumer demo (job-worker + job-api + jobctl all sharing one generated
client over a Temporal dev server). The demo now lives in this repo at
[`examples/job-queue`](./examples/job-queue/). The PoC established:

- The `DescriptorPool::decode_file_descriptor_set` pattern for surviving extension data through prost-types (prost-types silently drops extensions on direct decode; you must reconstruct a `FileDescriptorSet` from the raw `CodeGeneratorRequest` bytes and feed it into the descriptor pool — `src/main.rs::extract_proto_file_blobs` is the working implementation).
- The four-stage pipeline: `parse → validate → render → CodeGeneratorResponse`.
- Golden-fixture testing with `regen_fixtures.sh` for reblessing.
- The `TypedProtoMessage<T: TemporalProtoMessage>` newtype that wraps prost-generated types and implements `TemporalSerializable`/`TemporalDeserializable` for the `binary/protobuf` wire format.
- Eight verified deviations between the `temporalio-sdk` 0.4 spec and reality
  (now tracked in `docs/sdk-shape.md`).

What carries over: the descriptor-extraction trick, the four-stage pipeline shape, the test harness, the `TypedProtoMessage` wrapper, the SDK landmines doc. What changes: input schema (cludden's, not ours), drop the `-client` suffix on the crate name, expand emit surface to cover update + signal-with-start.

This repo is a **fresh rewrite** of the plugin. The multi-consumer job-queue
demo has since been imported as the primary in-tree example.

## Schema source of truth

```yaml
# buf.yaml in any consumer
version: v2
deps:
  - buf.build/cludden/protoc-gen-go-temporal   # annotation schema
  - buf.build/temporalio/api                   # transitive: VersioningBehavior, WorkflowIdConflictPolicy enums
```

Consumer protos import as `import "temporal/v1/temporal.proto";` — same path cludden's own examples use.

Current baseline annotation behavior:

| Annotation | Current Rust behavior |
|---|---|
| `temporal.v1.workflow` on a method | Emit a typed `<workflow>(input, opts) -> <Workflow>Handle` method on the service client. With `workflows=true`, also emit `<Workflow>Definition` and `register_<workflow>_workflow(...)` glue. |
| `temporal.v1.query` on a method | Emit `handle.<query>() -> Output` returning the typed response. |
| `temporal.v1.signal` on a method | Emit `handle.<signal>(input) -> ()`. Validate signal returns `google.protobuf.Empty`. |
| `temporal.v1.update` on a method | Emit `handle.<update>(input, wait_policy) -> Output`. |
| `temporal.v1.activity` on a method | By default, validate only. With `activities=true`, emit `<Service>Activities`, activity name consts, and `register_<service>_activities(...)` glue. |
| `temporal.v1.service` on the service | Use as default `task_queue` if a workflow doesn't override it. |
| `WorkflowOptions.{Query,Signal,Update}.ref` | Wire each same-service ref through to the generated handle as a method. Unknown refs are a validation error. Cross-service refs are roadmap work. |
| `WorkflowOptions.aliases[]` | Parsed into the model but not fully emitted or registered yet. Alias parity is roadmap work. |
| `WorkflowOptions.{Signal,Update}.start = true` | Emit a free function `<signal>_with_start(...)` / `<update>_with_start(...)` alongside the client. |

Current unsupported or partial annotation areas include XNS, Nexus, patch/protopatch,
Bloblang-derived search attributes, method co-annotations, cross-service refs,
most activity runtime options, several workflow runtime options, and full CLI
execution. The active plan for these gaps is tracked in `ROADMAP.md`.

## Wire format

This is the one thing we own and must specify ourselves; orthogonal to the annotation schema. Documented in `WIRE-FORMAT.md` at the repo root, **byte-identical to the TS plugin's spec** so the two plugins can't drift.

```
metadata.encoding   = "binary/protobuf"
metadata.messageType = "<fully.qualified.proto.message.name>"
data                = raw proto wire bytes
```

- `binary/protobuf` matches the Temporal Go SDK's proto3 payload codec → interoperable with workers in any language.
- `messageType` header (e.g. `jobs.v1.JobInput`) is required for polymorphic decode. The PoC's `TypedProtoMessage<T>` already writes/checks this exact pair.
- Missing `messageType` → decode fails. We never fall through to the SDK's JSON default.
- Empty inputs use `google.protobuf.Empty` rather than null payloads — uniform decode path.

**Compatibility audited (2026-05-12):** `compat-tests/` confirms cludden's Go runtime emits the same triple against `cludden/protoc-gen-go-temporal@v1.22.1` — a Go client written against cludden's plugin can drive a Rust worker registered against ours and vice versa. See `WIRE-FORMAT.md` for the result and the CI guard.

## Generated Rust surface (sketch)

For a service like:

```proto
service JobService {
  rpc RunJob(JobInput) returns (JobOutput) {
    option (temporal.v1.workflow) = {
      task_queue: "jobs"
      id: "{{ .name }}"
      query:  [{ ref: "GetStatus" }]
      signal: [{ ref: "CancelJob", start: true }]
    };
  }
  rpc GetStatus(google.protobuf.Empty) returns (JobStatusOutput) {
    option (temporal.v1.query) = {};
  }
  rpc CancelJob(CancelJobInput) returns (google.protobuf.Empty) {
    option (temporal.v1.signal) = {};
  }
}
```

The plugin emits one Rust module per source proto, suitable for `include!`-ing from the consumer's lib.rs. Sketch:

```rust
pub mod jobs_v1_temporal {
    use crate::jobs::v1::*;                // prost types from protoc-gen-prost
    use crate::temporal_runtime;

    pub const RUN_JOB_WORKFLOW_NAME: &str = "jobs.v1.JobService.RunJob";
    pub const RUN_JOB_TASK_QUEUE: &str = "jobs";

    pub struct JobServiceClient {
        client: temporal_runtime::TemporalClient,
    }

    impl JobServiceClient {
        pub fn new(client: temporal_runtime::TemporalClient) -> Self { Self { client } }

        /// Start RunJob. Returns a typed handle.
        pub async fn run_job(
            &self,
            input: JobInput,
            opts: RunJobStartOptions,
        ) -> Result<RunJobHandle> { /* ... */ }

        /// Attach to a running workflow by ID.
        pub fn run_job_handle(&self, workflow_id: impl Into<String>) -> RunJobHandle { /* ... */ }
    }

    pub struct RunJobHandle { /* wraps WorkflowHandle */ }

    impl RunJobHandle {
        pub fn workflow_id(&self) -> &str { /* ... */ }
        pub async fn result(&self) -> Result<JobOutput> { /* ... */ }

        /// Method exists because `query: [{ ref: "GetStatus" }]` is wired in proto.
        pub async fn get_status(&self) -> Result<JobStatusOutput> { /* ... */ }

        /// Method exists because `signal: [{ ref: "CancelJob" }]` is wired in proto.
        pub async fn cancel_job(&self, input: CancelJobInput) -> Result<()> { /* ... */ }
    }

    /// Free function exists because the CancelJob signal has `start: true`.
    pub async fn cancel_job_with_start(
        client: &JobServiceClient,
        signal_input: CancelJobInput,
        workflow_input: JobInput,
        opts: RunJobStartOptions,
    ) -> Result<RunJobHandle> { /* ... */ }
}
```

Properties of the generated code:

- **One `<Service>Client` per proto service**, owning a
  `temporal_runtime::TemporalClient`.
- **One `<Workflow>Handle` per workflow rpc**, exposing only the signals/queries/updates declared in that workflow's options. Wrong signal name = compile error.
- **Workflow registration name, task queue, id expression, reuse policy, execution timeout** baked in from proto options as defaults; caller-supplied options override.
- **Inputs/outputs wrapped in `TypedProtoMessage<T>`** inside the runtime bridge
  so the SDK's `TemporalSerializable` dispatch picks `binary/protobuf` over the
  JSON default. Public API takes/returns bare prost types; wrapping is hidden.

## Runtime support

| Toolchain | Status | Notes |
|---|---|---|
| Stable Rust 1.88 | Tier 1 | Pinned by `rust-toolchain.toml` and workspace `rust-version`. |
| `temporalio-sdk` / `temporalio-client` =0.4.0 | Tier 1 | The default bridge pins exact SDK versions to avoid silent pre-1.0 API drift. |
| `prost` 0.13 | Tier 1 | Workspace dependency used by generated payload helpers and prost message types. |

Generator rules:

1. Emitted code is `no_std`-incompatible (uses `Box`, `async`). That's fine — Temporal workers run with `tokio`.
2. No `unsafe`. No proc-macros at consumer build time beyond what `prost` already requires.
3. Generated files compile cleanly under `clippy -D warnings`. The PoC currently does; preserve that property.
4. Public API uses `impl Into<String>` and `&str` for ergonomic input where it doesn't cost performance.

## Distribution

| Artifact | Channel | Consumer |
|---|---|---|
| `protoc-gen-rust-temporal` binary | crates.io + GitHub Releases (prebuilt: `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`) | `cargo install protoc-gen-rust-temporal` or download |
| Same binary | **BSR Remote Plugin** as `buf.build/nu-sync/rust-temporal` | `buf.gen.yaml: plugins: - remote: buf.build/nu-sync/rust-temporal` |
| Helper crate `temporal-proto-runtime` | crates.io | Provides the `TemporalProtoMessage` trait + `TypedProtoMessage<T>`. Its `sdk` feature supplies the SDK serialization impls. |
| Default bridge crate `temporal-proto-runtime-bridge` | crates.io | Drop-in `crate::temporal_runtime` facade backed by the pinned Temporal SDK crates. |

BSR Remote Plugin is the headline distribution path. Cargo install is the fallback.

## Repo layout (target)

```
protoc-gen-rust-temporal/
├── SPEC.md                            # this file
├── ROADMAP.md                         # active majority-parity direction
├── WIRE-FORMAT.md                     # payload contract (byte-identical to TS sibling)
├── README.md                          # quickstart + buf.gen.yaml example
├── Cargo.toml                         # workspace
├── crates/
│   ├── protoc-gen-rust-temporal/      # THE PLUGIN
│   │   ├── Cargo.toml
│   │   ├── build.rs                   # compile cludden's proto via prost-build
│   │   ├── src/
│   │   │   ├── main.rs                # stdin → CodeGeneratorRequest → stdout
│   │   │   ├── lib.rs                 # run_with_pool entry point
│   │   │   ├── model.rs               # ServiceModel, WorkflowModel, …
│   │   │   ├── parse.rs               # DescriptorPool → ServiceModel
│   │   │   ├── validate.rs            # cross-method invariants
│   │   │   └── render.rs              # ServiceModel → Rust source
│   │   └── tests/
│   │       ├── golden.rs
│   │       ├── regen_fixtures.sh
│   │       └── fixtures/
│   │           ├── minimal_workflow/
│   │           ├── workflow_with_query/
│   │           ├── workflow_with_signal/
│   │           ├── workflow_with_signal_with_start/
│   │           ├── workflow_with_update/
│   │           ├── activity_only/          # validate-only path
│   │           ├── full/
│   │           └── bad_*/                  # validation-error fixtures
│   ├── temporal-proto-runtime/        # shared TypedProtoMessage<T> helper
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   └── temporal-proto-runtime-bridge/ # default crate::temporal_runtime facade
│       ├── Cargo.toml
│       └── src/lib.rs
├── docs/
│   └── sdk-shape.md                   # ported/refreshed from job-queue
├── examples/
│   └── job-queue/                     # primary end-to-end consumer example
└── .github/
    └── workflows/
        ├── ci.yml                     # cargo test + clippy + fmt + golden bless check
        └── release.yml                # build matrix → GitHub Release + crates.io publish
```

## Roadmap relationship

The phases below describe the path that produced the current `0.1.x` baseline.
New feature planning should start from [`ROADMAP.md`](./ROADMAP.md), which
prioritizes majority parity with cludden's Go generator. When a roadmap phase
changes generated code, update this spec only for durable behavior and keep
phase-by-phase planning in the roadmap or a focused design note.

## Phased delivery

Each phase ends with green CI and a tagged release.

**Phase 0 — Repo bootstrap**
- Workspace `Cargo.toml`, plugin crate skeleton, cludden's proto pulled via `prost-build` from a vendored copy of `temporal/v1/temporal.proto` (switch to a BSR-fed build step later if it pays off). Add the `temporalio/api` enum protos transitively required.
- Port `docs/sdk-shape.md` from job-queue. Refresh against current `temporalio-sdk` if it's moved.
- CI: cargo test, clippy `-D warnings`, fmt check.

**Phase 1 — Parse cludden's schema**
- Reimplement `parse.rs` against the new schema. Key changes from PoC: `WorkflowOptions.signal` / `query` / `update` are nested messages with `ref`, not `repeated string`. Field numbers move to `7233-7237`.
- Validate-only support for `activity` annotations: reject collisions with workflow/signal/query/update names.

**Phase 2 — Render parity with PoC + extend**
- Port `render.rs` to emit the new client surface. Match the PoC byte-for-byte for the overlapping subset (workflow + query + signal handles); extend for update + signal-with-start.
- Optionally factor `TypedProtoMessage<T>` into the `temporal-proto-runtime` helper crate.
- Golden fixtures rebless.

**Phase 3 — Wire-format audit** (completed 2026-05-12)
- Cross-tested our emitted converter against `cludden/protoc-gen-go-temporal@v1.22.1` via `compat-tests/`. Four fixtures (scalar, `google.protobuf.Empty`, nested message, repeated message) produced byte-identical Payloads from both arms.
- `WIRE-FORMAT.md` pinned at v1. `compat-audit` CI job regression-protects the result.

**Phase 4 — Distribution**
- crates.io publish + GitHub Release with prebuilt binaries (`cargo-dist` or hand-rolled).
- BSR Remote Plugin registered as `buf.build/nu-sync/rust-temporal`.
- README quickstart includes both `buf.gen.yaml` shapes (remote + local install).

**Phase 5 — First external consumer** (completed 2026-05-13)
- `job-queue` now consumes the externally-installed
  `protoc-gen-rust-temporal` for client + worker emit and registers its Rust
  worker through generated `RunJobDefinition`, `JobServiceActivities`,
  `register_run_job_workflow`, and `register_job_service_activities` glue.
  Landed in [`job-queue` commit `88c4749`](../job-queue) (`Migrate worker to
  generated Temporal contracts`).
- The external-consumer pass kept the vendored annotation schema pinned instead
  of switching to a BSR dep so it stays aligned with the plugin's cludden
  v1.22.1 schema while Phase 4 distribution remains in flight.
- Verification: `just gen` idempotence, `cargo check --workspace
  --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace --all-targets`, `just demo` against a real Temporal
  dev server, Go client -> migrated Rust worker, and migrated Rust client -> Go
  worker all passed.

**Phase 6 — Update + signal-with-start emit polish** (completed 2026-05-12)
- The generated handle surface includes typed update methods with explicit
  `temporal_runtime::WaitPolicy` plumbing for non-Empty and Empty request /
  response combinations.
- Signal-with-start and update-with-start emit free functions alongside the
  client struct. Runtime requirements are pinned in `docs/RUNTIME-API.md`, and
  golden fixtures cover the generated call sites.

**Phase 7 — Worker emit** (completed 2026-05-13)
- Opt-in flags: `activities=true` and `workflows=true`.
- Activity emit: per-service `<Service>Activities` trait, per-activity
  `<METHOD>_ACTIVITY_NAME` constants, and
  `register_<service>_activities<I>(&mut temporal_runtime::worker::Worker, I)`
  where `I` implements both the generated trait and the SDK
  `ActivityImplementer`.
- Workflow emit: per-workflow `<Workflow>Definition` trait with associated
  `Input` / `Output` types and default associated consts for workflow name,
  task queue, and attached signal/query/update names, plus
  `register_<workflow>_workflow<W>(&mut temporal_runtime::worker::Worker)`
  where `W` implements both the generated definition trait and the SDK
  `WorkflowImplementer`.
- Consumer-owned code remains responsible for `temporalio-sdk` macros:
  `#[activities]` adapters for activities and `#[workflow]` /
  `#[workflow_methods]` for workflows. Generated code does not emit workflow
  structs, workflow method bodies, activity bodies, worker construction,
  interceptors, worker versioning, or `WorkerOptions`.
- Every worker-facing generated reference goes through
  `crate::temporal_runtime`; the default bridge exposes the needed symbols
  behind its `worker` feature.
- The job-queue Phase 5 migration now consumes this worker emit through the
  generated `RunJobDefinition`, `JobServiceActivities`,
  `register_run_job_workflow`, and `register_job_service_activities` glue.

**Phase 8 — Test client** (gated by SDK support)
- Probe result: `docs/sdk-shape-worker.md` found no
  `TestWorkflowEnvironment` equivalent in pinned `temporalio-sdk = "=0.4.0"`.
- No `test_client` emit ships for this SDK pin. Building a time-skipping test
  harness from `temporalio-sdk-core::ephemeral_server` and raw
  `temporalio-client` TestService RPCs is a separate project-level decision,
  not part of Phase 7.

## Open questions

1. **Does cludden's Go runtime wire-format match ours?** Resolved in Phase 3; not blocking earlier phases.
2. **MSRV target?** Resolved: workspace `rust-version` is pinned to 1.88.
3. **`temporal-proto-runtime` helper crate: ship it or inline?** Resolved:
   shipped as the `temporal-proto-runtime` crate, with SDK serialization impls
   behind its `sdk` feature.
4. **Should activity-tagged methods emit *anything*?** Resolved in Phase 7:
   validate-only by default; `activities=true` emits the typed trait, name
   constants, and thin registration helper.
5. **Workflow start options shape.** Resolved in Phase 2: each workflow gets a
   generated `<Workflow>StartOptions` struct with optional overrides and
   proto-derived defaults.
6. **License.** Resolved: MIT.
