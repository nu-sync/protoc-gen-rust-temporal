# Phase 1 — Bridge crate implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Scope note.** The design at `docs/superpowers/specs/2026-05-12-cludden-parity-design.md` covers six phases. This plan only implements **Phase 1 (Bridge crate)**. Phases 2–6 (activities, workflows, CLI, Nexus/XNS, codec/docs) each become separate plans whose entry criteria depend on Phase 1 shipping first. Per the design's exit criterion, Phase 1 succeeds when the in-tree example compiles end-to-end against the new bridge crate (no `todo!()` panics if bodies execute under a real client). The out-of-tree PoC migration (`/Users/wcygan/Development/job-queue`) is a follow-up — not in scope here.

**Goal:** Ship `temporal-proto-runtime-bridge 0.1.0`, a concrete implementation of every function documented in `docs/RUNTIME-API.md`, backed by `temporalio-client 0.4`. Consumers swap one line — `pub use temporal_proto_runtime_bridge as temporal_runtime;` — and the plugin's generated code runs against the real SDK.

**Architecture:** New workspace member `crates/temporal-proto-runtime-bridge/`. Single `src/lib.rs` exposing free functions + a `TemporalClient` newtype around `Arc<temporalio_client::Client>` and a `WorkflowHandle` that stores client + workflow id (and re-derives an `UntypedWorkflowHandle` per call, matching the PoC's verified pattern). Internally maps the bridge's `WorkflowIdReusePolicy` / `WaitPolicy` to the SDK's prost enums; never re-exports SDK types in the public API. The in-tree `examples/job-queue-integration` keeps its `todo!()` stub by default, and a new `bridge` cargo feature replaces it with `pub use temporal_proto_runtime_bridge as temporal_runtime;` for the `just verify-bridge` recipe.

**Tech stack:** Rust 2024 edition, MSRV 1.88, `temporalio-client = "=0.4.0"` (exact-patch pin per design), `temporalio-common = "=0.4.0"`, `prost 0.13`, `anyhow 1` (matches the facade's `Result<T>` shape), `uuid 1` (for `random_workflow_id`), `url 2` (for `connect` helper).

---

## File structure

| File | Status | Responsibility |
|---|---|---|
| `Cargo.toml` (workspace root) | Modify | Add `temporal-proto-runtime-bridge` workspace member; declare `temporalio-client`, `temporalio-common`, `uuid`, `url`, `tokio` as workspace dependencies. |
| `crates/temporal-proto-runtime-bridge/Cargo.toml` | Create | New crate manifest. Pins `temporalio-client = "=0.4.0"` (exact patch). |
| `crates/temporal-proto-runtime-bridge/README.md` | Create | Crate-level docs: one-line integration example, SDK pinning rationale, override pattern. |
| `crates/temporal-proto-runtime-bridge/src/lib.rs` | Create | The whole crate surface: types, payload helpers, 14 facade functions, doc examples, deterministic unit tests. |
| `examples/job-queue-integration/Cargo.toml` | Modify | Add optional `temporal-proto-runtime-bridge` dep + `bridge` feature. |
| `examples/job-queue-integration/src/lib.rs` | Modify | Cfg-gate the stub `pub mod temporal_runtime;` against the `bridge` feature. |
| `examples/job-queue-integration/justfile` | Modify | Add `verify-bridge` recipe. |
| `.github/workflows/ci.yml` | Modify | Add a `verify-bridge` job step (cargo check on the example with `--features bridge`). |
| `docs/RUNTIME-API.md` | Modify | Add a paragraph at the top pointing to the new bridge crate as the default impl; keep all 0.1.0 signatures unchanged. |
| `CHANGELOG.md` | Modify | Add an `[Unreleased]` entry for the new crate. |
| `README.md` (repo root) | Modify | Mention the bridge crate in the consumer on-ramp. |
| `SPEC.md` (design header) | Modify | Pin the cludden commit (currently `*(TBD)*` per the design's open follow-ups). |

Single-file `src/lib.rs` is the deliberate choice for Phase 1 — matches the PoC's 239-line monolithic adapter shape and stays small enough to hold in working memory. The file will split naturally when worker emit lands in Phase 2/3 (each phase's facade additions form a clear seam).

---

## Task 1: Bootstrap bridge crate skeleton

**Files:**
- Create: `crates/temporal-proto-runtime-bridge/Cargo.toml`
- Create: `crates/temporal-proto-runtime-bridge/src/lib.rs`
- Create: `crates/temporal-proto-runtime-bridge/README.md`
- Modify: `Cargo.toml` (workspace root) — add member + workspace deps

- [ ] **Step 1: Update root `Cargo.toml`**

Edit `/Users/wcygan/Development/protoc-gen-rust-temporal/Cargo.toml`. Add the new member and workspace dependencies. The new dep entries pin to the same versions the PoC validated against.

```toml
[workspace]
resolver = "2"
members = [
    "crates/protoc-gen-rust-temporal",
    "crates/temporal-proto-runtime",
    "crates/temporal-proto-runtime-bridge",
    "compat-tests/rust",
    "examples/job-queue-integration",
]

[workspace.package]
edition = "2024"
rust-version = "1.88"
license = "MIT"
repository = "https://github.com/nu-sync/protoc-gen-rust-temporal"
homepage = "https://github.com/nu-sync/protoc-gen-rust-temporal"
authors = ["nu-sync contributors"]

[workspace.dependencies]
anyhow = "1"
prost = "0.13"
prost-types = "0.13"
prost-reflect = "0.14"
prost-build = "0.13"
heck = "0.5"
# Phase 1 bridge crate dependencies. Exact-patch pin on the SDK is intentional
# (design §"SDK pinning"): when temporalio-sdk 0.5 ships, we cut a new bridge
# crate minor version; plugin emit stays unchanged.
temporalio-client = "=0.4.0"
temporalio-common = "=0.4.0"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
url = "2"
uuid = { version = "1", features = ["v4"] }
```

- [ ] **Step 2: Create the bridge crate `Cargo.toml`**

Create `/Users/wcygan/Development/protoc-gen-rust-temporal/crates/temporal-proto-runtime-bridge/Cargo.toml`:

```toml
[package]
name = "temporal-proto-runtime-bridge"
version = "0.1.0"
description = "Default bridge between protoc-gen-rust-temporal generated clients and temporalio-client 0.4. Drop-in `temporal_runtime` facade."
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
readme = "README.md"
keywords = ["temporal", "protobuf", "workflow", "client"]
categories = ["asynchronous"]

[dependencies]
anyhow = { workspace = true }
prost = { workspace = true }
temporal-proto-runtime = { path = "../temporal-proto-runtime", version = "0.1", features = ["sdk"] }
temporalio-client = { workspace = true }
temporalio-common = { workspace = true }
url = { workspace = true }
uuid = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
```

- [ ] **Step 3: Create the placeholder `src/lib.rs`**

Create `/Users/wcygan/Development/protoc-gen-rust-temporal/crates/temporal-proto-runtime-bridge/src/lib.rs` with the crate-level doc comment and an empty body (real code lands in subsequent tasks):

```rust
//! Default implementation of the `crate::temporal_runtime` facade that
//! `protoc-gen-rust-temporal` emits calls against. Backed by
//! `temporalio-client = "=0.4"` (exact-patch pinned — bridge crate minor
//! versions track SDK reshapes; plugin emit is unaffected).
//!
//! # Usage
//!
//! Add the dep and re-export from your crate's `lib.rs`:
//!
//! ```toml
//! [dependencies]
//! temporal-proto-runtime-bridge = "0.1"
//! ```
//!
//! ```ignore
//! pub use temporal_proto_runtime_bridge as temporal_runtime;
//! ```
//!
//! That's the whole wiring. The hand-written `temporal_runtime.rs` becomes
//! optional — only consumers who stub for tests or pin a vendored SDK keep
//! their own.
//!
//! See `docs/RUNTIME-API.md` for the contract this crate implements.

// Subsequent tasks fill this module in.
```

- [ ] **Step 4: Create a minimal `README.md`**

Create `/Users/wcygan/Development/protoc-gen-rust-temporal/crates/temporal-proto-runtime-bridge/README.md`:

```markdown
# temporal-proto-runtime-bridge

Default bridge crate for [`protoc-gen-rust-temporal`]-generated clients. Implements
every symbol in [`docs/RUNTIME-API.md`] against `temporalio-client 0.4`.

## Wiring

```toml
[dependencies]
temporal-proto-runtime-bridge = "0.1"
```

```rust,ignore
// In your crate's lib.rs:
pub use temporal_proto_runtime_bridge as temporal_runtime;
```

That single re-export satisfies every `crate::temporal_runtime::*` reference the
plugin emits.

## SDK pinning

This crate pins `temporalio-client = "=0.4.0"` (exact patch). When the SDK ships
a breaking 0.5, we cut `temporal-proto-runtime-bridge 0.2`; the plugin's emit
shape does not change, so consumers bump the bridge crate version and recompile.

## Override

Drop the `pub use` line and write your own `mod temporal_runtime;` against the
facade — for tests, vendored SDKs, or custom transport. The
[`examples/job-queue-integration`] crate ships a `todo!()`-bodied stub that's
the canonical override reference.

[`protoc-gen-rust-temporal`]: https://github.com/nu-sync/protoc-gen-rust-temporal
[`docs/RUNTIME-API.md`]: ../../docs/RUNTIME-API.md
[`examples/job-queue-integration`]: ../../examples/job-queue-integration
```

- [ ] **Step 5: Verify the skeleton compiles**

Run from the repo root:

```bash
cargo check -p temporal-proto-runtime-bridge
```

Expected: PASS (no warnings; empty crate compiles cleanly with deps resolved).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/temporal-proto-runtime-bridge/
git commit -m "$(cat <<'EOF'
bridge: scaffold temporal-proto-runtime-bridge crate

Workspace member with empty lib.rs and the dep set Phase 1 needs:
temporalio-client/common (exact-patch pinned), prost, anyhow, uuid,
url. Subsequent commits land the 14 facade functions plus example
crate wiring.
EOF
)"
```

---

## Task 2: Types + payload helpers

**Files:**
- Modify: `crates/temporal-proto-runtime-bridge/src/lib.rs`

The bridge crate defines its own enums (`WorkflowIdReusePolicy`, `WaitPolicy`) instead of re-exporting SDK ones — this is the load-bearing rule from the design: "Bridge crate ... re-exports nothing SDK-typed in its public API." Internal `impl From` blocks map to the SDK's prost enums when calling into the SDK.

- [ ] **Step 1: Write the failing tests for types + helpers**

Add to `crates/temporal-proto-runtime-bridge/src/lib.rs`, replacing the placeholder body. Start with the test module so the test-driven loop is real (these tests cover encode/decode round-trips and enum mapping — the SDK-call sites are exercised via `cargo check` on the example crate in Task 7).

```rust
//! (crate-level doc from Task 1 — leave unchanged)

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use temporalio_client::{
    Client, UntypedQuery, UntypedSignal, UntypedUpdate, UntypedWorkflowHandle,
    WorkflowExecuteUpdateOptions, WorkflowGetResultOptions, WorkflowQueryOptions,
    WorkflowSignalOptions, WorkflowStartOptions, WorkflowStartSignal,
};
use temporalio_common::UntypedWorkflow;
use temporalio_common::data_converters::RawValue;
use temporalio_common::protos::temporal::api::common::v1::Payload;
use temporalio_common::protos::temporal::api::enums::v1 as sdk_enums;

pub use temporal_proto_runtime::TemporalProtoMessage;

/// Encoding constant for the wire-format triple (`metadata.encoding`).
const ENCODING: &str = temporal_proto_runtime::ENCODING;

/// Opaque handle on the Temporal client connection. Cheaply cloneable via
/// `Arc`. Constructed by [`connect`] or directly by the consumer.
#[derive(Clone)]
pub struct TemporalClient {
    inner: Arc<Client>,
}

impl TemporalClient {
    /// Wrap an existing `temporalio_client::Client` (already-constructed by
    /// the consumer, e.g. via custom transport).
    pub fn from_client(client: Client) -> Self {
        Self { inner: Arc::new(client) }
    }

    /// Wrap a shared `Arc<Client>` without re-wrapping.
    pub fn from_arc(client: Arc<Client>) -> Self {
        Self { inner: client }
    }

    /// Borrow the underlying SDK client. Escape hatch for power users who
    /// need SDK-typed access (e.g. for features the facade hasn't grown yet).
    pub fn sdk(&self) -> &Client {
        &self.inner
    }
}

/// Live workflow handle. Stores the workflow id (and run id if known) so we
/// can re-derive an `UntypedWorkflowHandle` per call without lifetime tying.
pub struct WorkflowHandle {
    client: TemporalClient,
    workflow_id: String,
    run_id: Option<String>,
}

impl WorkflowHandle {
    /// The workflow id. Always populated.
    pub fn workflow_id(&self) -> &str {
        &self.workflow_id
    }

    /// The run id, if known. Populated by `start_workflow_proto*`;
    /// `attach_handle` leaves it `None` (consumer didn't supply one).
    pub fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }

    fn untyped(&self) -> UntypedWorkflowHandle<Client> {
        self.client.inner.get_workflow_handle::<UntypedWorkflow>(&self.workflow_id)
    }
}

/// Mirror of cludden's `IDReusePolicy`. Variants match the proto enum modulo
/// the unspecified default (we model that as `Option::None` at call sites).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowIdReusePolicy {
    AllowDuplicate,
    AllowDuplicateFailedOnly,
    RejectDuplicate,
    TerminateIfRunning,
}

impl From<WorkflowIdReusePolicy> for sdk_enums::WorkflowIdReusePolicy {
    fn from(value: WorkflowIdReusePolicy) -> Self {
        match value {
            WorkflowIdReusePolicy::AllowDuplicate => Self::AllowDuplicate,
            WorkflowIdReusePolicy::AllowDuplicateFailedOnly => Self::AllowDuplicateFailedOnly,
            WorkflowIdReusePolicy::RejectDuplicate => Self::RejectDuplicate,
            WorkflowIdReusePolicy::TerminateIfRunning => Self::TerminateIfRunning,
        }
    }
}

/// Update stage to wait for before the update call returns. The Rust facade
/// always returns the update's output, so the call site still blocks on
/// completion; `WaitPolicy` controls the *stage acknowledgement* level the
/// server reports back at, not whether `get_result` is awaited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitPolicy {
    Admitted,
    Accepted,
    Completed,
}

impl From<WaitPolicy> for sdk_enums::UpdateWorkflowExecutionLifecycleStage {
    fn from(value: WaitPolicy) -> Self {
        match value {
            WaitPolicy::Admitted => Self::Admitted,
            WaitPolicy::Accepted => Self::Accepted,
            WaitPolicy::Completed => Self::Completed,
        }
    }
}

/// Convenience: build a single `binary/protobuf` payload from a prost message.
fn encode_proto_payload<T: TemporalProtoMessage>(msg: &T) -> Payload {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("encoding".to_string(), ENCODING.as_bytes().to_vec());
    metadata.insert(
        "messageType".to_string(),
        T::MESSAGE_TYPE.as_bytes().to_vec(),
    );
    Payload {
        metadata,
        data: prost::Message::encode_to_vec(msg),
        external_payloads: vec![],
    }
}

/// Convenience: decode a single `binary/protobuf` payload back into a prost
/// message. Metadata mismatch is *not* checked here — the wire-format invariant
/// is asserted by `temporal-proto-runtime`'s `TemporalDeserializable` impl;
/// this helper is only reached after the SDK has already validated metadata.
fn decode_proto_payload<T: TemporalProtoMessage>(payload: &Payload) -> Result<T, prost::DecodeError> {
    T::decode(payload.data.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Eq, prost::Message)]
    struct Sample {
        #[prost(string, tag = "1")]
        name: String,
    }

    impl TemporalProtoMessage for Sample {
        const MESSAGE_TYPE: &'static str = "test.v1.Sample";
    }

    #[test]
    fn encode_decode_round_trip() {
        let original = Sample { name: "hello".into() };
        let payload = encode_proto_payload(&original);
        assert_eq!(
            payload.metadata.get("encoding").map(Vec::as_slice),
            Some(b"binary/protobuf".as_slice()),
        );
        assert_eq!(
            payload.metadata.get("messageType").map(Vec::as_slice),
            Some(b"test.v1.Sample".as_slice()),
        );
        let decoded: Sample = decode_proto_payload(&payload).expect("decode");
        assert_eq!(decoded, original);
    }

    #[test]
    fn workflow_id_reuse_policy_maps_to_sdk_variants() {
        assert_eq!(
            sdk_enums::WorkflowIdReusePolicy::from(WorkflowIdReusePolicy::AllowDuplicate),
            sdk_enums::WorkflowIdReusePolicy::AllowDuplicate,
        );
        assert_eq!(
            sdk_enums::WorkflowIdReusePolicy::from(WorkflowIdReusePolicy::AllowDuplicateFailedOnly),
            sdk_enums::WorkflowIdReusePolicy::AllowDuplicateFailedOnly,
        );
        assert_eq!(
            sdk_enums::WorkflowIdReusePolicy::from(WorkflowIdReusePolicy::RejectDuplicate),
            sdk_enums::WorkflowIdReusePolicy::RejectDuplicate,
        );
        assert_eq!(
            sdk_enums::WorkflowIdReusePolicy::from(WorkflowIdReusePolicy::TerminateIfRunning),
            sdk_enums::WorkflowIdReusePolicy::TerminateIfRunning,
        );
    }

    #[test]
    fn wait_policy_maps_to_sdk_lifecycle_stages() {
        use sdk_enums::UpdateWorkflowExecutionLifecycleStage as Stage;
        assert_eq!(Stage::from(WaitPolicy::Admitted), Stage::Admitted);
        assert_eq!(Stage::from(WaitPolicy::Accepted), Stage::Accepted);
        assert_eq!(Stage::from(WaitPolicy::Completed), Stage::Completed);
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

```bash
cargo test -p temporal-proto-runtime-bridge
```

Expected: 3 tests pass (`encode_decode_round_trip`, `workflow_id_reuse_policy_maps_to_sdk_variants`, `wait_policy_maps_to_sdk_lifecycle_stages`). The SDK-call functions aren't implemented yet, so no other tests exist.

- [ ] **Step 3: Verify clippy is clean**

```bash
cargo clippy -p temporal-proto-runtime-bridge --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/temporal-proto-runtime-bridge/src/lib.rs
git commit -m "$(cat <<'EOF'
bridge: types + payload helpers

TemporalClient (Arc<Client> wrapper), WorkflowHandle (stores id+run_id,
re-derives the untyped handle per call), and the bridge-owned
WorkflowIdReusePolicy / WaitPolicy enums with From impls to the SDK
prost variants. Per the design, no SDK type leaks into the bridge's
public API. encode/decode helpers stay private; the wire-format
invariant is asserted by temporal-proto-runtime's TemporalDeserializable
impl, not duplicated here.
EOF
)"
```

---

## Task 3: Client lifecycle functions

Implements: `attach_handle`, `random_workflow_id`, `start_workflow_proto`, `start_workflow_proto_empty`, `wait_result_proto`, `wait_result_unit`, plus a convenience `connect(url, namespace)` for consumers.

**Files:**
- Modify: `crates/temporal-proto-runtime-bridge/src/lib.rs`

- [ ] **Step 1: Append the functions to `src/lib.rs`**

Insert *before* the `#[cfg(test)] mod tests` block in `src/lib.rs`:

```rust
// ── Client construction ────────────────────────────────────────────────

/// Connect to a Temporal frontend and produce a [`TemporalClient`].
///
/// Convenience for the common case. Power users who need custom transport,
/// interceptors, or a vendored `Client` should construct one themselves and
/// call [`TemporalClient::from_client`] or [`TemporalClient::from_arc`].
pub async fn connect(url: &str, namespace: &str) -> Result<TemporalClient> {
    use temporalio_client::{ClientOptions, Connection, ConnectionOptions};
    use url::Url;

    let parsed = Url::parse(url).with_context(|| format!("parse temporal url {url}"))?;
    let connection = Connection::connect(ConnectionOptions::new(parsed).build())
        .await
        .context("connect to Temporal frontend")?;
    let client = Client::new(connection, ClientOptions::new(namespace.to_string()).build())
        .context("build Temporal client")?;
    Ok(TemporalClient::from_client(client))
}

// ── Workflow lifecycle ─────────────────────────────────────────────────

/// Attach to an existing workflow by ID. The returned handle has no run id
/// (the SDK will resolve to the most-recent run on each call).
pub fn attach_handle(client: &TemporalClient, workflow_id: String) -> WorkflowHandle {
    WorkflowHandle {
        client: client.clone(),
        workflow_id,
        run_id: None,
    }
}

/// Generate a fresh random workflow id. Used by the plugin when a workflow
/// has no proto-level `id` template — templated ids are materialised inline
/// as `<wf>_id(...)` functions and never reach this call site.
pub fn random_workflow_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Start a workflow with a proto-encoded input.
#[allow(clippy::too_many_arguments)]
pub async fn start_workflow_proto<I>(
    client: &TemporalClient,
    workflow_name: &'static str,
    workflow_id: &str,
    task_queue: &str,
    input: &I,
    id_reuse_policy: Option<WorkflowIdReusePolicy>,
    execution_timeout: Option<Duration>,
    run_timeout: Option<Duration>,
    task_timeout: Option<Duration>,
) -> Result<WorkflowHandle>
where
    I: TemporalProtoMessage,
{
    let payload = encode_proto_payload(input);
    let raw = RawValue::new(vec![payload]);
    let base = WorkflowStartOptions::new(task_queue.to_string(), workflow_id.to_string())
        .maybe_execution_timeout(execution_timeout)
        .maybe_run_timeout(run_timeout)
        .maybe_task_timeout(task_timeout);
    // bon builders use typestate — id_reuse_policy has #[builder(default)],
    // so we only call the setter when present.
    let options = match id_reuse_policy {
        Some(p) => base.id_reuse_policy(p.into()).build(),
        None => base.build(),
    };
    let handle = client
        .sdk()
        .start_workflow(UntypedWorkflow::new(workflow_name), raw, options)
        .await
        .with_context(|| format!("start workflow {workflow_name}"))?;
    let info = handle.info().clone();
    Ok(WorkflowHandle {
        client: client.clone(),
        workflow_id: info.workflow_id,
        run_id: info.run_id,
    })
}

/// Start a workflow whose input is `google.protobuf.Empty`. The plugin
/// emits a call to this function instead of `start_workflow_proto` when
/// the input message is Empty, avoiding the need to express `()` as a
/// `TemporalProtoMessage`.
#[allow(clippy::too_many_arguments)]
pub async fn start_workflow_proto_empty(
    client: &TemporalClient,
    workflow_name: &'static str,
    workflow_id: &str,
    task_queue: &str,
    id_reuse_policy: Option<WorkflowIdReusePolicy>,
    execution_timeout: Option<Duration>,
    run_timeout: Option<Duration>,
    task_timeout: Option<Duration>,
) -> Result<WorkflowHandle> {
    let raw = RawValue::new(vec![]);
    let base = WorkflowStartOptions::new(task_queue.to_string(), workflow_id.to_string())
        .maybe_execution_timeout(execution_timeout)
        .maybe_run_timeout(run_timeout)
        .maybe_task_timeout(task_timeout);
    let options = match id_reuse_policy {
        Some(p) => base.id_reuse_policy(p.into()).build(),
        None => base.build(),
    };
    let handle = client
        .sdk()
        .start_workflow(UntypedWorkflow::new(workflow_name), raw, options)
        .await
        .with_context(|| format!("start workflow {workflow_name}"))?;
    let info = handle.info().clone();
    Ok(WorkflowHandle {
        client: client.clone(),
        workflow_id: info.workflow_id,
        run_id: info.run_id,
    })
}

/// Wait for a workflow to complete and decode its single proto output.
pub async fn wait_result_proto<O>(handle: &WorkflowHandle) -> Result<O>
where
    O: TemporalProtoMessage,
{
    let raw = handle
        .untyped()
        .get_result(WorkflowGetResultOptions::builder().build())
        .await
        .context("await workflow result")?;
    let payload = raw
        .payloads
        .first()
        .context("workflow returned no payloads")?;
    decode_proto_payload::<O>(payload).context("decode workflow output")
}

/// Wait variant for workflows that return `google.protobuf.Empty`.
pub async fn wait_result_unit(handle: &WorkflowHandle) -> Result<()> {
    handle
        .untyped()
        .get_result(WorkflowGetResultOptions::builder().build())
        .await
        .context("await workflow result")?;
    Ok(())
}
```

- [ ] **Step 2: Add a test for `random_workflow_id` and `attach_handle`**

Append inside the `#[cfg(test)] mod tests` block (after the existing tests):

```rust
    #[test]
    fn random_workflow_id_produces_distinct_uuids() {
        let a = random_workflow_id();
        let b = random_workflow_id();
        assert_ne!(a, b);
        // UUID v4 canonical length = 36 (8-4-4-4-12 hex with hyphens).
        assert_eq!(a.len(), 36);
        // Spot-check format: hyphens at the canonical positions.
        let chars: Vec<char> = a.chars().collect();
        for &i in &[8usize, 13, 18, 23] {
            assert_eq!(chars[i], '-', "expected hyphen at position {i} in {a}");
        }
    }
```

Note: `attach_handle` would require constructing a `TemporalClient`, which requires a live `temporalio_client::Client`. That's covered by Task 7's compile-time verification on the example crate; we don't unit-test it here.

- [ ] **Step 3: Run tests**

```bash
cargo test -p temporal-proto-runtime-bridge
```

Expected: 4 tests pass (3 previous + `random_workflow_id_produces_distinct_uuids`).

- [ ] **Step 4: Run clippy + check on full target**

```bash
cargo clippy -p temporal-proto-runtime-bridge --all-targets -- -D warnings
```

Expected: PASS. If the SDK's `WorkflowStartOptions::new` signature has drifted from `(task_queue, workflow_id)`, fix the call site to match before continuing — the SDK pin is exact-patch so this should not happen mid-Phase-1.

- [ ] **Step 5: Commit**

```bash
git add crates/temporal-proto-runtime-bridge/src/lib.rs
git commit -m "$(cat <<'EOF'
bridge: client lifecycle (start/attach/wait_result + connect)

Implements start_workflow_proto, start_workflow_proto_empty (empty input
variant), attach_handle, random_workflow_id (uuid::Uuid::new_v4),
wait_result_proto, wait_result_unit. Also adds a convenience connect(url,
namespace) helper that wraps Connection + Client construction.
EOF
)"
```

---

## Task 4: Signal and query functions

Implements: `signal_proto`, `signal_unit`, `query_proto`, `query_proto_empty`.

**Files:**
- Modify: `crates/temporal-proto-runtime-bridge/src/lib.rs`

- [ ] **Step 1: Append the signal + query functions to `src/lib.rs`**

Add after the `wait_result_unit` definition, before `#[cfg(test)]`:

```rust
// ── Signals ────────────────────────────────────────────────────────────

/// Send a typed signal with proto input.
pub async fn signal_proto<I>(handle: &WorkflowHandle, name: &str, input: &I) -> Result<()>
where
    I: TemporalProtoMessage,
{
    let payload = encode_proto_payload(input);
    let raw = RawValue::new(vec![payload]);
    handle
        .untyped()
        .signal(
            UntypedSignal::<UntypedWorkflow>::new(name),
            raw,
            WorkflowSignalOptions::builder().build(),
        )
        .await
        .with_context(|| format!("send signal {name}"))?;
    Ok(())
}

/// Send a signal whose input is `google.protobuf.Empty`.
pub async fn signal_unit(handle: &WorkflowHandle, name: &str) -> Result<()> {
    let raw = RawValue::new(vec![]);
    handle
        .untyped()
        .signal(
            UntypedSignal::<UntypedWorkflow>::new(name),
            raw,
            WorkflowSignalOptions::builder().build(),
        )
        .await
        .with_context(|| format!("send signal {name}"))?;
    Ok(())
}

// ── Queries ────────────────────────────────────────────────────────────

/// Run a query with proto input and decode the typed response.
pub async fn query_proto<I, O>(handle: &WorkflowHandle, name: &str, input: &I) -> Result<O>
where
    I: TemporalProtoMessage,
    O: TemporalProtoMessage,
{
    let payload = encode_proto_payload(input);
    let raw_input = RawValue::new(vec![payload]);
    let raw_out: RawValue = handle
        .untyped()
        .query(
            UntypedQuery::<UntypedWorkflow>::new(name),
            raw_input,
            WorkflowQueryOptions::builder().build(),
        )
        .await
        .with_context(|| format!("run query {name}"))?;
    let payload = raw_out
        .payloads
        .first()
        .context("query returned no payloads")?;
    decode_proto_payload::<O>(payload).context("decode query output")
}

/// Run a query whose input is `google.protobuf.Empty`.
pub async fn query_proto_empty<O>(handle: &WorkflowHandle, name: &str) -> Result<O>
where
    O: TemporalProtoMessage,
{
    let raw_input = RawValue::new(vec![]);
    let raw_out: RawValue = handle
        .untyped()
        .query(
            UntypedQuery::<UntypedWorkflow>::new(name),
            raw_input,
            WorkflowQueryOptions::builder().build(),
        )
        .await
        .with_context(|| format!("run query {name}"))?;
    let payload = raw_out
        .payloads
        .first()
        .context("query returned no payloads")?;
    decode_proto_payload::<O>(payload).context("decode query output")
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p temporal-proto-runtime-bridge
```

Expected: 4 tests pass (no new tests added — these functions are SDK-call wrappers; Task 7's `cargo check` on the example crate exercises them at type-level).

- [ ] **Step 3: Clippy clean**

```bash
cargo clippy -p temporal-proto-runtime-bridge --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/temporal-proto-runtime-bridge/src/lib.rs
git commit -m "$(cat <<'EOF'
bridge: signal + query surfaces

Implements signal_proto, signal_unit, query_proto, query_proto_empty
against UntypedSignal/UntypedQuery + RawValue payloads. Mirrors the
PoC's verified call shape.
EOF
)"
```

---

## Task 5: Update functions

Implements: `update_proto`, `update_proto_empty`. The SDK's `execute_update` hardcodes `WaitForStage::Completed`; for the bridge we use `start_update` with the user-supplied stage, then call `get_result()` so the facade can always return `Result<O>` regardless of which stage the caller asked for (the SDK still acknowledges at that stage at the gRPC level).

**Files:**
- Modify: `crates/temporal-proto-runtime-bridge/src/lib.rs`

- [ ] **Step 1: Append update functions to `src/lib.rs`**

Add after the query functions, before `#[cfg(test)]`:

```rust
// ── Updates ────────────────────────────────────────────────────────────

use temporalio_client::WorkflowStartUpdateOptions;
use temporalio_common::WorkflowUpdateWaitStage;

fn wait_stage_from(policy: WaitPolicy) -> WorkflowUpdateWaitStage {
    match policy {
        WaitPolicy::Admitted => WorkflowUpdateWaitStage::Admitted,
        WaitPolicy::Accepted => WorkflowUpdateWaitStage::Accepted,
        WaitPolicy::Completed => WorkflowUpdateWaitStage::Completed,
    }
}

/// Send an update with proto input and wait for the result.
pub async fn update_proto<I, O>(
    handle: &WorkflowHandle,
    name: &str,
    input: &I,
    wait_policy: WaitPolicy,
) -> Result<O>
where
    I: TemporalProtoMessage,
    O: TemporalProtoMessage,
{
    let payload = encode_proto_payload(input);
    let raw_input = RawValue::new(vec![payload]);
    let update_handle = handle
        .untyped()
        .start_update(
            UntypedUpdate::<UntypedWorkflow>::new(name),
            raw_input,
            WorkflowStartUpdateOptions::builder()
                .wait_for_stage(wait_stage_from(wait_policy))
                .build(),
        )
        .await
        .with_context(|| format!("start update {name}"))?;
    let raw_out: RawValue = update_handle
        .get_result()
        .await
        .with_context(|| format!("await update {name} result"))?;
    let payload = raw_out
        .payloads
        .first()
        .context("update returned no payloads")?;
    decode_proto_payload::<O>(payload).context("decode update output")
}

/// Send an update whose input is `google.protobuf.Empty`.
pub async fn update_proto_empty<O>(
    handle: &WorkflowHandle,
    name: &str,
    wait_policy: WaitPolicy,
) -> Result<O>
where
    O: TemporalProtoMessage,
{
    let raw_input = RawValue::new(vec![]);
    let update_handle = handle
        .untyped()
        .start_update(
            UntypedUpdate::<UntypedWorkflow>::new(name),
            raw_input,
            WorkflowStartUpdateOptions::builder()
                .wait_for_stage(wait_stage_from(wait_policy))
                .build(),
        )
        .await
        .with_context(|| format!("start update {name}"))?;
    let raw_out: RawValue = update_handle
        .get_result()
        .await
        .with_context(|| format!("await update {name} result"))?;
    let payload = raw_out
        .payloads
        .first()
        .context("update returned no payloads")?;
    decode_proto_payload::<O>(payload).context("decode update output")
}
```

- [ ] **Step 2: Add a unit test for the `wait_stage_from` mapping**

Append inside the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn wait_stage_from_maps_to_sdk_stage() {
        use temporalio_common::WorkflowUpdateWaitStage as Stage;
        assert!(matches!(wait_stage_from(WaitPolicy::Admitted), Stage::Admitted));
        assert!(matches!(wait_stage_from(WaitPolicy::Accepted), Stage::Accepted));
        assert!(matches!(wait_stage_from(WaitPolicy::Completed), Stage::Completed));
    }
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p temporal-proto-runtime-bridge
```

Expected: 5 tests pass.

- [ ] **Step 4: Clippy clean**

```bash
cargo clippy -p temporal-proto-runtime-bridge --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/temporal-proto-runtime-bridge/src/lib.rs
git commit -m "$(cat <<'EOF'
bridge: update_proto + update_proto_empty

Wires the bridge's WaitPolicy through to WorkflowUpdateWaitStage at the
start_update call, then awaits get_result() so the facade can always
return Result<O>. Acknowledgement stage is honored at the gRPC level;
the call still blocks on completion (the facade signature doesn't
expose a partial-completion return path in Phase 1).
EOF
)"
```

---

## Task 6: With-start functions

Implements: `signal_with_start_workflow_proto`, `update_with_start_workflow_proto`. The SDK natively supports signal-with-start via `WorkflowStartOptions::start_signal`. Update-with-start has no SDK helper in 0.4 — we construct an `ExecuteMultiOperationRequest` and call the raw gRPC method directly.

**Files:**
- Modify: `crates/temporal-proto-runtime-bridge/src/lib.rs`

- [ ] **Step 1: Append `signal_with_start_workflow_proto` to `src/lib.rs`**

Add after the update functions, before `#[cfg(test)]`:

```rust
// ── With-start helpers ─────────────────────────────────────────────────

use temporalio_common::protos::temporal::api::common::v1::Payloads;

/// Atomically start a workflow and send it an initial signal. The plugin
/// emits a call to this function alongside the generated client whenever a
/// signal annotation declares `start: true`.
#[allow(clippy::too_many_arguments)]
pub async fn signal_with_start_workflow_proto<W, S>(
    client: &TemporalClient,
    workflow_name: &'static str,
    workflow_id: &str,
    task_queue: &str,
    workflow_input: &W,
    signal_name: &str,
    signal_input: &S,
    id_reuse_policy: Option<WorkflowIdReusePolicy>,
    execution_timeout: Option<Duration>,
    run_timeout: Option<Duration>,
    task_timeout: Option<Duration>,
) -> Result<WorkflowHandle>
where
    W: TemporalProtoMessage,
    S: TemporalProtoMessage,
{
    let workflow_payload = encode_proto_payload(workflow_input);
    let signal_payload = encode_proto_payload(signal_input);
    let workflow_raw = RawValue::new(vec![workflow_payload]);
    let signal_payloads = Payloads { payloads: vec![signal_payload] };

    let start_signal = WorkflowStartSignal::new(signal_name.to_string())
        .input(signal_payloads)
        .build();

    let base = WorkflowStartOptions::new(task_queue.to_string(), workflow_id.to_string())
        .maybe_execution_timeout(execution_timeout)
        .maybe_run_timeout(run_timeout)
        .maybe_task_timeout(task_timeout)
        .start_signal(start_signal);
    let options = match id_reuse_policy {
        Some(p) => base.id_reuse_policy(p.into()).build(),
        None => base.build(),
    };

    let handle = client
        .sdk()
        .start_workflow(UntypedWorkflow::new(workflow_name), workflow_raw, options)
        .await
        .with_context(|| format!("signal-with-start workflow {workflow_name}"))?;
    let info = handle.info().clone();
    Ok(WorkflowHandle {
        client: client.clone(),
        workflow_id: info.workflow_id,
        run_id: info.run_id,
    })
}
```

- [ ] **Step 2: Append `update_with_start_workflow_proto` to `src/lib.rs`**

This one calls the raw gRPC `execute_multi_operation` because `temporalio-client 0.4` doesn't expose a friendly wrapper. Add immediately after `signal_with_start_workflow_proto`:

```rust
use temporalio_common::protos::temporal::api::enums::v1::{
    TaskQueueKind, WorkflowIdConflictPolicy,
};
use temporalio_common::protos::temporal::api::taskqueue::v1::TaskQueue;
use temporalio_common::protos::temporal::api::update::v1 as update;
use temporalio_common::protos::temporal::api::update::v1::WaitPolicy as ProtoWaitPolicy;
use temporalio_common::protos::temporal::api::workflowservice::v1::{
    ExecuteMultiOperationRequest, StartWorkflowExecutionRequest, UpdateWorkflowExecutionRequest,
    execute_multi_operation_request::{Operation, operation::Operation as OperationKind},
};
use temporalio_common::protos::temporal::api::common::v1::WorkflowType;
use temporalio_client::WorkflowService;
use tonic::IntoRequest;

/// Atomically start a workflow and send it an initial update. The plugin
/// emits a call to this function alongside the generated client whenever an
/// update annotation declares `start: true`.
///
/// Backed by the server's `ExecuteMultiOperationRequest` gRPC, since
/// `temporalio-client 0.4` doesn't expose a friendly wrapper for this combo.
#[allow(clippy::too_many_arguments)]
pub async fn update_with_start_workflow_proto<W, U, O>(
    client: &TemporalClient,
    workflow_name: &'static str,
    workflow_id: &str,
    task_queue: &str,
    workflow_input: &W,
    update_name: &str,
    update_input: &U,
    wait_policy: WaitPolicy,
    id_reuse_policy: Option<WorkflowIdReusePolicy>,
    execution_timeout: Option<Duration>,
    run_timeout: Option<Duration>,
    task_timeout: Option<Duration>,
) -> Result<(WorkflowHandle, O)>
where
    W: TemporalProtoMessage,
    U: TemporalProtoMessage,
    O: TemporalProtoMessage,
{
    use temporalio_client::NamespacedClient;

    let sdk_client = client.sdk();
    let namespace = sdk_client.namespace();
    let identity = sdk_client.identity();

    let workflow_payload = encode_proto_payload(workflow_input);
    let update_payload = encode_proto_payload(update_input);

    let id_reuse = id_reuse_policy
        .map(sdk_enums::WorkflowIdReusePolicy::from)
        .unwrap_or(sdk_enums::WorkflowIdReusePolicy::Unspecified) as i32;

    let start = StartWorkflowExecutionRequest {
        namespace: namespace.clone(),
        workflow_id: workflow_id.to_string(),
        workflow_type: Some(WorkflowType { name: workflow_name.to_string() }),
        task_queue: Some(TaskQueue {
            name: task_queue.to_string(),
            kind: TaskQueueKind::Unspecified as i32,
            normal_name: String::new(),
        }),
        input: Some(Payloads { payloads: vec![workflow_payload] }),
        workflow_execution_timeout: execution_timeout.and_then(|d| d.try_into().ok()),
        workflow_run_timeout: run_timeout.and_then(|d| d.try_into().ok()),
        workflow_task_timeout: task_timeout.and_then(|d| d.try_into().ok()),
        workflow_id_reuse_policy: id_reuse,
        // Update-with-start needs a non-default conflict policy server-side;
        // UseExisting is the documented choice (start if absent, attach if
        // present).
        workflow_id_conflict_policy: WorkflowIdConflictPolicy::UseExisting as i32,
        request_id: uuid::Uuid::new_v4().to_string(),
        identity: identity.clone(),
        ..Default::default()
    };

    let update_request = UpdateWorkflowExecutionRequest {
        namespace: namespace.clone(),
        workflow_execution: Some(
            temporalio_common::protos::temporal::api::common::v1::WorkflowExecution {
                workflow_id: workflow_id.to_string(),
                run_id: String::new(),
            },
        ),
        wait_policy: Some(ProtoWaitPolicy {
            lifecycle_stage: sdk_enums::UpdateWorkflowExecutionLifecycleStage::from(wait_policy)
                as i32,
        }),
        request: Some(update::Request {
            meta: Some(update::Meta {
                update_id: uuid::Uuid::new_v4().to_string(),
                identity: identity.clone(),
            }),
            input: Some(update::Input {
                header: None,
                name: update_name.to_string(),
                args: Some(Payloads { payloads: vec![update_payload] }),
            }),
        }),
        ..Default::default()
    };

    let req = ExecuteMultiOperationRequest {
        namespace: namespace.clone(),
        operations: vec![
            Operation {
                operation: Some(OperationKind::StartWorkflow(start)),
            },
            Operation {
                operation: Some(OperationKind::UpdateWorkflow(update_request)),
            },
        ],
        resource_id: workflow_id.to_string(),
    };

    let response = WorkflowService::execute_multi_operation(&mut sdk_client.clone(), req.into_request())
        .await
        .with_context(|| format!("update-with-start workflow {workflow_name}"))?
        .into_inner();

    // Response.responses parallels request.operations. Op[0] = start, op[1] = update.
    let start_resp = response
        .responses
        .first()
        .and_then(|r| r.response.as_ref())
        .context("execute_multi_operation: missing start response")?;
    let update_resp = response
        .responses
        .get(1)
        .and_then(|r| r.response.as_ref())
        .context("execute_multi_operation: missing update response")?;

    use temporalio_common::protos::temporal::api::workflowservice::v1::execute_multi_operation_response::response::Response as RespKind;
    let run_id = match start_resp {
        RespKind::StartWorkflow(r) => r.run_id.clone(),
        _ => anyhow::bail!("execute_multi_operation: response[0] was not StartWorkflow"),
    };
    let update_payloads = match update_resp {
        RespKind::UpdateWorkflow(r) => r
            .outcome
            .as_ref()
            .and_then(|o| match &o.value {
                Some(update::outcome::Value::Success(s)) => Some(s.payloads.clone()),
                _ => None,
            })
            .context("execute_multi_operation: update outcome had no success payloads")?,
        _ => anyhow::bail!("execute_multi_operation: response[1] was not UpdateWorkflow"),
    };

    let update_payload = update_payloads
        .first()
        .context("update returned no payloads")?;
    let output: O = decode_proto_payload(update_payload).context("decode update output")?;

    Ok((
        WorkflowHandle {
            client: client.clone(),
            workflow_id: workflow_id.to_string(),
            run_id: if run_id.is_empty() { None } else { Some(run_id) },
        },
        output,
    ))
}
```

Note on imports: this block adds several `use` statements at the function-block boundary. After this task, consolidate all imports at the top of `lib.rs` in Task 8's cleanup.

- [ ] **Step 3: Run tests + clippy**

```bash
cargo test -p temporal-proto-runtime-bridge && cargo clippy -p temporal-proto-runtime-bridge --all-targets -- -D warnings
```

Expected: 5 tests pass; clippy PASS. If clippy fires on the long argument lists, `#[allow(clippy::too_many_arguments)]` is already on every function — no further suppression needed.

If the `update::outcome::Value::Success` variant path differs in the SDK proto types, use `cargo doc -p temporalio-common --open` and search for `update::v1::Outcome` to verify the exact path before correcting the match arm. The proto path is stable across patch versions of the SDK.

- [ ] **Step 4: Commit**

```bash
git add crates/temporal-proto-runtime-bridge/src/lib.rs
git commit -m "$(cat <<'EOF'
bridge: signal_with_start + update_with_start

signal_with_start uses WorkflowStartOptions::start_signal (SDK 0.4 native
path). update_with_start has no SDK helper, so this builds an
ExecuteMultiOperationRequest with [StartWorkflow, UpdateWorkflow] ops
and calls WorkflowService::execute_multi_operation directly. Conflict
policy is UseExisting per the server contract (start if absent, attach
if present).
EOF
)"
```

---

## Task 7: Wire the bridge into the example crate

Phase 1's exit criterion is "the example compiles end-to-end against the bridge crate." We add a `bridge` cargo feature on `examples/job-queue-integration` that swaps the `pub mod temporal_runtime;` stub for `pub use temporal_proto_runtime_bridge as temporal_runtime;`, plus a `just verify-bridge` recipe that runs `cargo check --features bridge`.

**Files:**
- Modify: `examples/job-queue-integration/Cargo.toml`
- Modify: `examples/job-queue-integration/src/lib.rs`
- Modify: `examples/job-queue-integration/justfile`

- [ ] **Step 1: Add the optional dep + feature in `examples/job-queue-integration/Cargo.toml`**

Edit `examples/job-queue-integration/Cargo.toml`. Add:

```toml
[dependencies]
anyhow = { workspace = true }
prost = { workspace = true }
prost-types = { workspace = true }
temporal-proto-runtime = { path = "../../crates/temporal-proto-runtime" }
# Phase-1 bridge crate — opt-in via `bridge` feature. Default builds keep
# the stub temporal_runtime.rs so the example stays SDK-free in CI.
temporal-proto-runtime-bridge = { path = "../../crates/temporal-proto-runtime-bridge", optional = true }

[features]
default = []
# Swap the stub temporal_runtime.rs for the real bridge crate. Builds with
# real SDK calls — used by `just verify-bridge` to prove the bridge is wire-
# compatible with the plugin's generated emit.
bridge = ["dep:temporal-proto-runtime-bridge"]

[build-dependencies]
prost-build = { workspace = true }
```

Important: replace the entire `[dependencies]` block; the existing keys are preserved verbatim above plus the new optional dep + feature gate.

- [ ] **Step 2: Cfg-gate the temporal_runtime mod in `lib.rs`**

Edit `/Users/wcygan/Development/protoc-gen-rust-temporal/examples/job-queue-integration/src/lib.rs`. Replace:

```rust
pub mod temporal_runtime;
```

with:

```rust
// Default build: stub temporal_runtime.rs with `todo!()` bodies — keeps the
// workspace SDK-free in CI. With `--features bridge`, swap to the real
// bridge crate; the plugin's generated emit calls `crate::temporal_runtime::*`
// either way, so this single re-export is the only knob.
#[cfg(not(feature = "bridge"))]
pub mod temporal_runtime;
#[cfg(feature = "bridge")]
pub use temporal_proto_runtime_bridge as temporal_runtime;
```

- [ ] **Step 3: Add the `verify-bridge` justfile recipe**

Edit `/Users/wcygan/Development/protoc-gen-rust-temporal/examples/job-queue-integration/justfile`. Append:

```just
# Build the example against the bridge crate, proving the plugin's generated
# emit + the bridge's facade impl are wire-compatible end-to-end. Phase 1
# exit criterion (per docs/superpowers/specs/2026-05-12-cludden-parity-design.md).
verify-bridge:
    cargo check -p job-queue-integration-example --features bridge
    cargo clippy -p job-queue-integration-example --features bridge --all-targets -- -D warnings
```

- [ ] **Step 4: Run the recipe**

```bash
cd examples/job-queue-integration && just verify-bridge
```

Expected: PASS. The plugin's generated code (`src/gen/jobs/v1/jobs_temporal.rs`) compiles against `crate::temporal_runtime::*` resolved through the bridge crate. Any signature drift between the bridge crate and what the plugin emits fails here loudly — that's the design.

If verify-bridge fails with "unresolved import `temporal_runtime`" because the example's generated emit doesn't exercise update/with-start, that's still a real failure — the bridge crate must export every documented symbol regardless. Add missing functions or fix typos at this step.

- [ ] **Step 5: Run the full workspace tests to confirm nothing else regressed**

```bash
cd /Users/wcygan/Development/protoc-gen-rust-temporal && cargo test --workspace --all-targets
```

Expected: PASS. The bridge feature is opt-in, so the workspace's default build path is unchanged.

- [ ] **Step 6: Commit**

```bash
git add examples/job-queue-integration/Cargo.toml examples/job-queue-integration/src/lib.rs examples/job-queue-integration/justfile
git commit -m "$(cat <<'EOF'
example: bridge feature + just verify-bridge

Adds an opt-in `bridge` cargo feature on the example crate that swaps
the stub temporal_runtime.rs for the real bridge crate. Default build
stays SDK-free for CI; verify-bridge proves the bridge is wire-
compatible with the plugin's emit (Phase 1 exit criterion).
EOF
)"
```

---

## Task 8: CI integration

Wire `verify-bridge` into the existing `ci.yml` so every PR proves the bridge crate compiles with the plugin's emit.

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add a new job step in `.github/workflows/ci.yml`**

Edit `.github/workflows/ci.yml`. Add a new `verify-bridge` job (right after the `test` job, before `msrv` to keep the dependency order readable):

```yaml
  verify-bridge:
    # Phase 1 exit criterion: prove the bridge crate is wire-compatible
    # with the plugin's emit. Builds examples/job-queue-integration with
    # --features bridge and clippy-cleans it. Default workspace builds
    # remain SDK-free — this is the dedicated bridge gate.
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install protoc
        uses: arduino/setup-protoc@v3
        with:
          version: "27.x"
          repo-token: ${{ secrets.GITHUB_TOKEN }}
      - uses: Swatinem/rust-cache@v2
      - name: Build bridge crate
        run: cargo build -p temporal-proto-runtime-bridge --all-targets
      - name: Bridge crate tests
        run: cargo test -p temporal-proto-runtime-bridge
      - name: Verify example against bridge
        working-directory: examples/job-queue-integration
        run: |
          cargo check -p job-queue-integration-example --features bridge
          cargo clippy -p job-queue-integration-example --features bridge --all-targets -- -D warnings
```

- [ ] **Step 2: Validate the YAML locally**

```bash
# Spot-check the new job: yamllint if installed, otherwise grep for syntax.
grep -n "verify-bridge\|jobs:\|runs-on" .github/workflows/ci.yml | head -30
```

Expected: see the new `verify-bridge:` job listed alongside `fmt`, `clippy`, `test`, `msrv`, `compat-audit`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
ci: verify-bridge job

Compiles the bridge crate, runs its unit tests, and builds the
example with --features bridge + clippy-D warnings. Phase 1 gate
against bridge/plugin drift.
EOF
)"
```

---

## Task 9: Documentation + design follow-up cleanup

Updates `CHANGELOG.md`, `README.md`, `docs/RUNTIME-API.md`, and resolves the design's open follow-up about pinning the cludden commit.

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `README.md`
- Modify: `docs/RUNTIME-API.md`
- Modify: `docs/superpowers/specs/2026-05-12-cludden-parity-design.md`

- [ ] **Step 1: Add an `[Unreleased]` entry to `CHANGELOG.md`**

Edit `/Users/wcygan/Development/protoc-gen-rust-temporal/CHANGELOG.md`. Replace the `## [Unreleased]` section with:

```markdown
## [Unreleased]

### Added
- **`temporal-proto-runtime-bridge 0.1.0`** — default consumer-facing bridge
  between the plugin's emit and `temporalio-client 0.4`. Drop one dep and one
  `pub use temporal_proto_runtime_bridge as temporal_runtime;` in your
  `lib.rs` and the generated code runs against the real SDK. See the design
  doc at `docs/superpowers/specs/2026-05-12-cludden-parity-design.md` Phase 1
  for the architectural rationale.
- New `examples/job-queue-integration` cargo feature `bridge` that swaps the
  stub `temporal_runtime.rs` for the bridge crate; `just verify-bridge`
  exercises end-to-end compilation. CI gates this on every PR via the new
  `verify-bridge` job.
- **Pinned cludden commit** documented in the design doc header (resolves the
  Phase 1 open follow-up).

### Notes
- Plugin output is unchanged in this release. Existing consumers on
  `protoc-gen-rust-temporal 0.1.1` can adopt the bridge crate without
  regenerating.
- SDK pinning: the bridge crate pins `temporalio-client = "=0.4.0"` exact-
  patch. SDK 0.5 will ship as `temporal-proto-runtime-bridge 0.2`; plugin
  emit is unaffected.
```

- [ ] **Step 2: Mention the bridge crate in repo `README.md`**

Read the existing `/Users/wcygan/Development/protoc-gen-rust-temporal/README.md` first to find the on-ramp section, then add a short paragraph after the install instructions. Exact location depends on the current README structure; if the README has a "Consumer wiring" or "Getting started" header, add this block right under it:

```markdown
## Consumer wiring (default)

Add the plugin's runtime helpers + the default bridge crate:

```toml
[dependencies]
temporal-proto-runtime = { version = "0.1", features = ["sdk"] }
temporal-proto-runtime-bridge = "0.1"
```

Then in your crate's `lib.rs`:

```rust
pub use temporal_proto_runtime_bridge as temporal_runtime;
```

That single re-export satisfies every `crate::temporal_runtime::*` reference
the plugin emits. Power users who need a custom transport, vendored SDK, or
test stub can drop the `pub use` and write `mod temporal_runtime;` against the
facade documented in [`docs/RUNTIME-API.md`](docs/RUNTIME-API.md).
```

If the README doesn't have a clear consumer-wiring section, append the block at the bottom under a new `## Consumer wiring` header — preserve all existing content above.

- [ ] **Step 3: Add a bridge-crate pointer to `docs/RUNTIME-API.md`**

Edit `/Users/wcygan/Development/protoc-gen-rust-temporal/docs/RUNTIME-API.md`. After the existing first paragraph (the one ending "...pointing at the call site."), insert:

```markdown
**Default implementation:** [`temporal-proto-runtime-bridge`](../crates/temporal-proto-runtime-bridge/)
ships a concrete impl of every function documented below, backed by
`temporalio-client 0.4`. Add it as a dep and `pub use temporal_proto_runtime_bridge as temporal_runtime;`
in your `lib.rs` to wire the plugin's generated code to the real SDK
without writing the facade yourself. The stub at
`examples/job-queue-integration/src/temporal_runtime.rs` stays the
canonical override reference for power users.
```

- [ ] **Step 4: Pin the cludden commit in the design doc**

Edit `/Users/wcygan/Development/protoc-gen-rust-temporal/docs/superpowers/specs/2026-05-12-cludden-parity-design.md`. Replace:

```markdown
**Pinned cludden commit:** *(TBD — record at phase 1 start; see "Magnitude" risk.)*
```

with (substituting the actual commit hash — look up `cludden/protoc-gen-go-temporal@v1.22.1` which is the version `compat-tests/` already targets):

```markdown
**Pinned cludden commit:** `cludden/protoc-gen-go-temporal@v1.22.1` (tag `v1.22.1`), matching what `compat-tests/` already audits against. Re-pinned only at Phase 5 start (per the spec's Nexus/XNS section).
```

Also remove the corresponding entry from the "Open follow-ups" section at the bottom of the design doc:

```markdown
- Pin the cludden commit at phase 1 start; record in this doc's header.
```

→ delete that line.

- [ ] **Step 5: Final verification — full workspace**

```bash
cd /Users/wcygan/Development/protoc-gen-rust-temporal && \
  cargo fmt --all -- --check && \
  cargo clippy --workspace --all-targets -- -D warnings && \
  cargo test --workspace --all-targets && \
  (cd examples/job-queue-integration && just verify-bridge)
```

Expected: all four PASS. If `cargo fmt --check` fires, run `cargo fmt --all` and stage the result before continuing.

- [ ] **Step 6: Commit**

```bash
git add CHANGELOG.md README.md docs/RUNTIME-API.md docs/superpowers/specs/2026-05-12-cludden-parity-design.md
git commit -m "$(cat <<'EOF'
docs: bridge crate launch + pin cludden commit

CHANGELOG entry, README on-ramp paragraph, RUNTIME-API.md pointer to
the bridge crate as the default impl, and design-doc commit pin
resolved (v1.22.1 to match compat-tests). Phase 1 paperwork complete.
EOF
)"
```

---

## Task 10 (optional / follow-up): Out-of-tree PoC migration

Per the design's Phase 1 description:

> PoC drops `jobs-proto/src/temporal_runtime.rs` and re-exports the bridge.
> **Exit criterion:** PoC's client-side integration tests still pass.

This work lives in `/Users/wcygan/Development/job-queue/` (a sibling repo), not in this repo, and is therefore optional for this plan. Run it after the bridge crate ships (either `0.1.0` published to crates.io or via path dep during local validation).

- [ ] **Step 1: Path-dep the bridge crate from the PoC** (if validating locally):

In `/Users/wcygan/Development/job-queue/crates/jobs-proto/Cargo.toml`, replace the SDK deps with:

```toml
temporal-proto-runtime-bridge = { path = "../../../protoc-gen-rust-temporal/crates/temporal-proto-runtime-bridge" }
```

(Drop `temporalio-client`, `temporalio-common`, `temporalio-sdk-core` deps — the bridge crate owns them.)

- [ ] **Step 2: Swap the PoC's `temporal_runtime.rs` for a re-export**

In `/Users/wcygan/Development/job-queue/crates/jobs-proto/src/lib.rs`:

```rust
// Was:
//   pub mod temporal_runtime;
// Now:
pub use temporal_proto_runtime_bridge as temporal_runtime;
```

Then delete `crates/jobs-proto/src/temporal_runtime.rs` (the 239 LOC adapter we already verified against the SDK).

- [ ] **Step 3: Run the PoC's integration suite**

```bash
cd /Users/wcygan/Development/job-queue && just demo  # or cargo test --workspace
```

Expected: PASS. If anything fails, the failure mode tells us where the bridge crate's behavior deviates from the PoC's hand-rolled adapter — fix the bridge, not the PoC.

This task is the design's Phase 1 exit criterion. Don't mark Phase 1 complete in this repo's CHANGELOG until the PoC migration passes.

---

## Self-review checklist

- [x] **Spec coverage.** Phase 1's three deliverables from the design are all covered:
  - Bridge crate publishing the current RUNTIME-API surface → Tasks 1–6.
  - `just verify-bridge` recipe on `examples/job-queue-integration` → Task 7.
  - PoC exit criterion → Task 10 (out-of-tree, explicitly optional).
- [x] **Placeholder scan.** No "TBD", no "handle edge cases", no "similar to Task N", no "fill in details". Each step has either a concrete edit, a verified command, or a precise navigation pointer (e.g. `cargo doc` lookup if the SDK proto path drifts).
- [x] **Type consistency.** `WorkflowHandle { client, workflow_id, run_id }` is constructed identically in every task that builds one (Tasks 3 / 6). `WorkflowIdReusePolicy` / `WaitPolicy` enum variant names match between bridge enum (Task 2), `From` impls (Task 2), and call sites (Tasks 3 / 5 / 6). `decode_proto_payload<O>` signature used uniformly across wait_result / query / update.

---

## Execution checkpoints

The plan partitions cleanly across nine in-repo tasks plus one optional follow-up. A reasonable subagent-driven cadence:

1. Tasks 1–2: bootstrap + types. One subagent, ~10 minutes.
2. Tasks 3–6: facade surface (functions in four commit batches). One subagent each, ~10–20 minutes apiece.
3. Tasks 7–8: example + CI wiring. One subagent.
4. Task 9: docs. One subagent.
5. Task 10: PoC migration. Manual / user-driven (out-of-tree).

For inline execution, a checkpoint after every task commit is the natural review cadence — each commit is self-contained and individually revertible.
