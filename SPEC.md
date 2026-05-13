# protoc-gen-rust-temporal — SPEC

**Status:** Design (pre-implementation)
**Date:** 2026-05-12
**Author:** wcygan
**Target repo:** `github.com/nu-sync/protoc-gen-rust-temporal`

## TL;DR

A `protoc` plugin that reads `temporal.v1.*` method options from a proto service and emits a typed **Rust** Temporal client. Schema is **not** ours — we consume the annotation set published by [`cludden/protoc-gen-go-temporal`](https://github.com/cludden/protoc-gen-go-temporal) on BSR at `buf.build/cludden/protoc-gen-go-temporal`. We supply the Rust code generator; cludden owns the schema.

Sibling project: [`nu-sync/protoc-gen-ts-temporal`](../protoc-gen-ts-temporal). Same schema, same wire format, different emit target. Together they let a proto annotated for `protoc-gen-go-temporal` produce Go (cludden), TS, and Rust clients with **zero proto changes**.

## Non-goals

- Authoring a new annotation schema. Field numbers, message shapes, semantics: all cludden.
- Emitting worker bodies or activity implementations. Worker code remains
  hand-written against `temporalio-sdk`; the plugin can optionally emit
  worker-side contracts and thin registration helpers that route through
  `crate::temporal_runtime`.
- Bundling a Temporal SDK. We depend on `temporalio-client` + `prost` at runtime.
- Supporting JSON payloads. Generated clients speak `binary/protobuf` and reject anything else (see Wire Format).
- **Bloblang `id` templates.** cludden's Go plugin compiles `WorkflowOptions.id` as [Bloblang](https://docs.redpanda.com/redpanda-connect/guides/bloblang/about/) (e.g. `${! name.or("anonymous") }`) and evaluates the expression *at workflow-start time* against the input message. The Rust plugin only accepts the [Go template](https://pkg.go.dev/text/template) subset cludden's own annotation comments use — `{{ .FieldName }}` references on the input proto — and materialises them into a private `<rpc>_id(input: &Input) -> String` function at codegen time. Non-Bloblang `{{ .X }}` templates round-trip identically between Go and Rust; Bloblang-only expressions are rejected by `parse_id_template` with a clear "only field references are supported" error so users see the limitation at protoc time rather than getting silently-wrong workflow ids at runtime. If full Bloblang parity becomes necessary post-1.0, the path is to pull `bloblang-rs` (or compile to a Rust closure during codegen) rather than ship a runtime evaluator.

## Reference implementation

[`/Users/wcygan/Development/job-queue`](../job-queue) holds the **proof-of-concept** plugin (`crates/protoc-gen-rust-temporal-client/`, ~780 LOC) and a working multi-consumer demo (job-worker + job-api + jobctl all sharing one generated client over a Temporal dev server). It established:

- The `DescriptorPool::decode_file_descriptor_set` pattern for surviving extension data through prost-types (prost-types silently drops extensions on direct decode; you must reconstruct a `FileDescriptorSet` from the raw `CodeGeneratorRequest` bytes and feed it into the descriptor pool — `src/main.rs::extract_proto_file_blobs` is the working implementation).
- The four-stage pipeline: `parse → validate → render → CodeGeneratorResponse`.
- Golden-fixture testing with `regen_fixtures.sh` for reblessing.
- The `TypedProtoMessage<T: TemporalProtoMessage>` newtype that wraps prost-generated types and implements `TemporalSerializable`/`TemporalDeserializable` for the `binary/protobuf` wire format.
- Eight verified deviations between the `temporalio-sdk` 0.4 spec and reality (documented in `job-queue/docs/sdk-shape.md`).

What carries over: the descriptor-extraction trick, the four-stage pipeline shape, the test harness, the `TypedProtoMessage` wrapper, the SDK landmines doc. What changes: input schema (cludden's, not ours), drop the `-client` suffix on the crate name, expand emit surface to cover update + signal-with-start.

This new repo is a **fresh rewrite**, not a move. The PoC stays in `job-queue/` until the new plugin is published and job-queue switches to consuming it.

## Schema source of truth

```yaml
# buf.yaml in any consumer
version: v2
deps:
  - buf.build/cludden/protoc-gen-go-temporal   # annotation schema
  - buf.build/temporalio/api                   # transitive: VersioningBehavior, WorkflowIdConflictPolicy enums
```

Consumer protos import as `import "temporal/v1/temporal.proto";` — same path cludden's own examples use.

Annotations we consume:

| Annotation | What v1 of this plugin does with it |
|---|---|
| `temporal.v1.workflow` on a method | Emit a typed `<workflow>(input, opts) -> <Workflow>Handle` method on the service client. With `workflows=true`, also emit `<Workflow>Definition` and `register_<workflow>_workflow(...)` glue. |
| `temporal.v1.query` on a method | Emit `handle.<query>() -> Output` returning the typed response. |
| `temporal.v1.signal` on a method | Emit `handle.<signal>(input) -> ()`. Validate signal returns `google.protobuf.Empty`. |
| `temporal.v1.update` on a method | Emit `handle.<update>(input, wait_policy) -> Output`. |
| `temporal.v1.activity` on a method | By default, validate only. With `activities=true`, emit `<Service>Activities`, activity name consts, and `register_<service>_activities(...)` glue. |
| `temporal.v1.service` on the service | Use as default `task_queue` if a workflow doesn't override it. |
| `WorkflowOptions.{Query,Signal,Update}.ref` | Wire each ref through to the generated handle as a method. Unknown refs are a validation error. |
| `WorkflowOptions.aliases[]` | Recorded as constants; not exposed on the client API directly. |
| `WorkflowOptions.{Signal,Update}.start = true` | Emit a free function `<signal>_with_start(...)` / `<update>_with_start(...)` alongside the client. |

Out of scope for v1 emit (read and ignored): `XNSActivityOptions`, `Patch`, `CLI*Options`, `FieldOptions`.

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
    use temporalio_client::Client;
    use crate::jobs::v1::*;                // prost types from protoc-gen-prost
    use crate::proto_message::TypedProtoMessage;

    pub const RUN_JOB_WORKFLOW: &str = "jobs.v1.JobService/RunJob";
    pub const RUN_JOB_TASK_QUEUE: &str = "jobs";

    pub struct JobServiceClient { client: Client }

    impl JobServiceClient {
        pub fn new(client: Client) -> Self { Self { client } }

        /// Start RunJob. Returns a typed handle.
        pub async fn run_job(
            &self,
            input: JobInput,
            opts: RunJobStartOptions,
        ) -> Result<RunJobHandle, temporalio_client::Error> { /* ... */ }

        /// Attach to a running workflow by ID.
        pub fn run_job_handle(&self, workflow_id: impl Into<String>) -> RunJobHandle { /* ... */ }
    }

    pub struct RunJobHandle { /* wraps WorkflowHandle */ }

    impl RunJobHandle {
        pub fn workflow_id(&self) -> &str { /* ... */ }
        pub async fn result(&self) -> Result<JobOutput, temporalio_client::Error> { /* ... */ }

        /// Method exists because `query: [{ ref: "GetStatus" }]` is wired in proto.
        pub async fn get_status(&self) -> Result<JobStatusOutput, temporalio_client::Error> { /* ... */ }

        /// Method exists because `signal: [{ ref: "CancelJob" }]` is wired in proto.
        pub async fn cancel_job(&self, input: CancelJobInput) -> Result<(), temporalio_client::Error> { /* ... */ }
    }

    /// Free function exists because the CancelJob signal has `start: true`.
    pub async fn cancel_job_with_start(
        client: &JobServiceClient,
        signal_input: CancelJobInput,
        workflow_input: JobInput,
        opts: RunJobStartOptions,
    ) -> Result<RunJobHandle, temporalio_client::Error> { /* ... */ }
}
```

Properties of the generated code:

- **One `<Service>Client` per proto service**, owning a `temporalio_client::Client`.
- **One `<Workflow>Handle` per workflow rpc**, exposing only the signals/queries/updates declared in that workflow's options. Wrong signal name = compile error.
- **Workflow registration name, task queue, id expression, reuse policy, execution timeout** baked in from proto options as defaults; caller-supplied options override.
- **Inputs/outputs wrapped in `TypedProtoMessage<T>`** internally so the SDK's `TemporalSerializable` dispatch picks `binary/protobuf` over the JSON default. Public API takes/returns bare prost types — wrapping is hidden.

## Runtime support

| Toolchain | Status | Notes |
|---|---|---|
| Stable Rust (current MSRV TBD; likely 1.78+) | Tier 1 | |
| `temporalio-sdk` / `temporalio-client` 0.4+ | Tier 1 | PoC validated against 0.4.0; track upstream as it matures. |
| `prost` 0.13 | Tier 1 | Same version the PoC uses; ecosystem default. |

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
| Optional helper crate `temporal-proto-runtime` (TBD) | crates.io | Provides the `TemporalProtoMessage` trait + `TypedProtoMessage<T>` so consumers don't hand-roll it. The PoC inlines this in `crates/jobs-proto/src/proto_message.rs`; lifting it to a shared crate avoids duplicating ~50 LOC across every consumer. |

BSR Remote Plugin is the headline distribution path. Cargo install is the fallback.

## Repo layout (target)

```
protoc-gen-rust-temporal/
├── SPEC.md                            # this file
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
│   └── temporal-proto-runtime/        # OPTIONAL — shared TypedProtoMessage<T> helper
│       ├── Cargo.toml
│       └── src/lib.rs
├── docs/
│   └── sdk-shape.md                   # ported/refreshed from job-queue
├── examples/
│   └── job-queue-integration/         # consumes from job-queue's proto
└── .github/
    └── workflows/
        ├── ci.yml                     # cargo test + clippy + fmt + golden bless check
        └── release.yml                # build matrix → GitHub Release + crates.io publish
```

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

**Phase 5 — First external consumer**
- Migrate `job-queue` off its vendored `temporal/v1/temporal.proto` onto the BSR dep. Switch `jobs-proto/build.rs` from invoking the in-tree plugin to invoking the externally-installed `protoc-gen-rust-temporal`. Delete `job-queue/crates/protoc-gen-rust-temporal-client/`. Verify the end-to-end demo (`just demo`) still passes against the new plugin binary. This is the integration test that proves "anyone can use it."

**Phase 6 — Update + signal-with-start emit polish**
- Beyond the PoC's surface area. Update support requires `WaitPolicy` plumbing. Signal-with-start and update-with-start require emitting free functions alongside the client struct.

**Phase 7 — Worker emit** (scoped 2026-05-13)
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

**Phase 8 — Test client** (gated by SDK support)
- Probe result: `docs/sdk-shape-worker.md` found no
  `TestWorkflowEnvironment` equivalent in pinned `temporalio-sdk = "=0.4.0"`.
- No `test_client` emit ships for this SDK pin. Building a time-skipping test
  harness from `temporalio-sdk-core::ephemeral_server` and raw
  `temporalio-client` TestService RPCs is a separate project-level decision,
  not part of Phase 7.

## Open questions

1. **Does cludden's Go runtime wire-format match ours?** Resolved in Phase 3; not blocking earlier phases.
2. **MSRV target?** PoC builds on stable; pin a concrete minimum (likely 1.78 or 1.80) once the new repo is bootstrapped.
3. **`temporal-proto-runtime` helper crate: ship it or inline?** Pro for shipping: removes ~50 LOC of boilerplate from every consumer. Con: one more crate to version. Decide in Phase 2.
4. **Should activity-tagged methods emit *anything*?** Resolved in Phase 7:
   validate-only by default; `activities=true` emits the typed trait, name
   constants, and thin registration helper.
5. **Workflow start options shape.** PoC uses a generated `<Workflow>StartOptions` struct (workflow_id, task_queue override, etc.). Reasonable defaults vs. fully-required fields — settle in Phase 2.
6. **License.** Cludden's repo is MIT. Match (MIT) or Apache-2.0 for the plugin?
