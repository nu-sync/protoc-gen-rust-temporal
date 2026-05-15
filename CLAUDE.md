# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

`protoc-gen-rust-temporal` is a `protoc` plugin that consumes
[`cludden/protoc-gen-go-temporal`](https://github.com/cludden/protoc-gen-go-temporal)'s
`temporal.v1.*` annotations from a proto service and emits typed **Rust**
[Temporal](https://temporal.io) client and worker-side code. We supply the
Rust code generator; cludden owns the annotation schema, and the schema is
vendored at
`crates/protoc-gen-rust-temporal/proto/temporal/v1/temporal.proto`.

Sibling project: `nu-sync/protoc-gen-ts-temporal`. Wire format is
intentionally byte-identical (see `WIRE-FORMAT.md`) so one annotated proto
produces Go, TS, and Rust clients with no proto changes.

The plugin currently emits generated client code plus typed worker-side
contracts (activity traits, workflow registration names/helpers, and optional
CLI scaffolding). The project direction is majority parity with
`protoc-gen-go-temporal`; richer generated worker surfaces and workflow-side
activity execution helpers are roadmap priorities. Workflow and activity bodies
remain hand-written against `temporalio-sdk`.

## Required reading before non-trivial changes

- `SPEC.md` — current baseline behavior, durable constraints, and historical
  delivery phases.
- `ROADMAP.md` — active direction for majority parity with
  `protoc-gen-go-temporal`, including current unsupported features and phase
  priorities.
- `WIRE-FORMAT.md` — the `(encoding, messageType, data)` Payload triple. Pinned at v1; must stay byte-identical to the TS sibling and to cludden's Go runtime.
- `docs/RUNTIME-API.md` — every symbol the generated code calls from the consumer-supplied `crate::temporal_runtime` facade, and when each emit branch fires.
- `docs/sdk-shape.md` — verified deviations between `temporalio-sdk` 0.4's spec and reality.
- `docs/SUPPORT-STATUS.md` — single index of every `temporal.v1.*` annotation
  field with its current support status (supported / rejected / intentionally
  ignored). Required reading before adding or relaxing a rejection rule.
- `examples/job-queue/AGENTS.md` — example-specific guidance for the real
  integration scenario that exercises generated clients, worker contracts, the
  HTTP API, and CLI usage together.

## Workspace layout

| Path | Role |
|---|---|
| `crates/protoc-gen-rust-temporal/` | The plugin binary + library. Pipeline: `parse → validate → render → CodeGeneratorResponse`. |
| `crates/temporal-proto-runtime/` | Optional consumer-facing helper: `TemporalProtoMessage` trait + `TypedProtoMessage<T>` wrapper. `sdk` feature pulls `temporalio-common` and ships the `TemporalSerializable`/`TemporalDeserializable` impls (orphan-rule workaround). |
| `crates/temporal-proto-runtime-bridge/` | Default `crate::temporal_runtime` facade backed by `temporalio-client 0.4`, plus worker/CLI feature re-exports. |
| `compat-tests/` | Cross-language Phase 3 audit. Rust + Go arms each emit a Payload JSON for the same fixture input; CI diffs them. |
| `examples/job-queue/` | Primary end-to-end example with generated client, Temporal worker, axum HTTP API, and clap CLI. |
| `crates/protoc-gen-rust-temporal/tests/fixtures/` | Per-emit-branch fixtures (`minimal_workflow`, `workflow_only`, `full_workflow`, `activity_only`, `empty_input_workflow`, `multiple_workflows`). |

## Common commands

```bash
# Full workspace check (matches CI):
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets

# Single integration test:
cargo test -p protoc-gen-rust-temporal --test parse_validate
cargo test -p protoc-gen-rust-temporal --test protoc_invoke -- minimal_workflow_via_protoc_matches_in_process_render

# MSRV check (Rust 1.88, pinned by rust-toolchain.toml + workspace.package.rust-version):
cargo +1.88 build --workspace --all-targets

# Run the plugin manually (e.g. against a local proto tree):
cargo build -p protoc-gen-rust-temporal
protoc --plugin=protoc-gen-rust-temporal=./target/debug/protoc-gen-rust-temporal \
       -Iyour/protos -Icrates/protoc-gen-rust-temporal/proto \
       --rust-temporal_out=out_dir your/protos/foo.proto

# Wire-format compat audit (both arms must produce byte-identical Payload JSON):
cargo run -p compat-tests-rust -- generate
(cd compat-tests/go && \
   protoc --proto_path=../rust/proto --go_out=gen --go_opt=paths=source_relative jobs/v1/jobs.proto && \
   go run . generate)
for f in compat-tests/fixtures/*.rust.payload.json; do
  diff -u "$f" "${f%.rust.payload.json}.go.payload.json"
done
```

Rust build scripts and Rust integration tests use `protoc-bin-vendored` by
default. Override fixture-driven `protoc` invocations with
`PROTOC=/path/to/protoc` when checking compatibility with a specific binary.
The Go compat arm still needs `protoc` on `PATH` for `--go_out`.

`buf` must be on `PATH` for the plugin's `cargo build` — the build script runs
`buf export` against cludden's BSR module to pull the annotation schema (see
the architecture section below). Offline builds: set `VENDORED_SCHEMA=1` to
fall back to the in-tree `proto/` copy.

## Architecture: how the plugin works end-to-end

1. **`main.rs`** reads `CodeGeneratorRequest` from stdin. It decodes the
   request *twice*: once via `prost-types` to get `file_to_generate`, and
   once by hand-walking the request's wire bytes (`extract_proto_file_blobs`)
   to recover each `FileDescriptorProto` blob with its extensions intact.
   **This is load-bearing**: prost-types silently drops unknown extensions
   on `MethodOptions`/`ServiceOptions`, which would erase every
   `temporal.v1.*` annotation. The reconstructed `FileDescriptorSet` is
   fed into `DescriptorPool::decode_file_descriptor_set`, which preserves
   extensions as unknown-field bytes that `prost-reflect` can then
   re-interpret via the schema compiled into the plugin at build time.

2. **`build.rs`** fetches cludden's annotation schema from BSR via
   `buf export buf.build/cludden/protoc-gen-go-temporal:<commit-digest>`
   into `OUT_DIR/cludden-schema/`, then runs `prost-build` over the
   resulting tree to generate typed Rust under `OUT_DIR`. The pinned digest
   lives in `build.rs::CLUDDEN_BSR_COMMIT` (matches the human-readable label
   `v1.22.1`); BSR commits are immutable, BSR labels and git tags are not,
   so we pin to the digest. `lib.rs` re-exports the generated types under
   `temporal::v1::*` and `temporal::api::enums::v1::*` so the parser can
   round-trip extension `Value`s through `DynamicMessage` and decode them
   into strongly-typed `WorkflowOptions` / `SignalOptions` / etc. (see
   `parse.rs`). The in-tree `proto/` directory is retained as the
   `VENDORED_SCHEMA=1` fallback for offline / air-gapped builds; the tests'
   `protoc -I` paths still resolve `import "temporal/v1/temporal.proto"`
   against it.

3. **`parse.rs`** walks `ServiceDescriptor`s in `files_to_generate`,
   pulls `temporal.v1.{service,workflow,activity,signal,query,update}`
   extensions off each `MethodOptions` / `ServiceOptions`, and produces
   `model::ServiceModel`. The current emit model mostly treats each method as
   one primary generated kind. Do not assume that is a permanent schema rule:
   cludden's Go generator supports useful co-annotations, and `ROADMAP.md`
   tracks moving the Rust model toward that parity.

4. **`validate.rs`** enforces cross-method invariants: every
   `WorkflowOptions.{signal,query,update}.ref` must currently point to an
   actual annotated method on the same service; `signal` methods must return
   `google.protobuf.Empty`; workflows need a `task_queue` (either inline
   or from the service-level default). Cross-service refs and Go-compatible
   co-annotation semantics are roadmap work, not invalid design directions.

5. **`render.rs`** emits one `<stem>_temporal.rs` per input proto containing
   a `<Service>Client` struct and one `<Workflow>Handle` per workflow rpc.
   Emitted code calls into a consumer-supplied `crate::temporal_runtime`
   facade — every symbol it references is documented in
   `docs/RUNTIME-API.md`. Generated code must compile under
   `clippy -D warnings`.

## Wire format invariant

Generated clients (and the `temporal-proto-runtime` helper) speak exactly:

```
metadata.encoding    = "binary/protobuf"
metadata.messageType = "<fully.qualified.proto.message.name>"
data                 = raw proto wire bytes
```

This triple is the **only** thing this repo owns about the on-wire
contract, and it must stay byte-identical to (a) the TS sibling and (b)
cludden's Go runtime. Any change here breaks cross-language interop and
must be reflected in `WIRE-FORMAT.md`, the TS sibling, and re-audited
via `compat-tests/` (the `compat-audit` CI job re-runs both arms on
every PR). Empty inputs use `google.protobuf.Empty` rather than null
payloads, so the decode path is uniform.

## Testing strategy

Two layers, both required to stay green:

- **`tests/parse_validate.rs`** — uses vendored `protoc` to produce a
  `FileDescriptorSet` from each fixture, then runs the pipeline in-process.
  Cheap, gives precise error messages, exercises validation paths.
- **`tests/protoc_invoke.rs`** — invokes the compiled plugin binary
  through `protoc --plugin=...` and diffs on-disk output against the
  in-process render. Catches stdin/stdout framing regressions and
  `CodeGeneratorResponse.error` plumbing that the in-process tests miss.

## Pull request interop CI

`.github/workflows/interop.yml` runs on every pull request and on pushes to
`main`. This is the cross-repository compatibility gate for the Rust generator:
it builds the local `protoc-gen-rust-temporal` binary from the PR checkout, then
clones `nu-sync/protoc-gen-temporal-interop` and runs:

```bash
cargo run -p interop-harness -- test
```

The workflow passes:

- `RUST_TEMPORAL_PLUGIN` — the locally built plugin binary.
- `RUST_TEMPORAL_WORKSPACE` — this checkout, so the generated worker is patched
  to use the local `temporal-proto-runtime` and
  `temporal-proto-runtime-bridge` crates.
- `TS_TEMPORAL_VERSION` — the pinned remote TypeScript generator version.

So this repo's PR CI tests **local Rust generator/runtime/bridge** against the
**remote pinned TypeScript generator**, not local Rust against remote Rust. The
harness generates both sides, starts a real Temporal dev server, starts the
generated Rust worker, and drives it with the generated TypeScript client.

The workflow installs both `buf` and `protoc`; the pinned TypeScript generator
build needs `google.protobuf.descriptor.proto` from the protobuf distribution.
On failure, inspect the printed `interop/.dev-logs/*.log` groups or download
the `interop-dev-logs` artifact.

The workflow clones the shared harness before restoring caches so GitHub Actions
can reuse both Cargo workspaces. It also caches the harness `.dev-tools`
directory, npm's package cache for `interop/ts-client`, and the Temporal CLI
download used by `temporalio-sdk-core`'s ephemeral dev server. When changing
this workflow, measure one cold run and one warm rerun with:

```bash
gh run view <run-id> --repo nu-sync/protoc-gen-rust-temporal --json jobs
gh run view <run-id> --repo nu-sync/protoc-gen-rust-temporal --job <job-id> --log
```

Compare the `ts-client-to-rust-worker` total duration, the `Run interop
harness` step duration, and cache-hit lines for npm, `actions/cache`, and
`Swatinem/rust-cache`.

When adding a new emit branch:
1. Add a fixture under `tests/fixtures/<name>/input.proto`.
2. Cover the parse + validate path in `parse_validate.rs`.
3. If it changes user-visible generated code, also exercise it through
   `protoc_invoke.rs` and update `examples/job-queue/` so the realistic
   consumer still compiles.
4. Update `docs/RUNTIME-API.md` if any new symbol on the
   `crate::temporal_runtime` facade gets emitted.

## Things that look weird but are intentional

- **`main.rs`'s hand-rolled wire-format walker.** Not a layering shortcut.
  `prost-types::CodeGeneratorRequest::decode` is structurally lossy for
  extensions on `MethodOptions`; the manual walk is the workaround
  validated by the PoC. See the comment above `extract_proto_file_blobs`.
- **`temporal-proto-runtime`'s `sdk` feature flag.** Default build is the
  trait + wrapper only, no SDK dependency. The `sdk` feature pulls
  `temporalio-common = "0.4"` and lands the `TemporalSerializable` /
  `TemporalDeserializable` impls in this crate so the Rust orphan rule
  doesn't force every consumer to redefine `TypedProtoMessage` locally.
- **MSRV is 1.88** (`rust-toolchain.toml` + `workspace.package.rust-version`).
  CI runs an explicit `cargo +1.88 build` matrix step; don't silently
  rely on newer std/stable features.
- **One module per source proto.** Output is `<stem>_temporal.rs` so
  consumer build scripts can `include!` deterministically.
- **Worker emit is contract-only today.** The plugin emits typed worker
  contracts and registration helpers; the user's workflow/activity bodies still
  live in application code because `temporalio-sdk` registration is
  macro-shaped. `ROADMAP.md` tracks richer generated worker implementation
  surface and activity execution helpers as priority parity work.
