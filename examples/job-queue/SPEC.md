# job-queue Example Spec

## Purpose

`examples/job-queue` is the repository's realistic consumer example for
`protoc-gen-rust-temporal`. It demonstrates the plugin's value in the case
that matters most: multiple unrelated Rust programs sharing one proto-defined
Temporal workflow contract.

## Success Criteria

1. `cargo check -p jobs-proto -p job-worker -p job-api -p jobctl --all-targets`
   passes from the repository root.
2. `cargo test -p jobs-proto -p job-worker -p job-api -p jobctl --all-features`
   passes without a live Temporal server.
3. `cd examples/job-queue && just gen` regenerates checked-in files with the
   repo-local `protoc-gen-rust-temporal` binary.
4. The optional `just demo` E2E path proves CLI and HTTP API consumers can
   both drive the same Temporal worker against a local Temporal CLI dev server.
5. Renaming a field in `proto/jobs/v1/jobs.proto`, regenerating, and building
   creates compile errors at every API/CLI call site that needs to change.

## Architecture

```text
proto/jobs/v1/jobs.proto
        |
        | buf generate
        |
        +-- protoc-gen-prost -> crates/jobs-proto/src/gen/jobs/v1/jobs.v1.rs
        |
        +-- protoc-gen-rust-temporal
              -> crates/jobs-proto/src/gen/jobs/v1/jobs_temporal.rs

crates/jobs-proto
        |
        +-- job-worker  # Temporal worker hosting RunJob
        +-- job-api     # axum HTTP API on :3030
        +-- jobctl      # clap CLI talking directly to Temporal
```

`jobs-proto` re-exports this repository's bridge crate as
`crate::temporal_runtime`, so generated code exercises the same default
runtime facade documented in `docs/RUNTIME-API.md`.

## Workflow Contract

`JobService.RunJob` starts a workflow on task queue `jobs` with workflow IDs
derived from the input name (`job-{{ .Name }}`). The workflow exposes:

- `CancelJob` signal
- `GetStatus` query
- `PrepareWorkspace`, `ExecuteCommand`, and `CollectOutput` activities

The worker implementation keeps command execution stubbed. The example tests
the typed contract and Temporal wiring, not shell execution semantics.

## Validation Levels

Fast local validation:

```bash
just check
just test
just verify
```

Generation validation:

```bash
just gen
git diff -- examples/job-queue/crates/jobs-proto/src/gen
```

Optional live E2E validation:

```bash
just up
just worker
just api
just demo
just down
```

`just up` starts:

```bash
temporal server start-dev \
  --namespace default \
  --ip 127.0.0.1 \
  --port 7233 \
  --ui-port 8233
```

The optional E2E path requires the `temporal` CLI and a live local Temporal
server. It is not part of the default fast validation path.
