# Phase 2 — `activities=true` emit plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or subagent-driven) to implement task-by-task.

> **Scope.** Phase 2 of the cludden-parity reframe per `docs/superpowers/specs/2026-05-12-cludden-parity-design.md`. Implements the **trait-only** emit shape recommended by `docs/superpowers/specs/2026-05-12-phase-2-spike-findings.md` (Option B). Adds the first plugin emit branch gated behind a new `activities=true` plugin option. Consumer wires the generated trait to `temporalio-sdk`'s `#[activity_definitions]` macro themselves — `~15 LOC per service`, documented in the bridge crate README.

**Goal:** Plugin emits a typed `<Service>Activities` async trait + per-activity name consts whenever `--rust-temporal_opt=activities=true` is passed AND the service has activity-annotated methods. Bridge crate gains an opt-in `worker` feature re-exporting `ActivityContext`/`Worker`/`ActivityError` from `temporalio-sdk 0.4` so consumers don't duplicate the dep. Example fixture exercises the new emit; CI builds it.

**Architecture:**
- `crates/protoc-gen-rust-temporal/src/options.rs` (new) parses `key=val,key=val` plugin options into a `RenderOptions` struct, strict on unknown keys.
- `main.rs` parses options from `CodeGeneratorRequest.parameter` once, passes through `run_with_pool`.
- `render.rs` branches on `options.activities` to emit the trait surface; existing client emit is unchanged.
- Bridge crate gains `worker = ["dep:temporalio-sdk"]` feature with re-exports.
- New fixture `activities_emit/` (annotated activity + `opt: activities=true`); existing fixtures stay opt-default (no behavior change).

**Tech stack:** Unchanged from Phase 1. `temporalio-sdk = "=0.4.0"` (exact-patch) added behind the bridge's `worker` feature.

---

## File structure

| File | Status | Responsibility |
|---|---|---|
| `crates/protoc-gen-rust-temporal/src/options.rs` | Create | `RenderOptions` struct + parser. Strict-mode parsing of `activities=true|false,...`. |
| `crates/protoc-gen-rust-temporal/src/lib.rs` | Modify | Add `pub mod options;`. Change `run_with_pool` to take `&RenderOptions`. |
| `crates/protoc-gen-rust-temporal/src/main.rs` | Modify | Decode `req.parameter`, parse via `options::parse`. |
| `crates/protoc-gen-rust-temporal/src/render.rs` | Modify | New `render_activities_trait(out, svc)` function, called when `options.activities && !svc.activities.is_empty()`. |
| `crates/protoc-gen-rust-temporal/src/validate.rs` | Modify | Warn-not-fail when `activities=true` with no activities; not a hard error since service authors may opt in for a service that doesn't have activities yet. |
| `crates/protoc-gen-rust-temporal/tests/parse_validate.rs` | Modify | Add `activities_emit` fixture coverage. |
| `crates/protoc-gen-rust-temporal/tests/protoc_invoke.rs` | Modify | Add a test passing `--rust-temporal_opt=activities=true` to verify the emit branch fires through the binary. |
| `crates/protoc-gen-rust-temporal/tests/fixtures/activities_emit/input.proto` | Create | Workflow + 2 activities + service-level task_queue. |
| `crates/protoc-gen-rust-temporal/tests/fixtures/activities_emit/expected.rs` | Create | Expected emit (golden). |
| `crates/protoc-gen-rust-temporal/tests/fixtures/activities_emit/options.txt` | Create | Single line `activities=true` — fixture-loader reads this to drive options for in-process render. |
| `crates/temporal-proto-runtime-bridge/Cargo.toml` | Modify | Add optional `temporalio-sdk` dep + `worker` feature. |
| `crates/temporal-proto-runtime-bridge/src/lib.rs` | Modify | New `#[cfg(feature = "worker")] pub mod worker { ... }` re-exporting the SDK worker primitives. |
| `crates/temporal-proto-runtime-bridge/README.md` | Modify | Document the `worker` feature + the adapter pattern snippet. |
| `examples/job-queue-integration/Cargo.toml` | Modify | Add `worker` feature that flips on the bridge's `worker` feature. |
| `examples/job-queue-integration/justfile` | Modify | `verify-bridge` step runs both feature combos. |
| `.github/workflows/ci.yml` | Modify | `verify-bridge` job runs the new feature combo. |
| `CHANGELOG.md` | Modify | `[Unreleased]` entry for Phase 2 emit + bridge worker feature. |
| `docs/RUNTIME-API.md` | Modify | New "Activities (Phase 2)" section documenting the trait emit shape. |

---

## Task 1: Plugin options parser

**Files:**
- Create: `crates/protoc-gen-rust-temporal/src/options.rs`
- Modify: `crates/protoc-gen-rust-temporal/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/protoc-gen-rust-temporal/tests/parse_validate.rs` (near the top, after existing imports):

```rust
#[test]
fn options_parse_defaults_to_disabled() {
    use protoc_gen_rust_temporal::options::{RenderOptions, parse_options};
    let opts: RenderOptions = parse_options("").unwrap();
    assert!(!opts.activities, "default-off");
}

#[test]
fn options_parse_activities_true() {
    use protoc_gen_rust_temporal::options::parse_options;
    let opts = parse_options("activities=true").unwrap();
    assert!(opts.activities);
}

#[test]
fn options_parse_rejects_unknown_keys() {
    use protoc_gen_rust_temporal::options::parse_options;
    let err = parse_options("activitie=true").unwrap_err();
    assert!(err.to_string().contains("activitie"), "{err}");
}

#[test]
fn options_parse_rejects_bad_bool() {
    use protoc_gen_rust_temporal::options::parse_options;
    let err = parse_options("activities=yes").unwrap_err();
    assert!(err.to_string().contains("activities"), "{err}");
}
```

Run:
```bash
cargo test -p protoc-gen-rust-temporal --test parse_validate options_parse
```

Expected: FAIL (`options` module doesn't exist).

- [ ] **Step 2: Implement `options.rs`**

Create `/Users/wcygan/Development/protoc-gen-rust-temporal/crates/protoc-gen-rust-temporal/src/options.rs`:

```rust
//! Plugin invocation options, parsed once from the protoc/buf-supplied
//! parameter string and threaded through `run_with_pool`.
//!
//! Strict by design: any unknown key is rejected so that typos like
//! `opt: [worker=true]` (missing `s`) fail loudly instead of silently
//! emitting nothing. See the cludden-parity reframe design doc for the
//! full surface; Phase 2 wires only `activities`.

use anyhow::{Result, anyhow};

#[derive(Debug, Default, Clone, Copy)]
pub struct RenderOptions {
    /// Emit the per-service `<Service>Activities` async trait + per-activity
    /// name consts when the service has activity-annotated methods.
    pub activities: bool,
}

/// Parse the protoc plugin parameter string.
///
/// Grammar: `key=val,key=val,...`. Whitespace trimmed around keys and values.
/// Empty input yields the default (all flags `false`).
pub fn parse_options(s: &str) -> Result<RenderOptions> {
    let mut out = RenderOptions::default();
    for pair in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| anyhow!("plugin option `{pair}` missing `=value`"))?;
        let key = key.trim();
        let value = value.trim();
        match key {
            "activities" => out.activities = parse_bool(key, value)?,
            other => return Err(anyhow!("unknown plugin option `{other}`")),
        }
    }
    Ok(out)
}

fn parse_bool(key: &str, value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(anyhow!("plugin option `{key}` expects `true|false`, got `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        let opts = parse_options("").unwrap();
        assert!(!opts.activities);
    }

    #[test]
    fn comma_separated_pairs() {
        let opts = parse_options("activities=true").unwrap();
        assert!(opts.activities);
    }

    #[test]
    fn whitespace_tolerated() {
        let opts = parse_options(" activities = true ").unwrap();
        assert!(opts.activities);
    }
}
```

Then in `crates/protoc-gen-rust-temporal/src/lib.rs`, add `pub mod options;` after the existing module declarations:

```rust
pub mod model;
pub mod options;
pub mod parse;
pub mod render;
pub mod validate;
```

- [ ] **Step 3: Run the tests**

```bash
cargo test -p protoc-gen-rust-temporal --test parse_validate options_parse
cargo test -p protoc-gen-rust-temporal --lib options::tests
```

Expected: all PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/protoc-gen-rust-temporal/src/options.rs crates/protoc-gen-rust-temporal/src/lib.rs crates/protoc-gen-rust-temporal/tests/parse_validate.rs
git commit -m "plugin: strict options parser (Phase 2 prep)"
```

---

## Task 2: Plumb options through `run_with_pool` + main.rs

**Files:**
- Modify: `crates/protoc-gen-rust-temporal/src/lib.rs`
- Modify: `crates/protoc-gen-rust-temporal/src/main.rs`

- [ ] **Step 1: Update `run_with_pool` signature**

Edit `/Users/wcygan/Development/protoc-gen-rust-temporal/crates/protoc-gen-rust-temporal/src/lib.rs`. Replace:

```rust
pub fn run_with_pool(
    pool: &DescriptorPool,
    files_to_generate: &HashSet<String>,
) -> Result<Vec<File>> {
    let services = parse::parse(pool, files_to_generate)?;
    let mut files = Vec::with_capacity(services.len());
    for service in &services {
        validate::validate(service)?;
        let content = render::render(service);
        let name = output_file_name(service);
        files.push(File {
            name: Some(name),
            insertion_point: None,
            content: Some(content),
            generated_code_info: None,
        });
    }
    Ok(files)
}
```

with:

```rust
pub fn run_with_pool(
    pool: &DescriptorPool,
    files_to_generate: &HashSet<String>,
    options: &options::RenderOptions,
) -> Result<Vec<File>> {
    let services = parse::parse(pool, files_to_generate)?;
    let mut files = Vec::with_capacity(services.len());
    for service in &services {
        validate::validate(service, options)?;
        let content = render::render(service, options);
        let name = output_file_name(service);
        files.push(File {
            name: Some(name),
            insertion_point: None,
            content: Some(content),
            generated_code_info: None,
        });
    }
    Ok(files)
}
```

- [ ] **Step 2: Update `main.rs` to parse options**

In `crates/protoc-gen-rust-temporal/src/main.rs`, find this section:

```rust
fn build_response(raw: &[u8]) -> Result<Vec<prost_types::compiler::code_generator_response::File>> {
    // Decode with prost-types just to get file_to_generate; extensions on
    // MethodOptions are dropped here, but we don't read them from this form.
    let req = CodeGeneratorRequest::decode(raw).context("decode CodeGeneratorRequest")?;
    let files_to_generate: HashSet<String> = req.file_to_generate.into_iter().collect();
```

Add the options parsing after the `files_to_generate` extraction:

```rust
fn build_response(raw: &[u8]) -> Result<Vec<prost_types::compiler::code_generator_response::File>> {
    let req = CodeGeneratorRequest::decode(raw).context("decode CodeGeneratorRequest")?;
    let files_to_generate: HashSet<String> = req.file_to_generate.into_iter().collect();
    let options = protoc_gen_rust_temporal::options::parse_options(req.parameter())
        .context("parse plugin options")?;
```

Then update the `run_with_pool` call site (further down in the same function):

```rust
    protoc_gen_rust_temporal::run_with_pool(&pool, &files_to_generate, &options).with_context(|| {
```

- [ ] **Step 3: Update `validate.rs` + `render.rs` signatures to accept options**

In `crates/protoc-gen-rust-temporal/src/validate.rs`, change the `validate` fn signature:

```rust
pub fn validate(svc: &ServiceModel, _options: &crate::options::RenderOptions) -> Result<()> {
    // existing body unchanged
```

(Phase 2 doesn't use the options in validate yet; Task 4 may add a warning. Underscore prefix avoids unused-variable lint.)

In `crates/protoc-gen-rust-temporal/src/render.rs`, change `render`:

```rust
pub fn render(svc: &ServiceModel, _options: &crate::options::RenderOptions) -> String {
    // existing body unchanged for now; Task 3 inserts the activities branch
```

- [ ] **Step 4: Fix the test harness to pass default options**

`tests/parse_validate.rs` and `tests/protoc_invoke.rs` may call `validate::validate(...)` or `render::render(...)` directly. Update those call sites to pass `&Default::default()`:

```bash
# Find the call sites
grep -rn "validate::validate\|render::render\b" /Users/wcygan/Development/protoc-gen-rust-temporal/crates/protoc-gen-rust-temporal/tests/
```

For each match, append `, &Default::default()` to the call. Example for `tests/parse_validate.rs:`

```rust
// Before:
let _ = render::render(svc);
// After:
let _ = render::render(svc, &Default::default());
```

- [ ] **Step 5: Run full test suite**

```bash
cargo test -p protoc-gen-rust-temporal
```

Expected: all PASS (no behavior change yet — options are threaded but not consulted).

- [ ] **Step 6: Commit**

```bash
git add crates/protoc-gen-rust-temporal/src/ crates/protoc-gen-rust-temporal/tests/
git commit -m "plugin: thread RenderOptions through pipeline"
```

---

## Task 3: Render the activities trait

**Files:**
- Modify: `crates/protoc-gen-rust-temporal/src/render.rs`
- Create: `crates/protoc-gen-rust-temporal/tests/fixtures/activities_emit/input.proto`
- Create: `crates/protoc-gen-rust-temporal/tests/fixtures/activities_emit/expected.rs`
- Create: `crates/protoc-gen-rust-temporal/tests/fixtures/activities_emit/options.txt`
- Modify: `crates/protoc-gen-rust-temporal/tests/parse_validate.rs`

- [ ] **Step 1: Create the fixture proto**

Create `/Users/wcygan/Development/protoc-gen-rust-temporal/crates/protoc-gen-rust-temporal/tests/fixtures/activities_emit/input.proto`:

```proto
syntax = "proto3";

package acts.v1;

import "google/protobuf/empty.proto";
import "temporal/v1/temporal.proto";

// Exercises the Phase 2 activities=true emit branch. Two activities
// with different input/output shapes (one Empty input) on a service
// that also has a workflow, to confirm activities + client emit
// coexist in the same generated module.
service ChunkService {
  rpc RunBatch(BatchInput) returns (BatchOutput) {
    option (temporal.v1.workflow) = {
      task_queue: "chunks"
      id: "batch-{{ .Name }}"
    };
  }

  rpc Process(ChunkInput) returns (ChunkOutput) {
    option (temporal.v1.activity) = {};
  }

  rpc Heartbeat(google.protobuf.Empty) returns (HeartbeatOutput) {
    option (temporal.v1.activity) = {};
  }
}

message BatchInput      { string name = 1; }
message BatchOutput     { uint32 chunks_done = 1; }
message ChunkInput      { bytes data = 1; }
message ChunkOutput     { uint64 hash = 1; }
message HeartbeatOutput { uint64 timestamp_ms = 1; }
```

- [ ] **Step 2: Add the options file**

Create `/Users/wcygan/Development/protoc-gen-rust-temporal/crates/protoc-gen-rust-temporal/tests/fixtures/activities_emit/options.txt`:

```
activities=true
```

The fixture loader (added in Step 5) reads this file. Existing fixtures without an `options.txt` use default options.

- [ ] **Step 3: Implement the activities trait emit in `render.rs`**

In `crates/protoc-gen-rust-temporal/src/render.rs`, change `render` to call a new branch:

```rust
pub fn render(svc: &ServiceModel, options: &crate::options::RenderOptions) -> String {
    let mut out = String::new();
    let mod_name = mod_name(svc);
    let proto_mod = proto_module_path(&svc.package);
    let client_struct = format!("{}Client", svc.service);

    let _ = writeln!(
        out,
        "// Code generated by protoc-gen-rust-temporal. DO NOT EDIT."
    );
    let _ = writeln!(out, "// source: {}", svc.source_file);
    let _ = writeln!(out);
    let _ = writeln!(out, "#[allow(clippy::all, unused_imports, dead_code)]");
    let _ = writeln!(out, "pub mod {mod_name} {{");
    let _ = writeln!(out, "    use anyhow::Result;");
    let _ = writeln!(out, "    use std::time::Duration;");
    let _ = writeln!(out, "    use crate::temporal_runtime;");
    let _ = writeln!(out, "    use {proto_mod}::*;");
    let _ = writeln!(out);

    render_message_type_impls(&mut out, svc);
    render_constants(&mut out, svc);
    render_id_fns(&mut out, svc);
    render_client_struct(&mut out, svc, &client_struct);
    for wf in &svc.workflows {
        render_start_options(&mut out, wf);
        render_handle(&mut out, svc, wf);
    }
    render_with_start_functions(&mut out, svc);

    // Phase 2: activities trait + name consts. Only emitted when both the
    // `activities=true` plugin option is set AND the service has at least
    // one activity-annotated method. Otherwise the trait would be empty
    // and the generated code would warn about an unused mod.
    if options.activities && !svc.activities.is_empty() {
        render_activities_trait(&mut out, svc);
    }

    let _ = writeln!(out, "}}");
    out
}
```

Then add the new function at the end of the file (before any test mod):

```rust
fn render_activities_trait(out: &mut String, svc: &ServiceModel) {
    use heck::ToShoutySnakeCase;

    let trait_name = format!("{}Activities", svc.service);

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "    // ── Activities ────────────────────────────────────────────"
    );
    let _ = writeln!(
        out,
        "    // Phase 2 (activities=true): typed trait + name consts. Wire to"
    );
    let _ = writeln!(
        out,
        "    // your worker via temporalio-sdk's #[activity_definitions] macro;"
    );
    let _ = writeln!(
        out,
        "    // see temporal-proto-runtime-bridge README for the adapter pattern."
    );
    let _ = writeln!(out);

    // Per-activity name consts.
    for act in &svc.activities {
        let const_ident = format!("{}_ACTIVITY_NAME", act.rpc_method.to_shouty_snake_case());
        let _ = writeln!(
            out,
            "    pub const {const_ident}: &'static str = \"{}\";",
            act.registered_name
        );
    }

    let _ = writeln!(out);

    // The trait. async fn in trait works on edition 2021+ with Rust 1.75+;
    // workspace MSRV is 1.88, so no async_trait needed.
    let _ = writeln!(out, "    pub trait {trait_name}: Send + Sync + 'static {{");
    for act in &svc.activities {
        let method_name = act.rpc_method.to_snake_case_method();
        let input_ty = if act.input_type.is_empty {
            "()".to_string()
        } else {
            act.input_type.rust_name().to_string()
        };
        let output_ty = if act.output_type.is_empty {
            "()".to_string()
        } else {
            act.output_type.rust_name().to_string()
        };
        let _ = writeln!(
            out,
            "        fn {method_name}(&self, ctx: temporal_runtime::ActivityContext, input: {input_ty}) -> impl ::std::future::Future<Output = Result<{output_ty}>> + Send;"
        );
    }
    let _ = writeln!(out, "    }}");
}

// Tiny helper since heck's ToSnakeCase needs an extra import below.
trait SnakeCaseMethodName {
    fn to_snake_case_method(&self) -> String;
}
impl SnakeCaseMethodName for String {
    fn to_snake_case_method(&self) -> String {
        use heck::ToSnakeCase;
        self.to_snake_case()
    }
}
impl SnakeCaseMethodName for str {
    fn to_snake_case_method(&self) -> String {
        use heck::ToSnakeCase;
        self.to_snake_case()
    }
}
```

The trait method uses `-> impl Future<Output = ...> + Send` rather than `async fn` because at the point of emit we want to keep the trait simple for the consumer's adapter to call. With `async fn` in trait, the user calling `JobServiceActivities::do_chunk(&*self, ctx, input).await` works the same — the choice between `async fn` and `impl Future` is mostly cosmetic. We pick `impl Future + Send` because it's explicit about the Send bound (which matters for spawning on a multi-threaded executor).

- [ ] **Step 4: Generate the golden `expected.rs`**

Don't hand-write it. The fixture-loader pattern in `parse_validate.rs` already supports `--bless` mode. Generate via:

```bash
cargo test -p protoc-gen-rust-temporal --test parse_validate activities_emit -- --nocapture 2>&1 | head -40
```

This will fail the first time. Then bless via:

```bash
PROTOC_GEN_RUST_TEMPORAL_BLESS=1 cargo test -p protoc-gen-rust-temporal --test parse_validate activities_emit
```

(If the bless mechanism doesn't exist for this fixture, generate the expected output manually by running the in-process pipeline against the fixture proto. See `regen_fixtures.sh` if present.)

Check the generated `expected.rs` matches expectations: it should contain `pub trait ChunkServiceActivities`, two name consts, and a `process(...)` + `heartbeat(...)` method.

- [ ] **Step 5: Update the fixture-loader to read `options.txt`**

In `tests/parse_validate.rs`, find the existing fixture-runner (the function that loads `input.proto` + `expected.rs`). Add a sibling that reads `options.txt` if present:

```rust
fn load_fixture_options(fixture_dir: &std::path::Path) -> protoc_gen_rust_temporal::options::RenderOptions {
    let p = fixture_dir.join("options.txt");
    if !p.exists() {
        return protoc_gen_rust_temporal::options::RenderOptions::default();
    }
    let s = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    protoc_gen_rust_temporal::options::parse_options(s.trim()).unwrap()
}
```

Wire it into the existing test function so the call becomes:

```rust
let opts = load_fixture_options(&fixture_dir);
let content = protoc_gen_rust_temporal::render::render(&svc, &opts);
```

- [ ] **Step 6: Add the `activities_emit` test case**

Append to `tests/parse_validate.rs`:

```rust
#[test]
fn activities_emit_renders_trait_with_activities_flag() {
    let fixture = fixture_dir("activities_emit");
    assert_render_matches(&fixture);
}
```

(Use whatever the existing helper function name is; mirror the existing fixture tests' shape.)

- [ ] **Step 7: Run + commit**

```bash
cargo test -p protoc-gen-rust-temporal
```

Expected: PASS, including the new test.

```bash
git add crates/protoc-gen-rust-temporal/src/render.rs crates/protoc-gen-rust-temporal/tests/
git commit -m "$(cat <<'EOF'
plugin: activities=true emit (trait + name consts)

Per Phase 2 spike findings: trait-only emit shape (Option B). Plugin
generates a per-service <Service>Activities async trait + per-activity
name consts when activities=true AND the service has activity-
annotated methods. Trait methods use impl Future<Output = Result<O>>
+ Send for explicit Send bounds (matters for multi-threaded executors).

New fixture activities_emit/ exercises a 2-activity service with one
Empty-input activity. options.txt sidecar drives the fixture's flags.
EOF
)"
```

---

## Task 4: Bridge crate `worker` feature

**Files:**
- Modify: `crates/temporal-proto-runtime-bridge/Cargo.toml`
- Modify: `crates/temporal-proto-runtime-bridge/src/lib.rs`
- Modify: `crates/temporal-proto-runtime-bridge/README.md`

- [ ] **Step 1: Add the `temporalio-sdk` workspace dep**

In root `Cargo.toml` workspace.dependencies:

```toml
temporalio-sdk = "=0.4.0"
```

- [ ] **Step 2: Add the optional dep + feature in bridge Cargo.toml**

In `crates/temporal-proto-runtime-bridge/Cargo.toml`, after the `[dependencies]` section:

```toml
[dependencies]
anyhow = { workspace = true }
prost = { workspace = true }
temporal-proto-runtime = { path = "../temporal-proto-runtime", version = "0.1", features = ["sdk"] }
temporalio-client = { workspace = true }
temporalio-common = { workspace = true }
temporalio-sdk = { workspace = true, optional = true }
url = { workspace = true }
uuid = { workspace = true }

[features]
default = []
# Worker-side primitives for consumers writing activity/workflow workers
# against the generated <Service>Activities / <Workflow> traits. Pulls in
# temporalio-sdk = "=0.4.0". The plugin's client-side emit does NOT depend
# on this feature.
worker = ["dep:temporalio-sdk"]

[dev-dependencies]
tokio = { workspace = true }
```

- [ ] **Step 3: Add the `worker` re-export module**

In `crates/temporal-proto-runtime-bridge/src/lib.rs`, append at the bottom (before `#[cfg(test)]`):

```rust
// ── Worker primitives (feature = "worker") ─────────────────────────────

/// Re-exports of the SDK worker primitives used by consumers wiring the
/// plugin-generated `<Service>Activities` trait to a Temporal worker.
///
/// **Stability:** these are direct re-exports of `temporalio-sdk 0.4` types.
/// When the SDK reshapes between minor versions, the bridge crate's minor
/// version bumps with it (per the design's SDK pinning rule). Consumer code
/// that touches these types may need adjustment at SDK upgrade time; the
/// plugin's emit is unaffected.
#[cfg(feature = "worker")]
pub mod worker {
    pub use temporalio_sdk::Worker;
    pub use temporalio_sdk::activities::{
        ActivityContext, ActivityDefinitions, ActivityError, ActivityImplementer,
    };
    pub use temporalio_common::ActivityDefinition;
}

// Top-level re-export so plugin-emitted code can resolve
// `crate::temporal_runtime::ActivityContext` without the consumer thinking
// about the worker submodule.
#[cfg(feature = "worker")]
pub use worker::ActivityContext;
```

- [ ] **Step 4: Document the adapter pattern in README.md**

Append to `crates/temporal-proto-runtime-bridge/README.md`:

```markdown
## Worker side (Phase 2+, opt-in)

The plugin's worker emit (activities, workflows) gives you a typed trait per
service. Wiring that trait to a Temporal `Worker` is currently a
~15-LOC consumer-owned adapter against `temporalio-sdk`'s
`#[activity_definitions]` macro. Enable the bridge crate's `worker`
feature to get the SDK types re-exported alongside the client surface:

```toml
[dependencies]
temporal-proto-runtime-bridge = { version = "0.1", features = ["worker"] }
```

Adapter pattern (for a service with `Process(ChunkInput) -> ChunkOutput`):

```rust,ignore
use std::sync::Arc;
use anyhow::Result;
use temporal_runtime::ActivityContext;
use temporal_runtime::worker::{ActivityDefinitions, Worker};

// 1. Impl the plugin-generated trait on your state struct.
pub struct MyImpl { /* shared deps here */ }

impl crate::generated::ChunkServiceActivities for MyImpl {
    fn process(
        &self,
        ctx: ActivityContext,
        input: ChunkInput,
    ) -> impl std::future::Future<Output = Result<ChunkOutput>> + Send {
        async move {
            // your activity body
            Ok(ChunkOutput { hash: 42 })
        }
    }
    // …one per activity in the trait…
}

// 2. Adapt via the SDK macro. This generates ActivityDefinition +
//    ExecutableActivity impls per method, tied to `MyImpl`.
#[temporalio_macros::activity_definitions]
impl MyImpl {
    #[activity(name = "acts.v1.ChunkService/Process")]
    async fn process_adapter(
        self: Arc<Self>,
        ctx: ActivityContext,
        input: ChunkInput,
    ) -> Result<ChunkOutput> {
        crate::generated::ChunkServiceActivities::process(&*self, ctx, input).await
    }
}

// 3. Register on the worker.
fn register(worker: &mut Worker, impl_: Arc<MyImpl>) {
    worker.register_activities(impl_);
}
```

Why the adapter exists: see `docs/superpowers/specs/2026-05-12-phase-2-spike-findings.md`.
The SDK's static-dispatch activity registration needs per-impl marker types
that the plugin can't generate at codegen time (the user's concrete type
isn't visible). The adapter is the documented 15-LOC bridge.
```

- [ ] **Step 5: Verify**

```bash
cargo check -p temporal-proto-runtime-bridge --features worker
cargo clippy -p temporal-proto-runtime-bridge --features worker --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/temporal-proto-runtime-bridge/
git commit -m "$(cat <<'EOF'
bridge: worker feature

Optional `worker` cargo feature pulls in temporalio-sdk = "=0.4.0" and
re-exports the SDK worker primitives consumers need to wire the plugin's
Phase 2 emit (Worker, ActivityContext, ActivityError, ActivityDefinitions,
ActivityDefinition, ActivityImplementer). Default builds remain SDK-
worker-free. Adapter pattern documented in the bridge README.
EOF
)"
```

---

## Task 5: Example crate worker feature + CI

**Files:**
- Modify: `examples/job-queue-integration/Cargo.toml`
- Modify: `examples/job-queue-integration/justfile`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Wire the bridge's `worker` feature in the example**

In `examples/job-queue-integration/Cargo.toml`, change the `[features]` block to:

```toml
[features]
default = []
bridge = ["dep:temporal-proto-runtime-bridge"]
worker = ["bridge", "temporal-proto-runtime-bridge/worker"]
```

- [ ] **Step 2: Extend the `verify-bridge` recipe**

In `examples/job-queue-integration/justfile`, replace the `verify-bridge` recipe with:

```just
# Build the example against the bridge crate, proving the plugin's generated
# emit + the bridge's facade impl are wire-compatible end-to-end. Phase 1
# exit criterion. Phase 2 also exercises the worker feature.
verify-bridge:
    cargo check -p job-queue-integration-example --features bridge
    cargo clippy -p job-queue-integration-example --features bridge --all-targets -- -D warnings
    cargo check -p job-queue-integration-example --features worker
    cargo clippy -p job-queue-integration-example --features worker --all-targets -- -D warnings
```

- [ ] **Step 3: Update CI to exercise worker feature**

In `.github/workflows/ci.yml`, replace the `verify-bridge` job's last step:

```yaml
      - name: Verify example against bridge
        run: |
          cargo check -p job-queue-integration-example --features bridge
          cargo clippy -p job-queue-integration-example --features bridge --all-targets -- -D warnings
          cargo check -p job-queue-integration-example --features worker
          cargo clippy -p job-queue-integration-example --features worker --all-targets -- -D warnings
```

- [ ] **Step 4: Run verify-bridge locally**

```bash
cd /Users/wcygan/Development/protoc-gen-rust-temporal/examples/job-queue-integration && just verify-bridge
```

Expected: all four cargo check/clippy invocations PASS.

- [ ] **Step 5: Commit**

```bash
git add examples/job-queue-integration/ .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
example/ci: exercise bridge worker feature

verify-bridge now runs cargo check + clippy with both `bridge` and
`worker` feature combos. CI mirrors the change. The example doesn't
have activity-annotated rpcs yet, so the worker feature only checks
that the bridge crate re-exports compile through the example's dep
tree — full activities emit coverage lives in the plugin's
`activities_emit` fixture (Task 3).
EOF
)"
```

---

## Task 6: Docs

**Files:**
- Modify: `docs/RUNTIME-API.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Add Phase 2 section to RUNTIME-API.md**

Append to `docs/RUNTIME-API.md`:

```markdown
## Phase 2 — Activities (opt-in via `activities=true`)

When invoked with `--rust-temporal_opt=activities=true`, the plugin emits, per
service with activity-annotated methods:

| Symbol | Shape |
|---|---|
| `<METHOD>_ACTIVITY_NAME` | `pub const &'static str` per annotated activity. Value matches what the activity is registered under server-side. |
| `<Service>Activities` | `pub trait <Service>Activities: Send + Sync + 'static` with one method per activity. Signature: `fn <method>(&self, ctx: temporal_runtime::ActivityContext, input: <Input>) -> impl Future<Output = Result<<Output>>> + Send`. |

The trait method takes `&self`. The consumer's adapter (which wires the trait
to a `temporalio-sdk` Worker via `#[activity_definitions]`) does the
`Arc<Self>` dance — see the bridge crate README for the pattern.

Required runtime symbols (only when `activities=true` is set):
- `temporal_runtime::ActivityContext` — opaque per-call context. Surfaces
  cancellation, heartbeat, and identity info to the activity body. Re-
  exported from `temporal-proto-runtime-bridge`'s `worker` feature; consumers
  using a custom facade must provide it.

The plugin does NOT emit a `register_<service>_activities(...)` function in
Phase 2 — the consumer-side adapter pattern handles registration. This is the
trait-only emit per the spike findings.
```

- [ ] **Step 2: CHANGELOG entry**

In the `[Unreleased]` block of `CHANGELOG.md`, append under the existing `### Added`:

```markdown
- **Phase 2 emit — activities trait** (opt-in via `--rust-temporal_opt=activities=true`).
  Plugin generates a per-service `<Service>Activities` async trait + per-activity
  name consts when the service has methods annotated with
  `option (temporal.v1.activity) = {}`. Trait method signature uses
  `impl Future<Output = Result<O>> + Send` (Rust 2024 / MSRV-1.88).
- **`temporal-proto-runtime-bridge` `worker` feature** — opt-in re-export of
  `temporalio-sdk 0.4`'s worker primitives (`Worker`, `ActivityContext`,
  `ActivityError`, `ActivityDefinitions`, `ActivityDefinition`,
  `ActivityImplementer`). Default builds remain SDK-worker-free; consumers
  opt in when wiring the plugin's worker emit.
- **Strict plugin options parser.** Unknown keys in `--rust-temporal_opt=...`
  return a `CodeGeneratorResponse.error` rather than silently emitting
  nothing — avoids the `worker=true` (missing `s`) trap.
- **Phase 2 spike findings** documented at
  `docs/superpowers/specs/2026-05-12-phase-2-spike-findings.md`.

### Notes
- The `<Service>Activities` trait is **not dyn-compatible** (uses async-fn-in-
  trait without box-future) — consumers should impl it on their concrete
  state struct, not store `Box<dyn Trait>`. The 15-LOC adapter pattern in
  the bridge crate README assumes a concrete impl.
- Consumers writing workers must bring `temporalio-sdk` + the SDK's
  `#[activity_definitions]` macro themselves (the plugin doesn't emit the
  registration glue — see spike findings Option B). The bridge crate's
  `worker` feature gives you the right SDK types; you supply the macro
  invocation.
```

- [ ] **Step 3: Final full-workspace verification**

```bash
cd /Users/wcygan/Development/protoc-gen-rust-temporal && \
  cargo fmt --all -- --check && \
  cargo clippy --workspace --all-targets -- -D warnings && \
  cargo test --workspace --all-targets && \
  (cd examples/job-queue-integration && just verify-bridge)
```

Expected: all four PASS.

- [ ] **Step 4: Commit**

```bash
git add docs/RUNTIME-API.md CHANGELOG.md
git commit -m "docs: Phase 2 emit (activities trait + bridge worker feature)"
```

---

## Self-review checklist

- [x] **Spec coverage** (against the design's Phase 2 row):
  - "Per `option (temporal.v1.activity) = {}` on a method, emit ... typed trait" → Task 3.
  - "register_<service>_activities" → **NOT emitted** per the spike's Option B (documented in CHANGELOG + RUNTIME-API).
  - "execute_activity_proto" (workflow-side activity execution) → **deferred to Phase 3** (workflows). The plugin doesn't emit workflow-side activity calls in Phase 2.
  - "Facade grows: register_activity_proto, execute_activity_proto" → **NOT added** to the bridge crate's facade surface in Phase 2 per spike. The bridge gains a `worker` feature with SDK re-exports instead. CHANGELOG calls this out explicitly.
  - PoC migration → **out of tree** (sibling repo). Tracked as Phase 2 follow-up, not in this plan.
- [x] **Placeholder scan.** Code blocks contain real code. The `--bless` mechanism for the golden in Task 3 is hand-wavy — if the fixture-loader doesn't already support bless mode, generate `expected.rs` once by running the in-process pipeline and committing the output. Note this caveat inline.
- [x] **Type consistency.** `RenderOptions` constructed by `parse_options(...)` is consumed by `render::render(svc, options)` and `validate::validate(svc, options)`. Activity name consts are emitted via `to_shouty_snake_case` on `act.rpc_method`. Trait method names use `to_snake_case` on `act.rpc_method`. Same casing scheme across all references.

---

## Deferred / out of scope (Phase 3+)

- `execute_activity_proto` — workflow-side typed activity execution. Requires `WorkflowContext` re-exports from the bridge, which doesn't make sense to add until workflow emit (Phase 3) lands.
- `register_<service>_activities` macro emit (the spike's Option A) — additive, lands as Phase 2.1 if the Option B adapter ergonomics prove painful in practice.
- PoC migration of `~/Development/job-queue` — sibling-repo work, performed after the bridge ships.
