# job-queue example

This is the primary end-to-end example for `protoc-gen-rust-temporal`.
It shows one annotated proto driving a real Rust Temporal worker plus two
unrelated Rust consumers: an axum HTTP API and a clap CLI.

The example is owned by the repository root workspace. There is intentionally
no `examples/job-queue/Cargo.toml`; the root `Cargo.toml` lists the four
example crates directly.

## Layout

```text
examples/job-queue/
├── proto/                  # buf module with jobs.v1 and vendored annotations
├── crates/
│   ├── jobs-proto          # prost types + generated Temporal client
│   ├── job-worker          # Temporal worker hosting RunJob
│   ├── job-api             # axum HTTP API on :3030
│   └── jobctl              # clap CLI that talks directly to Temporal
├── scripts/                # optional dev/e2e helpers
└── justfile                # example-local recipes
```

## Fast Checks

Run these from the repository root:

```bash
cargo check -p jobs-proto -p job-worker -p job-api -p jobctl --all-targets
cargo test -p jobs-proto -p job-worker -p job-api -p jobctl --all-features
```

Or from this directory:

```bash
just check
just test
just verify
```

These checks do not require a live Temporal server.

## Regenerate

Prerequisites:

- `protoc`
- `buf`
- `protoc-gen-prost` (`cargo install protoc-gen-prost`)

Then run:

```bash
just gen
```

`just gen` builds the repo-local `protoc-gen-rust-temporal` binary first,
prepends `target/debug` to `PATH`, and runs `buf generate` in `proto/`.
Generated files land under `crates/jobs-proto/src/gen/`.

## Run The Demo

The full demo uses the local Temporal CLI dev server. Install the `temporal`
CLI and start the stack:

```bash
# Terminal 1
just up

# Terminal 2
just worker

# Terminal 3
just api
```

`just up` runs:

```bash
temporal server start-dev \
  --namespace default \
  --ip 127.0.0.1 \
  --port 7233 \
  --ui-port 8233
```

The Temporal frontend listens on `localhost:7233`, the Temporal UI listens on
`http://localhost:8233`, and the API listens on `http://localhost:3030`.
Dev-server state is temporary and is lost when the Temporal process exits.

Smoke test the two consumers:

```bash
cargo run -q -p jobctl -- submit --name lint --command "cargo clippy"
curl -s http://localhost:3030/jobs/<workflow_id> | jq
cargo run -q -p jobctl -- cancel <workflow_id> --reason "manual"
```

The scripted E2E path is explicit because it depends on a live Temporal server,
worker, and API:

```bash
just demo
# or
uv run --script scripts/dev.py e2e
```

Tear down with:

```bash
just down
```

## What This Proves

- `proto/jobs/v1/jobs.proto` is the single contract.
- `jobs-proto` contains both prost message types and the generated typed
  Temporal client.
- `jobs-proto` wires generated code to this repo's
  `temporal-proto-runtime-bridge` with:

  ```rust
  pub use temporal_proto_runtime_bridge as temporal_runtime;
  ```

- `job-worker`, `job-api`, and `jobctl` all compile against the same generated
  Rust surface.
- Renaming a proto field and running `just gen` produces compile errors at
  every drift point in the API and CLI.
