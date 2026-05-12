# Cludden-parity reframe

**Status:** Design — awaiting implementation plan.
**Authors:** wcygan + Claude.
**Date:** 2026-05-12.
**Pinned cludden commit:** *(TBD — record at phase 1 start; see "Magnitude" risk.)*

## Goal

Reframe v1's "client-only emit, consumer-owned facade" decision into rough
functional parity with
[`cludden/protoc-gen-go-temporal`](https://cludden.github.io/protoc-gen-go-temporal/):
worker-side scaffolding, CLI emit, Nexus + XNS helpers, codec server, and
generated docs — all behind per-category plugin flags, all routed through
a default bridge crate that isolates `temporalio-sdk`'s pre-1.0 churn.

## Non-goals

- **Dropping the facade.** We keep the `crate::temporal_runtime::*` seam.
  Direct binding to `temporalio-sdk` from generated code is deferred until
  the Rust SDK signals 1.0 intent.
- **Chasing cludden's moving target.** Scope is frozen at the cludden
  commit pinned above. Features cludden adds after that commit are
  separate proposals.
- **TS sibling parity.** This design constrains only Rust emit. Wire
  format stays byte-identical to the TS sibling (and to cludden's Go
  runtime); the TS sibling is free to skip worker emit indefinitely.
- **Strict semver during the reframe.** All phases land as `0.1.x`
  patches; the `0.2.0` / `1.0.0` cut happens at the end of phase 6.

## Decisions captured during brainstorming

| Question | Choice |
|---|---|
| Worker-emit scope | Match cludden's full surface (workers + CLI + Nexus + XNS + codec + docs). |
| Facade strategy | Keep the seam; ship a default bridge crate. |
| Release phasing | Phased patch releases bumping `0.1.x`. |
| PoC role | `~/Development/job-queue` drives the design; migrates per-phase. |
| Emit-flag surface | Per-category plugin flags (`workers=true,cli=true,...`). |
| Phase ordering | PoC-needs-first (Option A from brainstorming). |

## Architecture

### Bridge crate

**New crate:** `temporal-proto-runtime-bridge` (sibling to
`temporal-proto-runtime`).

A concrete implementation of the `crate::temporal_runtime::*` surface the
plugin emits against, backed by `temporalio-client` / `temporalio-sdk`
0.4. Consumers add one dep:

```toml
temporal-proto-runtime-bridge = "0.1"
```

…and a re-export in their `lib.rs`:

```rust
pub use temporal_proto_runtime_bridge as temporal_runtime;
```

That's the whole bridge wiring. The hand-written `temporal_runtime.rs`
becomes optional — only consumers who stub for tests or pin a vendored
SDK keep their own.

**Layering:**

```
generated code
   └─→ crate::temporal_runtime  (alias)
          └─→ temporal-proto-runtime-bridge  (default: real SDK)
                 └─→ temporalio-client / temporalio-sdk 0.4
```

Power-user override: drop the `pub use`, write `mod temporal_runtime;`
against the trait surface, done. Plugin emit shape doesn't change — it
still calls `crate::temporal_runtime::*`.

**SDK pinning.** Bridge crate pins `temporalio-client = "=0.4.x"` (exact
patch) and re-exports nothing SDK-typed in its public API — only the
facade functions. When SDK 0.5 lands, we cut
`temporal-proto-runtime-bridge = "0.2"` against the new SDK; plugin
output is unchanged; consumers bump the bridge-crate version. This is
the load-bearing reason for the seam.

**Facade trait surface grows over phases.** Phase 1 ships exactly the
functions documented in `docs/RUNTIME-API.md` today (client-side).
Phases 2+ extend it with worker-side functions (activity execution,
child workflow execution, registration). The bridge crate adds impls as
the trait grows; consumers who skipped optional plugin flags don't
reference the new symbols.

**Sub-trait split** (mitigation for facade bloat, see Risks):

```rust
pub trait ClientRuntime { /* phase 1 surface */ }
pub trait ActivityRuntime { /* phase 2 surface */ }
pub trait WorkflowRuntime { /* phase 3 surface */ }
pub trait CliRuntime { /* phase 4 surface */ }
pub trait NexusRuntime { /* phase 5 surface */ }
pub trait XnsRuntime { /* phase 5 surface */ }
pub trait CodecRuntime { /* phase 6 surface */ }
```

Bridge crate provides one struct implementing all sub-traits. Power-user
overrides impl only what they use.

**Example crate migration.** The in-tree
`examples/job-queue-integration` keeps its current stub `temporal_runtime.rs`
as a minimal-override reference. A new `just verify-bridge` recipe
builds a sibling configuration against the bridge crate end-to-end (no
`todo!()` panics if bodies execute under a real client).

### Plugin emit flags

**Grammar.** `protoc` passes plugin options via
`CodeGeneratorRequest.parameter` as a single comma-separated string
(`workers=true,cli=false,xns=true`). buf surfaces this through `opt:
[...]` in `buf.gen.yaml`. Parsed once in `main.rs` into a
`RenderOptions` struct, threaded through `parse → validate → render`.

**Flag set:**

| Flag | Default | Phase | Enables |
|---|---|---|---|
| (always-on) | — | 1 | Client surface — today's emit. |
| `activities` | `false` | 2 | `<Service>Activities` trait + `register_<service>_activities(...)` + typed `<Activity>::execute(...)` helpers. |
| `workflows` | `false` | 3 | `<Workflow>` trait + `register_<service>_workflows(...)` + typed child-workflow execution + signal/query/update receiver scaffolding. |
| `cli` | `false` | 4 | `<service>_cli::Cli` (clap-derived) with subcommands per workflow/signal/query/update. |
| `nexus` | `false` | 5 | Nexus operation helpers (when `temporal.v1.nexus` annotations exist). |
| `xns` | `false` | 5 | Cross-namespace helpers. |
| `codec` | `false` | 6 | Codec-server scaffolding. |
| `docs` | `false` | 6 | Markdown reference dumped alongside the `.rs` output. |

**Unknown-flag behaviour.** Strict — unknown keys return a
`CodeGeneratorResponse.error`. Avoids the silent-typo trap where
`opt: [worker=true]` (missing `s`) silently emits nothing.

**Validation rules.**

- Flags compose freely. `workflows=true` without `activities=true` is
  legal (a workflow may not call activities).
- `cli=true` with zero workflows annotated emits a warning, not an error.

**Layout.** `crates/protoc-gen-rust-temporal/src/options.rs` (new).
Parsed in `main.rs`, attached to `ServiceModel` so `render.rs` can
branch per-flag without re-parsing strings. Each render branch becomes
its own module (`render/client.rs`, `render/worker_activities.rs`, …) —
keeps `render.rs` from growing unboundedly as phases land.

## Per-phase emit + PoC migration

Each row is one plugin patch release; each ends with a job-queue PoC
migration that retires hand-rolled code.

### Phase 1 — Bridge crate

*Plugin: no change. Runtime: new crate.*

- Publish `temporal-proto-runtime-bridge 0.1.0` implementing the current
  `RUNTIME-API.md` surface against `temporalio-client 0.4`.
- Add `just verify-bridge` recipe to `examples/job-queue-integration`.
- PoC drops `jobs-proto/src/temporal_runtime.rs` and re-exports the bridge.
- **Exit criterion:** PoC's client-side integration tests still pass.

### Phase 2 — `activities=true`

*Plugin 0.1.x → next patch.*

Per `option (temporal.v1.activity) = {}` on a method, emit:

```rust
pub trait <Service>Activities: Send + Sync + 'static {
    async fn <activity>(&self, ctx: ActivityCtx, input: <Input>) -> Result<<Output>>;
    // ... one per activity ...
}

pub fn register_<service>_activities(
    worker: &mut Worker,
    impl_: Arc<dyn <Service>Activities>,
);

// Workflow-side typed call:
pub async fn <Activity>::execute(
    ctx: &WorkflowCtx,
    input: <Input>,
) -> Result<<Output>>;
```

Facade grows: `register_activity_proto<I, O>`, `execute_activity_proto<I, O>`.
Bridge implements both against `temporalio-sdk` activity primitives.

**PoC migration:** delete the hand-rolled `Activities` trait + manual
`worker.register_activity` calls; impl the generated trait.

### Phase 3 — `workflows=true`

*Plugin 0.1.x → next patch.*

Per workflow rpc, emit:

```rust
pub trait <Workflow>: Sized + Send + 'static {
    type Input;
    type Output;
    async fn run(self, ctx: WorkflowCtx, input: Self::Input) -> Result<Self::Output>;
    // signal/query/update handlers auto-derived from annotations:
    async fn on_<signal>(&mut self, ctx: &WorkflowCtx, input: <SigInput>) -> Result<()>;
    fn on_<query>(&self, ctx: &WorkflowCtx, input: <QInput>) -> Result<<QOutput>>;
    async fn on_<update>(&mut self, ctx: &WorkflowCtx, input: <UInput>) -> Result<<UOutput>>;
}

pub fn register_<service>_workflows(
    worker: &mut Worker,
    constructors: <Service>WorkflowConstructors,
);

// Workflow-side child-workflow call:
pub async fn <Workflow>::execute_child(
    ctx: &WorkflowCtx,
    input: <Input>,
    opts: <Workflow>ChildOptions,
) -> Result<<Output>>;
```

Facade grows substantially: `register_workflow_proto`,
`execute_child_workflow_proto`, signal/query/update receiver primitives,
`WorkflowCtx`.

**PoC migration:** delete hand-rolled workflow registration + hand-rolled
signal/query handler dispatch. Hand-written workflow *bodies* stay; only
registration and dispatch boilerplate goes.

### Phase 4 — `cli=true`

*Plugin 0.1.x → next patch.*

Per service: emit `pub mod <service>_cli { /* clap-derive Cli */ }` with
subcommands for each workflow/signal/query/update.

Bridge crate (with `features = ["cli"]`) exposes `Cli::run(self,
&client) -> Result<()>` dispatching to the generated client.

**PoC migration:** replace the ad-hoc `jobs-cli` binary with a 5-line
`main.rs` calling the generated CLI.

### Phase 5 — `nexus=true` + `xns=true`

*Plugin 0.1.x → next patch.*

Requires vendoring `temporal.v1.nexus` extensions from cludden's
schema. Our currently-vendored copy at
`crates/protoc-gen-rust-temporal/proto/temporal/v1/temporal.proto` has
no Nexus references — phase 5 starts with a re-vendor against cludden's
HEAD-at-pin (see top of doc) to pull them in. Emit Nexus operation
helpers + cross-namespace caller helpers.

PoC doesn't drive these directly — validated against new fixtures and a
sketch consumer in `compat-tests/`.

### Phase 6 — `codec=true` + `docs=true`

*Plugin 0.1.x → next patch, then promote.*

Codec-server scaffolding (smaller emit branch) and markdown reference
docs alongside the `.rs` output.

**Exit criterion for the whole reframe:** cut `0.2.0` (or `1.0.0` if
we're confident) once all six phases shipped and the PoC runs
end-to-end on the generated surface alone.

### Compat invariant across every phase

Wire format stays byte-identical to cludden's Go runtime —
`compat-tests/` keeps validating this on every PR. Worker emit doesn't
change what goes on the wire; it changes who writes the dispatch glue.

## Testing strategy

### Layer 1 — Plugin emit (in-tree, every PR)

Existing `parse_validate.rs` + `protoc_invoke.rs` get one new fixture
per flag combination that matters. Naming convention
`tests/fixtures/<branch>/`:

- `activities_only/` — activities + `opt: [activities=true]`.
- `workflows_only/` — workflows + `opt: [workflows=true]`.
- `full_cludden/` — workflows + activities + signals + queries + updates + `opt: [activities=true,workflows=true]`.
- One fixture per new flag (`cli_only`, `nexus_only`, `xns_only`, `codec_only`, `docs_only`).
- `all_flags_on/` — combinatorial sanity for flag-interaction bugs.

Each fixture asserts:
1. In-process render matches a checked-in snapshot.
2. Protoc-invoked output matches the in-process render.
3. Output compiles when included via `examples/job-queue-integration`
   (which gets sibling sub-examples per flag set).

### Layer 2 — Bridge crate (`cargo test -p temporal-proto-runtime-bridge`)

Unit tests against `temporalio-client`'s in-memory test harness —
exercises facade impls without a real server. One test per facade
function, asserting Payload round-trips through the SDK's mock
transport. This is the SDK-version compatibility net: when
`temporalio-sdk` 0.5 lands, bridge-crate tests break first.

### Layer 3 — PoC end-to-end (out-of-tree, gated on every plugin release)

Each phase's exit criterion is "PoC's existing integration suite passes
against the new generated surface." PoC's harness already brings up a
Temporal Server in Docker; we add `compat-tests/ci-poc-integration.sh`
that:

1. Bumps PoC's plugin dep to the candidate version.
2. Runs `cargo test --workspace` inside PoC.
3. Reports diff vs. baseline.

Lives outside required CI (PoC has its own commit cadence). A Release
Checklist row in `RELEASING.md` makes the green run a gate for tagging.

### Wire-format compat (unchanged)

`compat-tests/` continues asserting Rust ↔ Go Payload-JSON
byte-identicality on every PR.

### Snapshot policy

Worker emit produces significantly more code per service than
client-only emit. Each fixture gets its own `just regen-check` (the
pattern from `examples/job-queue-integration`), all run under `cargo
test --features snapshot-check`. Drift is loud (full diff in CI output)
and trivially fixable (`just regen` per fixture).

## Risks

| Risk | Mitigation |
|---|---|
| **`temporalio-sdk` 0.5 lands mid-project.** Bridge crate has to track every SDK reshape; PoC integration breaks until it does. | Bridge crate has independent release cadence. SDK bumps land as bridge minor versions (`0.1` → `0.2`); plugin emit unchanged. Bridge-crate tests (Layer 2) break first, before consumers see it. |
| **Facade trait surface bloats.** By phase 3 the facade has ~30 functions; power-user overrides become painful. | Sub-trait split documented in Architecture. Bridge crate impls all sub-traits via one struct; overrides impl only what they use. |
| **Rust SDK worker primitives don't compose with generated traits.** `temporalio-sdk`'s `#[workflow]` macro is pre-1.0 and may assume hand-written types; codegen-driven impls could hit limitations. | Phase 2 lead-in spike: prototype `register_<service>_activities` against `temporalio-sdk` 0.4 *before* committing to the trait shape in the implementation plan. If macros fight back, fall back to "consumer writes a 5-line registration helper using a generated trait" rather than a fully generated registration function. |
| **Patch-version semver violation.** Adding facade trait methods is technically breaking for power-user overrides; `0.1.x` patches mislead consumers running strict semver tooling (e.g., `cargo-semver-checks`). | Document explicitly in `CHANGELOG.md` that the project is `< 0.2` experimental and facade-trait additions ship as patches by design. Cut `0.2.0` (or `1.0.0`) at the end of phase 6 with strict semver from there. Power-user consumers can pin tightly (`temporal-proto-runtime-bridge = "=0.1.3"`) until ready. |
| **CLI emit drags `clap` into every consumer's dep tree.** Even consumers with `cli=false` get `clap` pulled by the bridge crate if we're not careful. | Bridge crate gates CLI support behind a Cargo feature: `temporal-proto-runtime-bridge = { version = "0.1", features = ["cli"] }`. Generated CLI module is `#[cfg(feature = "cli")]` on the bridge side. |
| **Nexus/XNS schema drift.** Cludden's annotation schema for Nexus may move between now and phase 5. | At phase 5 start, re-sync vendored proto against cludden's HEAD, run compat audit, document the pinned cludden commit in `VENDOR.md`. Already the pattern for the base schema. |
| **PoC scope drift.** If `~/Development/job-queue` evolves toward an unrelated direction, it stops being a useful test bed mid-project. | Each phase's PoC-migration commit is reviewed by the user before the plugin patch ships; if PoC has diverged, that's an explicit decision point — pause the design phase and re-scope. |
| **Generated workflow emit + worker-side dispatch may not be wire-compat with cludden's Go output**, even with matching Payload triple. | Add a cross-language workflow test in `compat-tests/`: a Go worker (cludden-generated) running a workflow that a Rust client starts, and vice-versa. Lives in `compat-tests/cross-language-workflow/`, runs on every release tag (not every PR — needs a Temporal Server). |
| **Magnitude.** Full cludden parity is a multi-quarter project. Scope creep mid-phase will stretch this further. | Spec freezes the surface at the pinned cludden commit (recorded at top of this doc). Anything cludden adds after is a separate proposal. |

## Open follow-ups

- Pin the cludden commit at phase 1 start; record in this doc's header.
- Decide bridge-crate Cargo feature naming convention (`cli`,
  `experimental-nexus`, …?) when phase 4/5 design lands.
- Decide whether `compat-tests/cross-language-workflow/` runs in repo
  CI (needs Docker) or out-of-band on release tags only.
