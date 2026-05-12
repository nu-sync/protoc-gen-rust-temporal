# Wire-format compatibility audit

This directory holds the cross-language harness referenced as **Phase 3** in
[`../SPEC.md`](../SPEC.md). Goal: verify that `protoc-gen-rust-temporal`'s
generated clients speak the same `(encoding, messageType, data)` Payload
triple as [cludden's `protoc-gen-go-temporal`](https://github.com/cludden/protoc-gen-go-temporal)
runtime. If the audit passes, a Go client written against cludden's plugin
can drive a Rust worker registered with ours (and vice versa) with no
adapter code.

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

The Rust side is fully runnable in CI today:

```bash
cargo run -p compat-tests-rust -- generate
# writes fixtures/*.rust.payload.json
```

The Go side requires the `nu-sync` GitHub org to exist (so consumers can
pull this repo into a Go module), but the script itself does not depend on
anything we publish:

```bash
cd compat-tests/go
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
- [ ] Rust generator binary (`compat-tests/rust/`).
- [ ] Go generator (`compat-tests/go/`).
- [ ] Fixture set covering: empty-input, scalar fields, nested messages,
      repeated fields, `google.protobuf.Empty` payload.
- [ ] CI job that runs both generators and diffs.

The Rust side will land first; the Go side runs against cludden's
upstream and can be re-cut whenever cludden's schema or runtime moves.
