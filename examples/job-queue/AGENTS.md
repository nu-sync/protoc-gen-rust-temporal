# Repository Guidelines

## Project Structure & Module Organization

This directory is the `job-queue` example for `protoc-gen-rust-temporal`. It has no local `Cargo.toml`; the root workspace owns these crates.

- `proto/` contains the Buf module, the `jobs.v1` service contract, and vendored Temporal annotations.
- `crates/jobs-proto/` contains prost types and generated Temporal client code under `src/gen/`.
- `crates/job-worker/` hosts the Temporal worker implementation.
- `crates/job-api/` exposes the Axum HTTP API on port `3030`.
- `crates/jobctl/` provides the Clap CLI.
- `scripts/` and `justfile` provide local development, codegen, and E2E helpers.

## Build, Test, and Development Commands

Run commands from `examples/job-queue/` unless noted.

- `just check` runs `cargo check` for all four example crates.
- `just test` runs the example test suite with all features enabled.
- `just lint` runs Clippy with `-D warnings`.
- `just verify` runs build, lint, and tests.
- `just gen` rebuilds the local plugin and runs `buf generate`; use this after changing `proto/jobs/v1/jobs.proto`.
- `just up`, `just worker`, and `just api` start Temporal, the worker, and the API.
- `just demo` runs the scripted E2E flow; `just down` stops managed processes.

Required tools for regeneration are `buf` and `protoc-gen-prost`; the Rust
plugin itself uses vendored `protoc` for its own build/test codegen.

## Coding Style & Naming Conventions

Use Rust 2024 with MSRV 1.88. Keep code simple and linear, prefer meaningful names, and follow existing crate boundaries. Use `cargo fmt` or `just fmt` before submitting. Generated code belongs in `crates/jobs-proto/src/gen/`; do not hand-edit it.

Use Rust module and file names in `snake_case`, crate names in kebab-case, and proto package paths that mirror `jobs/v1`.

## Testing Guidelines

Use standard Rust unit tests near the code under test and integration tests under `crates/*/tests/`. Existing examples include `create_job_request_defaults_timeout`, `build_marker`, and `generated_temporal_e2e`.

For narrow changes, run the smallest relevant package test first. For proto or generated surface changes, run `just gen`, then `just verify`. Validate Temporal-dependent demo behavior with `just demo`.

## Commit & Pull Request Guidelines

Recent commits use concise scoped subjects such as `plugin: R6 - ...`; prefer `scope: summary` and keep the first line specific. PRs should describe the user-visible change, list verification commands, and mention any proto or generated-code updates. Link related issues when applicable and include API examples or logs for behavior changes.

## Security & Configuration Tips

Do not commit secrets, local Temporal state, `.dev-logs/`, `.dev-pids/`, `target/`, or virtual environments. Keep runtime configuration local through environment variables and scripts.
