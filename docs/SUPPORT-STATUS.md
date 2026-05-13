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

## WorkflowOptions (`(temporal.v1.workflow)`)

| Field | Status | Notes |
|---|---|---|
| `name` | supported | Cross-language registration name; defaults to the proto's fully-qualified method name. |
| `aliases` | supported | Emitted as `<RPC>_WORKFLOW_ALIASES: &[&str]` and re-exposed on `<Workflow>Definition::WORKFLOW_ALIASES` when `workflows=true`. |
| `task_queue` | supported | Overrides `ServiceOptions.task_queue`; required if neither is set. |
| `id` | supported | Subset only: simple `{{ .Field }}` Go-template segments. R7 will support Bloblang. |
| `id_reuse_policy` | supported | Maps to `temporal_runtime::WorkflowIdReusePolicy`. |
| `execution_timeout`, `run_timeout`, `task_timeout` | supported | Folded into the generated start path as defaults; caller can override via `<Workflow>StartOptions`. |
| `query[]`, `signal[]`, `update[]` | supported (same-service only) | Same-service refs become typed handle methods. Fully-qualified refs are rejected by `validate.rs::check_ref` (R1). Per-entry sub-fields each have their own row below. |
| `retry_policy` | rejected | R5. |
| `search_attributes` | rejected | R5 + R7 (Bloblang). |
| `typed_search_attributes` | rejected | R5 + R7. |
| `parent_close_policy` | rejected | R5. |
| `workflow_id_conflict_policy` | supported | Plumbed through to `WorkflowStartOptions.id_conflict_policy`. Caller can override via `<Workflow>StartOptions::id_conflict_policy`. |
| `wait_for_cancellation` | rejected | R5. |
| `enable_eager_start` | supported | Plumbed through to `WorkflowStartOptions.enable_eager_workflow_start`. The generated `<Workflow>StartOptions` exposes `enable_eager_workflow_start: Option<bool>` so call sites can override the proto-declared default. |
| `retry_policy` | supported | Compiled to a `temporal_runtime::RetryPolicy` literal at the start path; caller can override via `<Workflow>StartOptions::retry_policy`. |
| `versioning_behavior` | rejected | R5. |
| `patches` | rejected | R8. |
| `namespace` | rejected | Deprecated in the schema; same rationale as `ServiceOptions.namespace`. |
| `cli.ignore` | supported | Filters the workflow out of the `cli=true` scaffold. Other `cli.*` fields are rejected — see below. |
| `cli.name` | rejected | R6. Would change the subcommand name. |
| `cli.usage` | rejected | R6. Would change the subcommand help text. |
| `cli.aliases` | rejected | R6. Would add subcommand aliases. |

### WorkflowOptions.Signal[] (nested)

| Field | Status | Notes |
|---|---|---|
| `ref` | supported | Must name a sibling rpc carrying `(temporal.v1.signal)`. |
| `start` | supported | Triggers emission of `<signal>_with_start`. |
| `cli` | rejected | R6. |
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
| `workflow_id_conflict_policy` | rejected | Bridge hardcodes `UseExisting`; R5. |
| `cli` | rejected | R6. |
| `xns` | rejected | R8. |

## ActivityOptions (`(temporal.v1.activity)`)

| Field | Status | Notes |
|---|---|---|
| `name` | supported | Cross-language activity name; defaults to the proto's fully-qualified method name. Used by the `activities=true` emit. |
| `task_queue` | rejected | R5. |
| `schedule_to_close_timeout` | rejected | R5. |
| `schedule_to_start_timeout` | rejected | R5. |
| `start_to_close_timeout` | rejected | R5. |
| `heartbeat_timeout` | rejected | R5. |
| `wait_for_cancellation` | rejected | R5. |
| `retry_policy` | rejected | R5. |

## SignalOptions (`(temporal.v1.signal)`)

| Field | Status | Notes |
|---|---|---|
| `name` | supported | |
| `cli` | intentionally ignored | R6. |
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
| `id` | rejected | R5 (workflow-id template targeting the parent). |
| `wait_for_stage` | rejected | R5. |
| `wait_policy` (deprecated) | rejected | R5. |
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

Read but not used; cross-namespace workflow execution is R8.

## Patch (`Patch`, `Patch.Version`, `Patch.Mode`)

The wrapper message is read for rejection purposes only; full patch
versioning is R8.
