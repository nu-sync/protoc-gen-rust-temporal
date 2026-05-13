# R7 — Bloblang-Backed Templates: design note

**Status:** Pre-implementation. Captures the contract the plugin
needs to honour and the minimum viable evaluator shape. Update when
implementation begins.

**Last updated:** 2026-05-13.

## What R7 covers

`cludden/protoc-gen-go-temporal` uses [Bloblang](https://docs.redpanda.com/redpanda-connect/guides/bloblang/about/)
as its mapping language for four annotation fields:

| Field | Where it lives | What it produces |
|---|---|---|
| `WorkflowOptions.id` | per-workflow | The workflow id string. The Rust plugin already supports the simple `{{ .Field }}` Go-template subset for this — Bloblang would replace that with a richer expression. |
| `UpdateOptions.id` | per-update | The *parent workflow* id the update should target. Same status as workflow id. |
| `WorkflowOptions.search_attributes` | per-workflow | A `map[string]any` of search attributes. Currently rejected at parse. |
| `WorkflowOptions.typed_search_attributes` | per-workflow | A typed search-attribute mapping. Currently rejected at parse. |

The workflow / update id paths already work end-to-end for users who
write the simple `{{ .Field }}` template. R7 is the foundation that
lifts them onto a richer mapping language and unlocks search
attributes (the two `*_search_attributes` fields the support-status
table currently lists as rejected).

## SDK contract

Search attributes flow through the bridge as:

```rust
// crates/temporal-proto-runtime-bridge/src/lib.rs (today)
pub use temporalio_sdk::WorkflowStartOptions;
// WorkflowStartOptions::search_attributes: Option<HashMap<String, Payload>>
```

The Rust SDK expects an already-built `HashMap<String, Payload>` —
it does **not** accept a Bloblang expression. So either:

1. **Compile-time evaluator** — the plugin parses the Bloblang
   expression at codegen and emits literal `HashMap` construction
   code. Works for the static subset (literal keys + values, no
   field references on input). The current `parse_id_template`
   uses this strategy for `{{ .Field }}` templates.

2. **Runtime evaluator** — emit a closure that takes the workflow
   input and returns the `HashMap`, then call it from the start
   path. Required for expressions that reference input fields
   (e.g. `root.CustomerId = this.customer_id`). Pulls a Bloblang
   crate into the bridge.

For R7 first-slice, **prefer compile-time** for the literal subset
and explicitly reject expressions that need runtime evaluation
with a "needs runtime Bloblang; not yet supported" diagnostic.

## Minimum viable subset (proposed)

Ship in three slices, each independently shippable + testable:

### Slice 1 — literal map (compile-time only)

Accept Bloblang expressions of the form:

```
root = { "Key1": <literal>, "Key2": <literal>, ... }
```

where `<literal>` is one of:

- String: `"foo"`
- Integer: `42`
- Boolean: `true` / `false`
- Empty map: `{}`

Compile to:

```rust
{
    let mut sa = ::std::collections::HashMap::new();
    sa.insert("Key1".to_string(), <Payload for value>);
    sa.insert("Key2".to_string(), <Payload for value>);
    sa
}
```

The `<Payload for value>` construction needs a helper in
`temporal-proto-runtime` that wraps a Rust literal in the
`(binary/protobuf, …)` triple. Reuses the existing wire-format
plumbing.

**Value of this slice on its own:** unblocks any service that
declares static search attributes (e.g. `"Environment":
"production"` as a tagging convention). Doesn't help dynamic
per-workflow tagging — that's slice 2.

### Slice 2 — field references via `this.<field>`

Accept:

```
root = { "Key1": this.customer_id, "Key2": this.region }
```

where `this.<field>` resolves to the workflow input's field. Compile
to a per-workflow function:

```rust
fn run_search_attributes(input: &OrderInput) -> HashMap<String, Payload> {
    let mut sa = HashMap::new();
    sa.insert("Key1".to_string(), <Payload from input.customer_id>);
    sa.insert("Key2".to_string(), <Payload from input.region>);
    sa
}
```

Same compile-time strategy as `parse_id_template`. The codegen
materialises field references against the proto input descriptor at
parse time — same field-name resolution (snake_case via `heck`) and
same "field doesn't exist" diagnostic.

### Slice 3 — typed search attributes

`WorkflowOptions.typed_search_attributes` declares the *type* of
each attribute, not just values. Per cludden's docs:

```
root.MyKeyword = { "type": "keyword" }
root.MyDouble = { "type": "double" }
```

The Temporal server validates the typed map against the registered
search-attribute schema. This is largely orthogonal to slice 1 and
slice 2 — it's a separate compile path that emits the
`SearchAttributeKey<T>` types from `temporalio-common`.

Slice 3 can land after slices 1 and 2 are validated against real
fixtures.

### Beyond slice 3 — defer

The full Bloblang language has conditionals, functions, pipelines,
regex, etc. Beyond slice 3 needs a real Bloblang evaluator (either
embed [bento](https://github.com/warpstreamlabs/bento)'s evaluator
or write one). That's a multi-week investment with a small audience
— cludden's published examples don't use the advanced surface.

## Test strategy per slice

Each slice should ship with:

1. Fixture under
   `crates/protoc-gen-rust-temporal/tests/fixtures/<slice_name>/`
   declaring representative Bloblang expressions.
2. Parser unit tests that pin the supported expression shapes.
3. Render golden tests that pin the emitted `HashMap` /
   derivation-fn code.
4. A negative test asserting the "expression too complex; needs
   slice N+1" diagnostic fires for expressions that overshoot the
   slice.
5. Stub-runtime additions to `generated_surface_compile.rs` so the
   emitted code type-checks against the SDK.

## What this design note unblocks

- A future contributor can scope an R7 PR to one slice without
  reading through the entire Bloblang language spec.
- The "Bloblang" row in `docs/SUPPORT-STATUS.md` can be split into
  separate rows for each slice as they land.
- The plugin can ship slice-1-supported, slice-2-supported, etc.
  diagnostics without redesign — the parser starts strict and
  loosens as slices land.

## Open questions

- Should the Bloblang evaluator live in `temporal-proto-runtime`
  (so consumer crates that don't enable the `sdk` feature still
  have it) or in `temporal-proto-runtime-bridge` (gated behind the
  SDK)? Slice 1 is pure compile-time so it doesn't matter, but
  slices 2+ ship runtime code and the answer affects the dep graph.
- Whether to mirror cludden's exact Bloblang flavor or define a
  Rust-specific subset documented as a Bloblang *compatible* but
  not identical mapping language.
- How to surface "expression overshoots the supported subset"
  diagnostics with line/column positions in the proto annotation
  string.

These can resolve during slice 1 implementation.
