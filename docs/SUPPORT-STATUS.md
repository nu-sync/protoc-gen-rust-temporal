# Annotation support status

This is the single source of truth for every field on cludden's
`temporal.v1.*` annotation schema and how the Rust plugin treats it.
ROADMAP R1 requires that no annotation field be silently dropped — every
row below is one of:

| Status | Meaning |
|---|---|
| **supported** | Parsed and emitted into generated code. |
| **rejected** | Parsed but refused at parse or validate with a diagnostic, because honouring it would change runtime behaviour and the v1 emit does not yet plumb it through. Lifting the rejection is roadmap work. |
| **intentionally ignored** | Read by the parser but does not affect generated code today. The behaviour is documented and covered by tests so the silence is not accidental. |

A field that does not appear in this table is a bug — please open an
issue or add the row. The diagnostic-coverage test
`unsupported_field_support_status_table` in
`crates/protoc-gen-rust-temporal/tests/parse_validate.rs` exercises every
**rejected** row.

## ServiceOptions (`(temporal.v1.service)`)

| Field | Status | Notes |
|---|---|---|
| `task_queue` | supported | Used as the default `task_queue` for child workflow annotations that don't override. |
| `patches` | rejected | Workflow patch-versioning. R8 (advanced subsystems). |
| `namespace` | rejected | Deprecated in the schema; would change the effective Temporal namespace. R5 once a namespace option exists at the workflow runtime layer. |

## CLIOptions (`(temporal.v1.cli)` — service-level)

Separate extension from `(temporal.v1.service)`; configures the top-level
`Cli` struct generated under `cli=true`.

| Field | Status | Notes |
|---|---|---|
| `ignore` | supported | `true` suppresses the entire CLI module for the service, overriding the per-workflow `cli.ignore` heuristic. |
| `name` | supported | Threads into the `Cli` struct's `#[command(name = …)]`. Falls back to the service name in snake_case when unset. |
| `usage` | supported | Threads into the `Cli` struct's `#[command(about = …)]`. Falls back to a generated `"Generated Temporal CLI for <pkg>.<Svc>"` line when unset. |
| `aliases` | supported | Threads into the `Cli` struct's `#[command(alias = [...])]`. Empty when the proto omits the field. |

## WorkflowOptions (`(temporal.v1.workflow)`)

| Field | Status | Notes |
|---|---|---|
| `name` | supported | Cross-language registration name; defaults to the proto's fully-qualified method name. |
| `aliases` | supported | Emitted as `<RPC>_WORKFLOW_ALIASES: &[&str]` and re-exposed on `<Workflow>Definition::WORKFLOW_ALIASES` when `workflows=true`. |
| `task_queue` | supported | Overrides `ServiceOptions.task_queue`; required if neither is set. |
| `id` | supported | Subset only: simple `{{ .Field }}` Go-template segments. R7 will support Bloblang. |
| `id_reuse_policy` | supported | Maps to `temporal_runtime::WorkflowIdReusePolicy`. |
| `execution_timeout`, `run_timeout`, `task_timeout` | supported | Folded into the generated start path as defaults; caller can override via `<Workflow>StartOptions`. |
| `query[]`, `signal[]`, `update[]` | supported (same-service + cross-service) | Same-service refs become typed handle methods. Fully-qualified cross-service refs (`pkg.OtherSvc.Method`) resolve at parse against the descriptor pool — render fabricates a transient `SignalModel` / `QueryModel` / `UpdateModel` from the resolved target and emits the same typed handle method as a same-service ref. Signal-with-start + update-with-start free functions emit for cross-service refs too. Per-entry sub-fields each have their own row below. |
| `search_attributes` | supported (slices 1 + 2 + 3a + 3b string/int/bool refs) | The empty-map `root = {}` (slice 1), non-empty literal maps `root = { "Key": <literal>, … }` (slice 2) with string / signed-integer / boolean values, and `this.<field>` references targeting singular `string` / `int64` / `bool` input fields (slice 3) compile and flow through to `WorkflowStartOptions.search_attributes`. The field-ref encoder reads from the start path's `input` binding at call time. Repeated / map / message / enum fields and other scalar types (`int32`, `uint64`, `float`, `double`, `bytes`, …) plus `typed_search_attributes` remain rejected. See `docs/R7-BLOBLANG.md`. |
| `typed_search_attributes` | rejected | R5 + R7. |
| `parent_close_policy` | supported | Folds into a per-workflow `<rpc>_default_child_options() -> ChildWorkflowOptions` factory that bakes the policy in. Caller passes the result into `start_<workflow>_child(ctx, input, opts)`. |
| `workflow_id_conflict_policy` | supported | Plumbed through to `WorkflowStartOptions.id_conflict_policy`. Caller can override via `<Workflow>StartOptions::id_conflict_policy`. |
| `wait_for_cancellation` | supported | Child-only. Folds into `<rpc>_default_child_options()` as `cancel_type: ChildWorkflowCancellationType::WaitCancellationCompleted`. `false` (default) leaves the SDK's `Abandon` default in place. |
| `enable_eager_start` | supported | Plumbed through to `WorkflowStartOptions.enable_eager_workflow_start`. The generated `<Workflow>StartOptions` exposes `enable_eager_workflow_start: Option<bool>` so call sites can override the proto-declared default. |
| `retry_policy` | supported | Compiled to a `temporal_runtime::RetryPolicy` literal at the start path; caller can override via `<Workflow>StartOptions::retry_policy`. |
| `versioning_behavior` | rejected | R5. |
| `patches` | rejected | R8. |
| `namespace` | rejected | Deprecated in the schema; same rationale as `ServiceOptions.namespace`. |
| `cli.ignore` | supported | Filters the workflow out of the `cli=true` scaffold. |
| `cli.name` | supported | Threads into the per-workflow clap variants as `#[command(name = "start-<override>")]` / `#[command(name = "attach-<override>")]` so callers see the rename uniformly for both verbs. |
| `cli.usage` | supported | Emitted as `#[command(about = "<usage>")]` on both the start and attach variants, overriding the clap docstring-derived default. |
| `cli.aliases` | supported | Threads into the per-workflow clap variants as `#[command(alias = ["start-<a>", …])]` / `#[command(alias = ["attach-<a>", …])]`. Both verbs share the same alias list. |

### WorkflowOptions.Signal[] (nested)

| Field | Status | Notes |
|---|---|---|
| `ref` | supported | Must name a sibling rpc carrying `(temporal.v1.signal)`, or a fully-qualified cross-service ref. |
| `start` | supported | Triggers emission of `<signal>_with_start` (same-service and cross-service). |
| `cli` | supported | Per-ref `{name, aliases, usage}` flow into the `Signal<Name>` clap subcommand's `#[command(name = "signal-<name>", alias = [...], about = …)]`. Service-scoped CLI emit means the first workflow ref carrying overrides for a given signal wins. |
| `xns` | rejected | R8. |

### WorkflowOptions.Query[] (nested)

| Field | Status | Notes |
|---|---|---|
| `ref` | supported | Same-service only. |
| `xns` | rejected | R8. |

### WorkflowOptions.Update[] (nested)

| Field | Status | Notes |
|---|---|---|
| `ref` | supported | Same-service only. |
| `start` | supported | Triggers emission of `<update>_with_start`. |
| `validate` | supported | Threaded into the generated update call (no validator hook emitted yet — R2). |
| `workflow_id_conflict_policy` | supported | Threads through `update_with_start_workflow_proto[_unit]` so the start half of update-with-start honours the proto-declared override. `None` (proto unset) keeps the bridge's `UseExisting` fallback in place. |
| `cli` | supported | Per-ref `{name, aliases, usage}` flow into the `Update<Name>` clap subcommand's `#[command(name = "update-<name>", alias = [...], about = …)]`. Service-scoped CLI emit means the first workflow ref carrying overrides for a given update wins. |
| `xns` | rejected | R8. |

## ActivityOptions (`(temporal.v1.activity)`)

| Field | Status | Notes |
|---|---|---|
| `name` | supported | Cross-language activity name; defaults to the proto's fully-qualified method name. Used by the `activities=true` emit. |
| `task_queue` | supported | Folds into the per-activity `<rpc>_default_options()` factory. |
| `schedule_to_close_timeout` | supported | Used as the `close_timeouts` kicker (either alone via `ScheduleToClose`, or paired with `start_to_close_timeout` via `Both`). |
| `schedule_to_start_timeout` | supported | Chains onto the factory builder. |
| `start_to_close_timeout` | supported | Used as the `close_timeouts` kicker; preferred when paired (via `Both`). |
| `heartbeat_timeout` | supported | Chains onto the factory builder. |
| `wait_for_cancellation` | supported | `true` chains `.cancellation_type(ActivityCancellationType::WaitCancellationCompleted)` onto the factory builder; `false` (default) emits no setter so the SDK's `TryCancel` default stays. |
| `retry_policy` | supported | The factory converts the proto retry policy to the SDK's `RetryPolicy` and chains it onto the builder. |

## SignalOptions (`(temporal.v1.signal)`)

| Field | Status | Notes |
|---|---|---|
| `name` | supported | |
| `cli` | supported | Acts as the fallback default for the `Signal<Name>` CLI subcommand's `#[command(name, alias, about)]` when no `WorkflowOptions.signal[N].cli` workflow ref carries overrides. Workflow-ref overrides win when both are set. |
| `xns` | intentionally ignored | R8. |
| `patches` | intentionally ignored | R8. |

Signal rpcs must return `google.protobuf.Empty`; non-Empty outputs are
rejected by `validate.rs::validate_signal_outputs`.

## QueryOptions (`(temporal.v1.query)`)

| Field | Status | Notes |
|---|---|---|
| `name` | supported | |
| `cli` | intentionally ignored | R6. |
| `xns` | intentionally ignored | R8. |
| `patches` | intentionally ignored | R8. |

## UpdateOptions (`(temporal.v1.update)`)

| Field | Status | Notes |
|---|---|---|
| `name` | supported | |
| `validate` | supported | Surfaces on `UpdateModel.validate`; no validator hook generated yet. |
| `id` | supported | Workflow-id template targeting the parent workflow, resolved against the update input. Compiled to a private `<update>_workflow_id(input)` helper plus a `<update>_by_template(input, wait_policy)` client convenience that forwards to the update-by-id call. Only `{{ .Field }}` segments supported (R7 will add Bloblang). |
| `wait_for_stage` | supported | The update method's `wait_policy` arg is `Option<temporal_runtime::WaitPolicy>`; when the caller passes `None`, codegen folds in the proto-declared default. Fallback when proto declares none: `Completed`. |
| `wait_policy` (deprecated) | supported (fallback to `wait_for_stage`) | Cludden's Go plugin still honours the deprecated `wait_policy` on legacy protos; we do the same. `wait_for_stage` takes precedence when both are set. |
| `cli` | intentionally ignored | R6. |
| `xns` | intentionally ignored | R8. |
| `patches` | intentionally ignored | R8. |

## Co-annotations on a single rpc

cludden's schema permits multiple `temporal.v1.*` extensions on the same
`MethodOptions`. The Go plugin treats some combinations (workflow+activity,
signal+activity, update+activity) as meaningful. The Rust plugin **rejects
all combinations** at parse with a diagnostic naming the pair — R1 tracks
lifting the rejection.

## CLI-related schema (`CLIOptions`, `CLICommandOptions`, `CLIFlagOptions`, `CommandOptions`, `FieldOptions`)

The `cli=true` plugin option emits a clap-derive scaffold (parser only — no
`run()`); the entries above marked "intentionally ignored" for `cli` are
the per-method overrides not yet wired into that scaffold. R6 widens this.

## XNS-related schema (`XNSActivityOptions`)

**Out of scope.** Cross-namespace workflow execution is not pursued by this
plugin (see ROADMAP "R8 — Explicitly out of scope"). The `xns` field on every
method ref is refused at parse with an unsupported-field diagnostic so users
see the no-op explicitly.

## Patch (`Patch`, `Patch.Version`, `Patch.Mode`)

**Out of scope.** cludden's `Patch` annotation stages fix-version
migrations for the Go plugin's inline Bloblang expression evaluation
pattern. The Rust plugin compiles templates at codegen time and has no
inline-eval pattern to stage. The `patches` proto fields on both
`ServiceOptions` and `WorkflowOptions` are rejected at parse so users
see the no-op explicitly. See ROADMAP "R8 — Explicitly out of scope".

## Out-of-scope features

The following features from cludden's Go plugin are explicitly not pursued
here — see ROADMAP "R8 — Explicitly out of scope" for the reasoning. They do
not block "majority parity" against the Rust client/worker surface.

- **XNS (cross-namespace workflow execution).** `xns` annotation fields are
  rejected at parse.
- **Nexus services and operations.** Not generated.
- **Generated Markdown / API documentation.** Documentation lives in
  `docs/RUNTIME-API.md` and `docs/SUPPORT-STATUS.md` (this file); per-service
  generated docs would duplicate the surface and drift.
- **Go-specific naming knobs** (PascalCase/camelCase overrides, package
  paths, etc.). The proto-driven defaults this plugin already emits cover
  the same ground for Rust consumers.
- **Patch / protopatch handling.** The Rust plugin compiles templates at
  codegen time, so there's no inline-eval pattern to stage migrations for.
  `patches` proto fields are rejected at parse.

## Blocked on upstream SDK 0.4

These items have clean Go-plugin equivalents but cannot ship cleanly until
`temporalio-sdk` exposes the corresponding hook. Tracked in ROADMAP, not
out-of-scope, but not actionable in this codebase alone.

- **R2 signal-receive / select helpers and query/update handler hooks.**
  The SDK's `#[workflow_methods]` macro owns the dispatch from the wire to
  the consumer's struct methods. There's no public
  `WorkflowContext::signal_channel<S>()` (or query/update equivalent) for
  the plugin to wrap. See ROADMAP "R2 — Blocked on upstream SDK shape".
- **R5 `WorkflowOptions.versioning_behavior`.** `WorkflowImplementation` in
  SDK 0.4 has no `VERSIONING_BEHAVIOR` const for the plugin's
  `register_<workflow>_workflow` to set.
- **R8 codec server generation.** No Rust SDK surface to target; the
  codec-server pattern is a separate Go service today.
- **R8 generated test clients / mocks.** `temporalio-sdk` 0.4 does not
  expose a `TestWorkflowEnvironment` equivalent (see
  `docs/sdk-shape-worker.md`).
