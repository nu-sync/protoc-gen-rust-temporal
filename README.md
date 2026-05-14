# protoc-gen-rust-temporal

[![crates.io](https://img.shields.io/crates/v/protoc-gen-rust-temporal.svg)](https://crates.io/crates/protoc-gen-rust-temporal)
[![docs.rs](https://docs.rs/protoc-gen-rust-temporal/badge.svg)](https://docs.rs/protoc-gen-rust-temporal)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

A `protoc` plugin that reads
[cludden/protoc-gen-go-temporal](https://github.com/cludden/protoc-gen-go-temporal)
annotations (`temporal.v1.*`) from a proto service definition and emits a
typed **Rust** [Temporal](https://temporal.io) client.

> **Status:** Phase 5 complete. The plugin emits typed client, update,
> signal-with-start, activity, workflow-registration, and CLI scaffold code;
> the job-queue external consumer has migrated to the generated contracts.
> The project direction is now majority parity with
> `protoc-gen-go-temporal`; see [`ROADMAP.md`](./ROADMAP.md).

## What it does

Given a service annotated for the Go plugin —

```proto
service JobService {
  rpc RunJob(JobInput) returns (JobOutput) {
    option (temporal.v1.workflow) = {
      task_queue: "jobs"
      id: "{{ .name }}"
    };
  }
}
```

— `protoc-gen-rust-temporal` generates a `JobServiceClient` exposing a typed
`run_job` method returning a `RunJobHandle`, plus signal/query/update methods
mirroring the proto. The intended sibling is
[`protoc-gen-ts-temporal`](https://github.com/nu-sync/protoc-gen-ts-temporal),
so one annotated proto produces Go, TS, and Rust clients with zero proto
changes.

## Roadmap

The current Rust generator is not full Go-plugin parity. The active roadmap is
to prioritize the most useful parity surfaces first: richer generated worker
implementation contracts, typed activity execution helpers from workflows,
broader client operations, and runtime option coverage. Unsupported features
should be explicit while they move through that plan.

See [`ROADMAP.md`](./ROADMAP.md) for the phase order and current unsupported
items.

## Quickstart

Install from crates.io:

```bash
cargo install protoc-gen-rust-temporal
```

Then point `buf.gen.yaml` at the local binary:

```yaml
# buf.gen.yaml
version: v2
plugins:
  - local: protoc-gen-rust-temporal
    out: src/gen
```

The BSR remote-plugin form (`remote: buf.build/nu-sync/rust-temporal`)
will land once the [curated-plugin PR](docs/bsr-publish.md) is merged
into `bufbuild/plugins`.

```yaml
# buf.gen.yaml
version: v2
plugins:
  - remote: buf.build/nu-sync/rust-temporal
    out: src/gen
```

Add the runtime helper to your crate's `Cargo.toml`:

```toml
[dependencies]
temporal-proto-runtime = "0.1"
```

## Wire format

Generated clients speak `binary/protobuf` end-to-end. See
[`WIRE-FORMAT.md`](./WIRE-FORMAT.md) for the contract; the sibling TS plugin
keeps a byte-identical mirror.

## Runtime API

Generated code calls into a consumer-supplied `crate::temporal_runtime`
module. Every function the plugin emits a call to, when it gets emitted,
and the exact signature it expects are documented in
[`docs/RUNTIME-API.md`](./docs/RUNTIME-API.md). The
[`examples/job-queue/`](./examples/job-queue/) workspace example shows the
default bridge wired into a real worker, HTTP API, and CLI.

## Consumer wiring (default)

Add the plugin's runtime helpers + the default bridge crate:

```toml
[dependencies]
temporal-proto-runtime = { version = "0.1", features = ["sdk"] }
temporal-proto-runtime-bridge = "0.1"
```

Then in your crate's `lib.rs`:

```rust,ignore
pub use temporal_proto_runtime_bridge as temporal_runtime;
```

That single re-export satisfies every `crate::temporal_runtime::*` reference
the plugin emits — the bridge crate ships a concrete impl backed by
`temporalio-client 0.4`. Power users who need a custom transport, vendored
SDK, or test stub can drop the `pub use` and write `mod temporal_runtime;`
against the facade documented in [`docs/RUNTIME-API.md`](./docs/RUNTIME-API.md).

## Layout

| Crate / file | Role |
|---|---|
| `crates/protoc-gen-rust-temporal/` | The plugin binary + library. |
| `crates/temporal-proto-runtime/` | `TypedProtoMessage<T>` runtime helper used by generated code. |
| `crates/temporal-proto-runtime-bridge/` | Default `temporal_runtime` facade impl backed by `temporalio-client 0.4`. |
| `examples/job-queue/` | Primary end-to-end example: generated client, worker, HTTP API, and CLI. |
| `proto/temporal/v1/temporal.proto` | Vendored copy of cludden's annotation schema. |
| `SPEC.md` | Design spec + phased delivery plan. |
| `ROADMAP.md` | Active majority-parity direction and unsupported-feature plan. |
| `WIRE-FORMAT.md` | Pinned wire-format contract. |
| `docs/sdk-shape.md` | Pinned reference for `temporalio-sdk` 0.4 quirks. |

## License

MIT. See [`LICENSE`](./LICENSE).
