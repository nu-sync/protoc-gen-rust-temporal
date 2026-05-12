# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

`protoc-gen-rust-temporal` is a `protoc` plugin that consumes
[`cludden/protoc-gen-go-temporal`](https://github.com/cludden/protoc-gen-go-temporal)'s
`temporal.v1.*` annotations from a proto service and emits a typed **Rust**
[Temporal](https://temporal.io) client. We supply the Rust code generator;
cludden owns the annotation schema, and the schema is vendored at
`crates/protoc-gen-rust-temporal/proto/temporal/v1/temporal.proto`.

Sibling project: `nu-sync/protoc-gen-ts-temporal`. Wire format is
intentionally byte-identical (see `WIRE-FORMAT.md`) so one annotated proto
produces Go, TS, and Rust clients with no proto changes.

**v1 emits clients only.** Worker-side workflow/activity bodies are
hand-written against `temporalio-sdk`; the plugin only validates `activity`
annotations, it does not generate worker code.

## Required reading before non-trivial changes

- `SPEC.md` — design, scope, phased delivery plan, what's intentionally out of scope.
- `WIRE-FORMAT.md` — the `(encoding, messageType, data)` Payload triple. Pinned at v1; must stay byte-identical to the TS sibling and to cludden's Go runtime.
- `docs/RUNTIME-API.md` — every symbol the generated code calls from the consumer-supplied `crate::temporal_runtime` facade, and when each emit branch fires.
- `docs/sdk-shape.md` — verified deviations between `temporalio-sdk` 0.4's spec and reality.

## Workspace layout

| Path | Role |
|---|---|
| `crates/protoc-gen-rust-temporal/` | The plugin binary + library. Pipeline: `parse → validate → render → CodeGeneratorResponse`. |
| `crates/temporal-proto-runtime/` | Optional consumer-facing helper: `TemporalProtoMessage` trait + `TypedProtoMessage<T>` wrapper. `sdk` feature pulls `temporalio-common` and ships the `TemporalSerializable`/`TemporalDeserializable` impls (orphan-rule workaround). |
| `compat-tests/` | Cross-language Phase 3 audit. Rust + Go arms each emit a Payload JSON for the same fixture input; CI diffs them. |
| `examples/job-queue-integration/` | Workspace-member reference consumer. Provides the canonical `temporal_runtime` facade stub (`src/temporal_runtime.rs`) that `cargo check`s clean — copy it into new consumer crates. |
| `crates/protoc-gen-rust-temporal/tests/fixtures/` | Per-emit-branch fixtures (`minimal_workflow`, `workflow_only`, `full_workflow`, `activity_only`, `empty_input_workflow`, `multiple_workflows`). |

## Common commands

```bash
# Full workspace check (matches CI):
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings    # requires protoc on PATH
cargo test --workspace --all-targets                     # requires protoc on PATH

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

`protoc` must be on `PATH` for the test suite and clippy — fixtures shell out
to the real `protoc` to build descriptor sets. Override with `PROTOC=/path/to/protoc`.

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

2. **`build.rs`** compiles the vendored `temporal/v1/temporal.proto` (plus
   transitively-required `temporal.api.enums.v1` enums) via `prost-build`
   into typed Rust under `OUT_DIR`. `lib.rs` re-exports those under
   `temporal::v1::*` and `temporal::api::enums::v1::*` so the parser can
   round-trip extension `Value`s through `DynamicMessage` and decode them
   into strongly-typed `WorkflowOptions` / `SignalOptions` / etc. (see
   `parse.rs`).

3. **`parse.rs`** walks `ServiceDescriptor`s in `files_to_generate`,
   pulls `temporal.v1.{service,workflow,activity,signal,query,update}`
   extensions off each `MethodOptions` / `ServiceOptions`, and produces
   `model::ServiceModel`. Method-level annotations are mutually exclusive
   (a method is exactly one of workflow / signal / query / update / activity).

4. **`validate.rs`** enforces cross-method invariants: every
   `WorkflowOptions.{signal,query,update}.ref` must point to an actual
   annotated method on the same service; `signal` methods must return
   `google.protobuf.Empty`; workflows need a `task_queue` (either inline
   or from the service-level default); activity-annotated method names
   can't collide with workflow/signal/query/update names.

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

- **`tests/parse_validate.rs`** — shells out to `protoc` to produce a
  `FileDescriptorSet` from each fixture, runs the pipeline in-process.
  Cheap, gives precise error messages, exercises validation paths.
- **`tests/protoc_invoke.rs`** — invokes the compiled plugin binary
  through `protoc --plugin=...` and diffs on-disk output against the
  in-process render. Catches stdin/stdout framing regressions and
  `CodeGeneratorResponse.error` plumbing that the in-process tests miss.

When adding a new emit branch:
1. Add a fixture under `tests/fixtures/<name>/input.proto`.
2. Cover the parse + validate path in `parse_validate.rs`.
3. If it changes user-visible generated code, also exercise it through
   `protoc_invoke.rs` and update `examples/job-queue-integration/` so the
   facade still compiles.
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
- **No worker-side emit in v1.** `activity` annotations are validated but
  produce no Rust code; this is intentional (see `SPEC.md` non-goals).
