# Wire-format compatibility audit

This directory holds the cross-language harness referenced as **Phase 3** in
[`../SPEC.md`](../SPEC.md). Goal: verify that `protoc-gen-rust-temporal`'s
generated clients speak the same `(encoding, messageType, data)` Payload
triple as [cludden's `protoc-gen-go-temporal`](https://github.com/cludden/protoc-gen-go-temporal)
runtime. If the audit passes, a Go client written against cludden's plugin
can drive a Rust worker registered with ours (and vice versa) with no
adapter code.

**Status: passed (2026-05-12)** against `cludden/protoc-gen-go-temporal@v1.22.1`
and `go.temporal.io/sdk@v1.43.0`. The CI `compat-audit` job re-runs both
arms on every PR and fails on any non-empty diff.

## What "passes" means

For every fixture in `fixtures/`:

1. The Rust side reads `<fixture>.input.json`, populates a typed prost
   message, runs it through `TypedProtoMessage<T>`, and writes the
   resulting `Payload` to `<fixture>.rust.payload.json` (the same shape
   `temporal.api.common.v1.Payload` produces over the wire).
2. The Go side reads the same `<fixture>.input.json`, populates the
   equivalent generated Go message, runs it through cludden's runtime
   data converter, and writes `<fixture>.go.payload.json`.
3. The diff between `<fixture>.rust.payload.json` and
   `<fixture>.go.payload.json` is empty.

The triple lives in
[`../WIRE-FORMAT.md`](../WIRE-FORMAT.md). Both sides must produce the same
three slots:

| Slot                   | Expected value                                |
|------------------------|-----------------------------------------------|
| `metadata.encoding`    | `"binary/protobuf"`                           |
| `metadata.messageType` | `"<fully.qualified.proto.message.name>"`     |
| `data`                 | raw proto wire bytes (base64-encoded in JSON) |

## Running the audit

Both arms are runnable locally. The Go arm pins
`github.com/cludden/protoc-gen-go-temporal@v1.22.1` as a record of which
plugin version this audit was certified against; it imports the Temporal
Go SDK's converter directly because cludden's plugin does not override the
SDK converter chain (auditing the SDK converter at the version cludden
targets is equivalent to auditing cludden's wire format — see the
`compat-audit` CI job and `../WIRE-FORMAT.md` "Compatibility audit" for the
chain of reasoning).

```bash
# Rust arm.
cargo run -p compat-tests-rust -- generate
# writes fixtures/*.rust.payload.json

# Go arm. Requires protoc + protoc-gen-go on PATH (a one-time `go install
# google.golang.org/protobuf/cmd/protoc-gen-go@latest` is enough).
cd compat-tests/go
protoc --proto_path=../rust/proto --go_out=gen --go_opt=paths=source_relative jobs/v1/jobs.proto
go run . generate
# writes fixtures/*.go.payload.json
```

Then diff:

```bash
for f in compat-tests/fixtures/*.rust.payload.json; do
  diff -u "$f" "${f%.rust.payload.json}.go.payload.json"
done
```

## Fixture format

`<fixture>.input.json`:

```json
{
  "message_type": "jobs.v1.JobInput",
  "fields": {
    "name": "demo"
  }
}
```

`message_type` is the fully-qualified proto name and `fields` is the
serde-JSON form of the message. Both ends parse the JSON into their
respective generated types using protobuf's JSON mapping
(`prost`'s `serde` feature on the Rust side; `protojson` on the Go side),
then serialise via prost / `proto.Marshal` and assemble the Payload.

## Status

- [x] Harness skeleton + README (this file).
- [x] Rust generator binary (`compat-tests/rust/`).
- [x] Go generator (`compat-tests/go/`).
- [x] Fixture set covering scalar, `google.protobuf.Empty`, nested message,
      and repeated message (including an empty element).
- [x] CI job that runs both generators and diffs (`compat-audit` in
      `.github/workflows/ci.yml`).

### Fast-follow fixtures

The current set covers the structural decisions in the triple. Future
additions to harden the audit further:

- `oneof` field with multiple arms exercised (catches tag-collision /
  unset-arm encoding).
- Proto3 `enum` field including the zero value (catches default-elision
  rules — proto3 omits enum=0 from the wire).
- `map<K, V>` field (each entry is a synthesised message; ordering is
  *not* guaranteed across encoders — the audit would need either a single
  entry or a deterministic-order comparison).

Re-cut the Go side whenever cludden's schema or runtime moves; the version
pinned in `compat-tests/go/go.mod` is the source of truth for what was
audited.
