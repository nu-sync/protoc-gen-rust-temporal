# protoc-gen-rust-temporal

[![status](https://img.shields.io/badge/status-Phase%200%20bootstrap-orange)](./SPEC.md)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

A `protoc` plugin that reads
[cludden/protoc-gen-go-temporal](https://github.com/cludden/protoc-gen-go-temporal)
annotations (`temporal.v1.*`) from a proto service definition and emits a
typed **Rust** [Temporal](https://temporal.io) client.

> **Status:** Phase 0 / bootstrap. The repo carries the scaffolding,
> vendored annotation schema, and wire-format spec. Code-emission is wired
> in later phases — see [`SPEC.md`](./SPEC.md) for the delivery plan.

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

## Quickstart (Phase 4)

```yaml
# buf.gen.yaml
version: v2
plugins:
  - remote: buf.build/nu-sync/rust-temporal
    out: src/gen
```

Or install locally:

```bash
cargo install protoc-gen-rust-temporal
```

```yaml
# buf.gen.yaml
plugins:
  - local: protoc-gen-rust-temporal
    out: src/gen
```

Add the runtime helper to your crate's `Cargo.toml`:

```toml
[dependencies]
temporal-proto-runtime = "0.0"
```

## Wire format

Generated clients speak `binary/protobuf` end-to-end. See
[`WIRE-FORMAT.md`](./WIRE-FORMAT.md) for the contract; the sibling TS plugin
keeps a byte-identical mirror.

## Layout

| Crate / file | Role |
|---|---|
| `crates/protoc-gen-rust-temporal/` | The plugin binary + library. |
| `crates/temporal-proto-runtime/` | `TypedProtoMessage<T>` runtime helper used by generated code. |
| `proto/temporal/v1/temporal.proto` | Vendored copy of cludden's annotation schema. |
| `SPEC.md` | Design spec + phased delivery plan. |
| `WIRE-FORMAT.md` | Pinned wire-format contract. |
| `docs/sdk-shape.md` | Pinned reference for `temporalio-sdk` 0.4 quirks. |

## License

MIT. See [`LICENSE`](./LICENSE).
