# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.1] — 2026-05-12

First name-claiming release on crates.io and BSR.

### Added
- Phase 4 scaffolding: `Dockerfile` for the BSR remote plugin,
  `buf.plugin.yaml` manifest, GitHub Actions release workflow building
  prebuilt binaries for `{x86_64,aarch64}-unknown-linux-gnu`, plus
  publish hooks for crates.io and the BSR. macOS / Windows targets are
  parked until downstream demand justifies the runner cost.
- Phase 5 example: `examples/job-queue-integration/` is now a workspace
  member that prost-builds the example's `jobs.v1` types and compiles
  the plugin's rendered output end-to-end against the documented
  `temporal_runtime` facade. `cargo check --workspace` covers the full
  pipeline.
- Plugin emits `impl temporal_runtime::TemporalProtoMessage` for every
  prost message type the rendered client surface touches. Consumers no
  longer hand-write the wire-format trait impls.
- Plugin binary supports `--version` / `--help` so installed binaries
  are diagnosable from a shell.
- Six golden fixtures pin the rendered output across major emit paths
  (`minimal_workflow`, `workflow_only`, `multiple_workflows`,
  `full_workflow`, `empty_input_workflow`, `activity_only`).
- End-to-end test (`tests/protoc_invoke.rs`) drives the plugin through
  real `protoc` and diffs the on-disk output against the in-process
  render.

### Fixed
- `update_with_start_workflow_proto` now uses three explicit generics
  (`W`, `U`, `O`); the previous signature conflated the workflow input
  and the update input under a single type parameter and would have
  refused at the consumer's call site.
- Empty-input workflows route to `start_workflow_proto_empty` instead
  of emitting `&()` against a `TemporalProtoMessage`-constrained
  generic.

## [0.0.0] — 2026-05-12

### Added
- **Phase 0 — Repo bootstrap.** Workspace `Cargo.toml`, plugin crate
  skeleton, vendored copy of cludden's `temporal/v1/temporal.proto` schema
  (and transitively required `temporal/api/enums/v1/workflow.proto` enums),
  `build.rs` compiling the annotation proto via `prost-build`. MIT license,
  README, `WIRE-FORMAT.md` (authoritative copy; TS sibling mirrors).
  `docs/sdk-shape.md` ported from the TS demo. CI workflow with fmt /
  clippy `-D warnings` / test / MSRV 1.85 build. Initial commit tagged
  `v0.0.0`.
- **Phase 1 — Parse cludden's schema.** `parse.rs` walks the descriptor
  pool and recognises all six annotation buckets
  (`service`, `workflow`, `activity`, `signal`, `query`, `update`) at
  cludden's field numbers 7233–7237. `validate.rs` rejects rpc-name
  collisions across kinds, missing `task_queue` (with service-default
  fallback), bad ref resolution, and non-Empty signal outputs. Integration
  tests invoke real `protoc` against a fixture proto and inline negative
  cases.
- **Phase 2 — Render Rust client surface.** `render.rs` emits one
  `<package>_<service>_temporal` module per service: workflow constants,
  `<Service>Client` struct, per-workflow `<Workflow>StartOptions` and
  `<Workflow>Handle`, signal/query/update methods, and
  `<signal>_with_start` / `<update>_with_start` free functions when the
  matching ref carries `start: true`. Generated code references
  `crate::temporal_runtime::*` (consumer-supplied facade) so plugin output
  is stable across upstream SDK churn. Render smoke test covers the key
  fragments emitted for the `minimal_workflow` fixture.

### Notes
- Wire-format audit against cludden's Go runtime is pending (Phase 3).
- BSR Remote Plugin (`buf.build/nu-sync/rust-temporal`) and crates.io
  publish are gated on the `nu-sync` org existing. See `SPEC.md` for the
  full phased delivery plan.
