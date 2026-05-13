# ROADMAP

**Status:** Active direction
**Date:** 2026-05-13
**Pinned Go reference:** `cludden/protoc-gen-go-temporal@v1.22.1`

## Goal

Move `protoc-gen-rust-temporal` from a typed-client generator toward majority
parity with `protoc-gen-go-temporal`.

Majority parity means an annotated proto that uses the common Go plugin surface
can generate useful Rust client and worker code without proto forks. It does
not mean every Go-specific convenience lands immediately, or that Rust hides
limitations in `temporalio-sdk` 0.4. Unsupported features should be explicit at
protoc time or documented as intentionally ignored while they are on this
roadmap.

The highest-value parity work is worker-side ergonomics:

- Generated worker implementation surface, beyond today's registration
  contracts.
- Typed activity execution helpers usable from workflow code.
- Broader client operations and runtime option coverage so generated Rust code
  follows the same proto semantics as the Go generator.

## Principles

1. Preserve the wire format. `WIRE-FORMAT.md` remains the compatibility
   contract for cross-language execution.
2. Do not fork cludden's schema. We consume the annotation schema and map its
   semantics to Rust as far as the SDK allows.
3. Prefer Go semantics when they are portable. Where Rust cannot match Go
   exactly, choose a compile-time error or a documented Rust-specific shape.
4. Keep generated code routed through `crate::temporal_runtime`. The default
   bridge can grow with the roadmap while consumers can still provide custom
   facades.
5. Make unsupported fields visible. Silent no-ops are only acceptable when
   documented and covered by tests.
6. Every roadmap phase updates fixtures, generated-surface tests,
   `docs/RUNTIME-API.md`, and at least one realistic example when the emitted
   API changes.

## Current Baseline

The current `0.1.x` line supports:

- Typed workflow start, attach, result, signal, query, and update client calls.
- Signal-with-start and update-with-start for supported non-Empty input shapes.
- Basic workflow start options: workflow id, task queue, id reuse policy, and
  workflow timeout fields currently emitted by the generator.
- `binary/protobuf` payload compatibility through `temporal-proto-runtime`.
- A default `crate::temporal_runtime` implementation in
  `temporal-proto-runtime-bridge`.
- Opt-in activity contracts with `activities=true`: activity name constants,
  a service activity trait, and registration helper.
- Opt-in workflow contracts with `workflows=true`: workflow definition traits,
  handler-name constants, and registration helpers.
- Opt-in CLI parser scaffolding with `cli=true`.

The baseline intentionally leaves large Go-plugin surfaces incomplete. The
sections below define the order in which those gaps should close.

## Priority Order

| Roadmap item | Target | Why it matters |
|---|---|---|
| R0 | Documentation and stale-scope cleanup | Make parity the project direction and keep current limits honest. |
| R1 | Parser/model parity foundation | Co-annotations, cross-service refs, aliases, and unsupported-field tracking unblock later emit. |
| R2 | Generated worker implementation surface | Highest workflow-author ergonomics gap versus Go. |
| R3 | Activity execution helpers from workflows | Lets workflow code call annotated activities through typed generated APIs. |
| R4 | Client API breadth | Brings day-to-day operations closer to the Go generated client. |
| R5 | Runtime option coverage | Makes proto options behave consistently across Go and Rust. |
| R6 | CLI execution surface | Turns today's parser scaffold into a useful generated command runner. |
| R7 | Bloblang-backed templates | Supports Go-style workflow/update ids and search attributes. |
| R8 | Advanced subsystems | Codec server, test clients, patch handling, and other lower-frequency Go features. (XNS, Nexus, generated docs, and Go-specific naming knobs are explicitly out of scope — see R8 below.) |

## R0 - Documentation Alignment

Status: completed 2026-05-13.

Deliverables:

- Add this roadmap as the active parity plan.
- Update `CLAUDE.md` so future work treats `ROADMAP.md` as required context.
- Reword stale docs that describe current gaps as permanent non-goals.
- Mark older parity design notes as historical when they conflict with this
  roadmap.

## R1 - Parser and Model Foundation

This phase should happen before richer emit. The current model is shaped around
one primary method kind and same-service workflow attachment. Go supports a
broader set of relationships.

Progress:

- 2026-05-13: workflow `aliases` emitted as `<RPC>_WORKFLOW_ALIASES: &[&str]`
  module const plus `Definition::WORKFLOW_ALIASES` associated const under
  `workflows=true`. Fixtures `workflow_aliases` and `worker_workflow_aliases`
  cover both branches; existing goldens unaffected when no aliases are
  declared. See `docs/RUNTIME-API.md`.
- 2026-05-13: every `reject_unsupported_*` branch in `parse.rs` is locked in
  by a table-driven `unsupported_field_support_status_table` test. Closes the
  R1 ask to add a test for each unsupported-field diagnostic. New rejection
  rules must add a row to that table so silent drops cannot regress.
- 2026-05-13: co-annotations on a single rpc are now refused at parse with a
  diagnostic naming the combination. Previously `method_kind` did first-match
  on the extension chain and silently dropped the others — a service with
  workflow+activity could ship with the activity invisibly missing.
  `co_annotations_are_rejected_with_clear_diagnostic` covers all three
  combinations R1 calls out. Lifting the rejection is the natural next step
  toward full co-annotation support.
- 2026-05-13: cross-service workflow refs now surface an explicit
  "cross-service refs are not yet supported" diagnostic instead of the generic
  "no sibling rpc carries…" error. `validate.rs::check_ref` detects the
  fully-qualified syntax (target contains `.`) and points users at R1. Test
  `cross_service_ref_is_rejected_with_clear_diagnostic` locks the diagnostic
  in. Full resolution against the descriptor pool remains R1 work.
- 2026-05-13: closed four previously-silent drops — `WorkflowOptions.patches`,
  `WorkflowOptions.namespace` (deprecated), `ServiceOptions.patches`, and
  `ServiceOptions.namespace` now refuse at parse with the standard "does not
  yet honour" diagnostic. `reject_unsupported_service_options` is the new
  service-level rejection sink. Support-status table grew by four rows.
- 2026-05-13: published `docs/SUPPORT-STATUS.md` — the single source-of-truth
  index of every `temporal.v1.*` annotation field with its current status
  (supported / rejected / intentionally ignored). Closes the R1 ask for an
  explicit support-status table in code+tests. New test
  `support_status_doc_lists_every_rejected_field` cross-checks that every
  field named in a `reject_unsupported_*` list appears in the doc so the
  table cannot drift from the rejection rules. `CLAUDE.md` now requires
  reading it before adding or relaxing a rejection.
- 2026-05-13 (first R5 option): `WorkflowOptions.enable_eager_start` is now
  honoured — moves from rejected to supported, plumbed through to
  `WorkflowStartOptions.enable_eager_workflow_start` in the bridge.
  `<Workflow>StartOptions` gains `enable_eager_workflow_start: Option<bool>`
  so callers can override the proto-declared default. Bridge signatures for
  `start_workflow_proto` / `start_workflow_proto_empty` grew a trailing bool;
  the runtime-API doc bumps the signature to 0.1.2. Two new tests pin the
  positive path and the false baseline; example regenerated.
- 2026-05-13 (R5 — `parent_close_policy`): `(temporal.v1.workflow).parent_close_policy`
  graduates from rejected to supported. Under `workflows=true`, every
  workflow that declares the policy now ships
  `pub fn <rpc>_default_child_options() -> ChildWorkflowOptions` that
  bakes the policy in via `..::std::default::Default::default()` spread.
  Caller passes the result straight into `start_<workflow>_child(ctx,
  input, opts)`. New facade enum `temporal_runtime::worker::ParentClosePolicy`
  with `From<…> for temporalio_common::…::ParentClosePolicy` impl. Two
  new positive tests; support-status drift table loses the row.
- 2026-05-13 (R3 — last ActivityOptions rejection closes): proto
  `wait_for_cancellation = true` now folds into the per-activity factory
  as `.cancellation_type(ActivityCancellationType::WaitCancellationCompleted)`;
  `false` (default) emits no setter so the SDK's `TryCancel` default
  stays. Bridge re-exports `ActivityCancellationType` from
  `temporalio_common::protos::coresdk::workflow_commands`. With this,
  **no `ActivityOptions` field is rejected anymore** — all six runtime
  fields fold into the factory. Two new positive tests; old rejection
  test deleted; support-status drift table loses its last activity row.
- 2026-05-13 (R3 — activity option builders): every activity that
  declares at least one close-timeout in `(temporal.v1.activity)` now
  also ships `pub fn <rpc>_default_options() -> ActivityOptions` that
  builds the SDK's `ActivityOptions` with the proto defaults baked in.
  Picks the right `ActivityCloseTimeouts` variant (`StartToClose`,
  `ScheduleToClose`, or `Both`) and chains `task_queue`,
  `schedule_to_start_timeout`, `heartbeat_timeout`, and `retry_policy`
  onto the builder. Five previously-rejected `ActivityOptions` fields
  flip from rejected to supported; `wait_for_cancellation` stays
  rejected (no clean mapping to the SDK's `ActivityCancellationType`
  yet). Bridge re-exports `ActivityCloseTimeouts`. Four new tests,
  support-status drift test absorbs the new rows.
- 2026-05-13 (R2 — external-signal markers + helpers): under
  `workflows=true`, every non-Empty signal attached to at least one
  non-Empty workflow now ships a `<RPC>Signal` marker +
  `SignalDefinition` impl plus a
  `signal_<rpc>_external<W>(ctx, workflow_id, run_id, input) ->
  SignalExternalWfResult` helper. Lets workflow code coordinate with
  sibling workflows by id without dropping into the SDK's raw
  `ExternalWorkflowHandle` API. The marker's `Workflow` associated type
  resolves to the first non-Empty attaching workflow on the service —
  the SDK doesn't validate it at the external-signal dispatch site
  (target is identified by id), but `SignalDefinition` requires it.
  Bridge re-exports `SignalDefinition`, `ExternalWorkflowHandle`,
  `SignalExternalOk`, and `SignalExternalWfResult`.
- 2026-05-13 (R2 — continue-as-new helper): every non-Empty workflow
  under `workflows=true` now also ships a
  `continue_<workflow>_as_new<W>(ctx, input, opts)` helper, bound to
  `WorkflowImplementation<Run = <RPC>Workflow>`. Wraps the raw proto
  input in a `TypedProtoMessage` and forwards to `ctx.continue_as_new`.
  Returns `Result<Infallible, WorkflowTermination>` — calling code
  propagates the Err so the SDK's run loop performs the actual
  continue-as-new dispatch. Bridge re-exports `WorkflowImplementation`,
  `ContinueAsNewOptions`, and `WorkflowTermination`.
- 2026-05-13 (R2 — child-workflow markers + start helpers): under
  `workflows=true`, every workflow with non-Empty input AND output now
  ships a per-rpc `<RPC>Workflow` marker struct + `WorkflowDefinition`
  impl plus a workflow-side `start_<workflow>_child(ctx, input, opts) ->
  Result<StartedChildWorkflow<<RPC>Workflow>, ChildWorkflowStartError>`
  helper. Lets workflow code spawn typed child workflows without hand-
  writing the WorkflowDefinition impl. Bridge re-exports
  `WorkflowDefinition`, `ChildWorkflowOptions`, `StartedChildWorkflow`,
  and `ChildWorkflowStartError`. Empty-input/output workflows fall
  through the same orphan-rule gate as the activity markers (lifting
  this is a follow-up once `()` impls land). Two new tests pin the
  marker, helper, and Empty-skip paths.
- 2026-05-13 (R3 — local-activity variant): every non-Empty activity
  now also ships `execute_<activity>_local(ctx, input, opts) ->
  Result<<Output>, ActivityExecutionError>` alongside the regular
  helper. Routes through `ctx.start_local_activity(<RPC>Activity, input,
  opts)` + `LocalActivityOptions`. Useful for deterministic in-process
  work that doesn't need workflow task scheduling overhead. Empty-side
  variant suppression matches the regular helper.
- 2026-05-13 (R3 — typed workflow-side helper): every non-Empty
  activity now also ships `pub async fn execute_<activity><W>(ctx, input,
  opts) -> Result<<Output>, ActivityExecutionError>` next to its marker
  struct. The helper delegates to `ctx.start_activity(<RPC>Activity,
  input, opts)` and unwraps the `TypedProtoMessage` envelope so workflow
  bodies see the raw output type. Bridge now re-exports
  `WorkflowContext`, `ActivityOptions`, `LocalActivityOptions`, and
  `ActivityExecutionError` from the SDK. Existing positive test extended.
- 2026-05-13 (R3 — first activity-from-workflow step): under
  `activities=true`, every activity with non-Empty input AND output now
  gets a per-rpc marker struct (`<RPC>Activity`) plus an
  `impl temporal_runtime::worker::ActivityDefinition` carrying the typed
  `Input` / `Output` (wrapped in `TypedProtoMessage<T>` for the
  orphan-rule reasons documented in the bridge) and a `name()` that
  delegates to the existing `<RPC>_ACTIVITY_NAME` const. Workflow code
  can call `ctx.start_activity(<RPC>Activity, input, opts).await` against
  the SDK's typed activity entrypoint without hand-writing the
  ActivityDefinition. The bridge gains a top-level
  `pub use TypedProtoMessage;` re-export so generated code can spell
  the type without reaching into the inner crate. Empty-input/output
  activities still ship the name const but skip the marker, gated by an
  explicit comment in the emit — lifting the gate is a follow-up once
  `()` impls land upstream. Two new tests pin the typed and Empty-skip
  paths.
- 2026-05-13 (R5 — `UpdateOptions.wait_for_stage` + deprecated
  `wait_policy`): both fields now fold into a generated default. Every
  update method's `wait_policy` arg moves from `temporal_runtime::WaitPolicy`
  to `Option<temporal_runtime::WaitPolicy>` — callers pass `None` to use
  the proto-declared default. `wait_for_stage` takes precedence when both
  are set; the deprecated `wait_policy` is the fallback so Go-side legacy
  protos still honour their declared stage. Hard fallback when proto
  declares neither: `Completed`. Touches every update emit site (4 Handle
  methods, 4 client-by-id methods, `_with_start`, `_by_template`). New
  positive tests cover the typical, deprecated, and no-default paths.
  With this, `UpdateOptions` has no more rejected fields under R5.
- 2026-05-13 (R5 — `UpdateOptions.id` template): `(temporal.v1.update).id`
  graduates from rejected to supported. Reuses the existing
  `parse_id_template` machinery (now factored into `emit_id_fn`) against
  the update's *input* descriptor — each field reference resolves to a
  field on the update message, not the workflow. Render emits a private
  `<update>_workflow_id(input) -> String` derivation fn plus a
  `<service>Client::<update>_by_template(input, wait_policy)` convenience
  that derives the parent workflow id and forwards to the by-id update
  method. Only emitted when the proto declares the template; existing
  goldens for templateless updates unchanged.
- 2026-05-13 (R4 — client update-by-id, completes the by-id trifecta):
  `<Service>Client` now exposes `<update>(workflow_id [, input], wait_policy)`
  for every attached update rpc, full Empty matrix routed to
  `update_proto_empty_unit` / `update_proto_empty` / `update_unit` /
  `update_proto`. With this, signal/query/update-by-id are all in: a caller
  with only a workflow id can drive every workflow interaction through the
  `<Service>Client` directly. Two new tests cover the typed and Empty
  paths.
- 2026-05-13 (R4 — client query-by-id): `<Service>Client` now exposes
  `<query>(workflow_id [, input])` for every attached query rpc, with
  full Empty-variant matrix coverage (`query_proto_empty`,
  `query_proto_empty_unit`, `query_proto`, `query_unit`). Same internal
  pattern as signal-by-id: attach a handle then delegate to the existing
  bridge fn. Two new tests pin the Empty-in/Empty-out and the
  Empty-output paths.
- 2026-05-13 (R4 — client signal-by-id): `<Service>Client` now exposes
  `<signal>(workflow_id, input)` for every attached signal rpc on the
  service, mirroring the Go plugin's `client.<Signal>(ctx, id, runID, …)`.
  Empty-input variants take only `workflow_id` and route to `signal_unit`.
  Internally attaches a `WorkflowHandle` and reuses the existing bridge
  `signal_proto` / `signal_unit` helpers — no new runtime surface. Two
  new tests pin the typed and Empty paths.
- 2026-05-13 (R4): every generated `<Workflow>Handle` now exposes
  `run_id(&self) -> Option<&str>` forwarding to the facade. Returns `None`
  for attached handles, `Some(...)` after the start path populates it.
  Pinned by a positive test on `minimal_workflow`; the RUNTIME-API doc's
  `WorkflowHandle` row now mandates the accessor.
- 2026-05-13 (first R4 deliverable): every generated `<Workflow>Handle`
  now exposes `cancel_workflow(reason)` and `terminate_workflow(reason)`
  delegating to new bridge fns `temporal_runtime::cancel_workflow` /
  `temporal_runtime::terminate_workflow`. Named with the `_workflow`
  suffix so they cannot collide with a sibling proto rpc literally named
  `Cancel` or `Terminate` (the Go plugin uses the same disambiguation).
  Two new tests: positive method-shape assertion plus a belt-and-braces
  walk over all 8 representative fixtures that pins one cancel/terminate
  pair per workflow.
- 2026-05-13 (third R5 option): `WorkflowOptions.retry_policy` shipped
  end-to-end. New facade struct `temporal_runtime::RetryPolicy` (with private
  bits-encoded backoff_coefficient so `Eq` still derives) converts to
  `temporalio_common::…::common::v1::RetryPolicy`. The start path emits a
  `temporal_runtime::RetryPolicy { … }` literal carrying the proto-declared
  default; callers can override via `<Workflow>StartOptions::retry_policy`.
  Old "retry_policy is rejected" test replaced with positive coverage of all
  five RetryPolicy fields; new bridge unit test pins the SDK conversion.
  Adds `prost-wkt-types` workspace dep (matches temporalio-common 0.7).
- 2026-05-13 (second R5 option): `WorkflowOptions.workflow_id_conflict_policy`
  shipped end-to-end. New facade enum `temporal_runtime::WorkflowIdConflictPolicy`
  maps to `temporalio-common::WorkflowIdConflictPolicy` (`Fail` / `UseExisting`
  / `TerminateExisting`). `<Workflow>StartOptions::id_conflict_policy:
  Option<WorkflowIdConflictPolicy>` exposes the override; the start path
  folds the proto default in. The bridge handles all four
  reuse-policy × conflict-policy combinations against bon's typestate
  builder. Two positive tests added; the support-status row flips from
  rejected to supported.
- 2026-05-13 (R6 down-payment): `WorkflowOptions.cli.ignore` is now honoured.
  Setting `cli: { ignore: true }` filters the workflow out of the `cli=true`
  scaffold (no `Start<Wf>`/`Attach<Wf>` subcommand variant or Args struct
  emitted), and the whole CLI module is suppressed when every workflow opts
  out. Sibling `cli.name`/`cli.usage`/`cli.aliases` move from silent-drop to
  rejected so users can't expect them to take effect. Fixture `cli_ignore`,
  three new tests, drift test absorbs four new rows.

Deliverables:

- Model co-annotations that Go allows, especially:
  - workflow + activity
  - signal + activity
  - update + activity
- Support fully-qualified and cross-service refs for workflow
  signal/query/update attachments.
- Track aliases for workflow, activity, signal, query, and update registration
  names and emit them where the runtime surface can use them.
- Replace ad hoc ignored fields with an explicit support-status table in code
  and tests: supported, rejected, parsed-for-later, or intentionally ignored.
- Add fixtures for cross-service refs, aliases, co-annotations, and each
  unsupported-field diagnostic.

Done when:

- The parser can represent Go-compatible service shapes without losing
  annotation information.
- Unsupported fields fail loudly unless the docs and tests declare the no-op.

## R2 - Generated Worker Implementation Surface

Today's worker emit is a contract and registration layer. Go generates a richer
worker-facing surface that reduces hand wiring inside workflow implementations.

Target capabilities:

- Generated per-workflow implementation traits or adapter contracts that include
  the workflow run signature and attached signal/query/update handler shapes.
- Generated typed names and input/output structs where Go exposes them and where
  Rust needs them to make handler wiring readable.
- Signal receive/select helpers, subject to what `temporalio-sdk` exposes.
- Query and update handler helpers, including validation hooks for update
  validators.
- Continue-as-new helpers for workflows.
- Child-workflow and external-signal helpers when the SDK shape allows clean
  facade functions.

Non-goal for this phase:

- Generating business logic. Consumers still own workflow and activity bodies.

Done when:

- The job-queue example can move more workflow wiring to generated symbols
  without weakening SDK macro integration.
- The generated worker surface compiles with `clippy -D warnings` under the
  pinned SDK.

## R3 - Activity Execution Helpers From Workflows

This is the other high-value worker-side gap. Annotated activities should be
callable from workflow code through typed generated helpers instead of stringly
runtime calls.

Target capabilities:

- Generated typed helpers for executing each annotated activity from workflow
  code.
- Async activity result helpers that fit the Rust SDK's workflow future model.
- Local activity variants if the SDK exposes enough stable surface.
- Activity option builders that merge annotation defaults with call-site
  overrides.
- Runtime facade functions for workflow-context activity execution.

Done when:

- A workflow implementation can call annotated activities without spelling
  activity registration names or payload wrappers by hand.
- Activity execution options preserve Go-compatible annotation semantics where
  the SDK supports them.

## R4 - Client API Breadth

The Rust client surface should cover the common operations Go users get from
generated clients.

Target capabilities:

- Client-level signal, query, and update by workflow id, not only through a
  previously attached handle.
- Update handles and result retrieval that map to Temporal update lifecycle
  semantics.
- Cancel and terminate helpers for generated workflow types.
- Run id access where the underlying SDK exposes it.
- Fill remaining signal/update-with-start input-shape gaps, including Empty
  variants if the runtime facade can stay manageable.
- Consider blocking convenience wrappers only if a real consumer needs them;
  Rust's primary surface should remain async.

Done when:

- Common operational code no longer drops down to raw `temporalio-client` for
  generated workflow types.

## R5 - Runtime Option Coverage

The current generator supports a narrow set of options. Go users expect more
annotation fields to flow through to workflow starts, activity execution, and
updates.

Target capabilities:

- Workflow options: retry policy, search attributes, typed search attributes,
  parent close policy, workflow id conflict policy, wait-for-cancellation,
  eager start, and versioning behavior.
- Activity options: task queue, schedule/start/close/heartbeat timeouts, retry
  policy, cancellation behavior, and local activity settings.
- Update options: update id, default wait stage/policy, and conflict policy
  fields as applicable.
- Service-level defaults that apply consistently across generated client,
  worker, and activity helper surfaces.

Done when:

- Supported option fields have runtime-facade entries, generated tests, and
  docs.
- Unsupported option fields are listed as blocked by a specific SDK gap or
  deferred roadmap item.

## R6 - CLI Execution Surface

Today's `cli=true` output is a clap parser scaffold. The Go plugin offers a
more complete command surface.

Target capabilities:

- `Cli::run(self, &<Service>Client)` or equivalent generated dispatch.
- Start, attach, wait/result, signal, query, update, cancel, and terminate
  commands for generated workflow types.
- JSON input support through a documented prost/serde strategy.
- Respect cludden CLI annotations and field options where practical.

Done when:

- Consumers can expose a generated service CLI without hand matching every
  command variant.

## R7 - Bloblang-Backed Templates

The Go plugin uses Bloblang for workflow ids, update ids, and search
attributes. Rust currently supports only the simple `{{ .Field }}` workflow-id
subset.

Target capabilities:

- Decide whether to embed a Rust Bloblang evaluator or compile supported
  expressions during code generation.
- Support workflow id and update id templates.
- Support search attributes and typed search attributes generated from input
  messages.
- Keep diagnostics precise for unsupported expression features.

Done when:

- Common Bloblang templates accepted by cludden's examples behave the same in
  Rust and Go fixtures.

## R8 - Advanced and Lower-Frequency Go Features

These features matter for eventual majority parity but are lower priority than
worker/activity/client coverage.

Candidates:

- Codec server generation.
- Generated test clients or mocks, gated by Rust SDK support.
- Patch/protopatch handling.

Done when:

- Each subsystem has a dedicated design note explaining the Rust API shape,
  SDK dependencies, and test strategy before implementation starts.

### Explicitly out of scope

The following items appear in cludden's Go plugin but are not pursued by this
Rust plugin. The reasons differ; the common thread is that the cost of
maintaining them in Rust outweighs the value relative to the core
worker/activity/client surface. These do not block "majority parity."

- **XNS helpers (cross-namespace workflow execution).** The annotation schema
  carries `xns` fields on every method ref; this plugin rejects them at parse
  so users see the no-op explicitly. Reviving them would require a parallel
  emit path that targets a different SDK API. Not pursued.
- **Nexus service and operation generation.** Nexus is Temporal's
  cross-service operation API. cludden generates a separate code path for it;
  this plugin does not. Annotated Nexus surfaces should be split into their
  own protos and used directly against the Nexus SDK.
- **Generated Markdown or API documentation.** `docs/RUNTIME-API.md` and
  `docs/SUPPORT-STATUS.md` already document the runtime contract by hand;
  per-service generated docs would duplicate that surface and drift quickly.
  Users should consult the generated Rust source via `cargo doc` for
  per-service detail.
- **Go-specific naming knobs.** Cludden's plugin exposes Go-side flags for
  PascalCase/camelCase overrides, package paths, etc. Rust idioms and the
  proto-driven defaults this plugin already emits cover the same ground;
  these flags would not have a Rust equivalent worth maintaining.

## Current Unsupported Items

This list is not exhaustive. It is the working set to keep visible while moving
toward majority parity.

| Area | Current behavior | Roadmap |
|---|---|---|
| Method co-annotations | Refused at parse with a clear diagnostic (2026-05-13); generator still models one primary kind per rpc. Full support is the next R1 step. | R1 |
| Cross-service refs | Same-service only; fully-qualified refs surface an explicit "cross-service refs are not yet supported" diagnostic at validate (2026-05-13). | R1 |
| Aliases | Workflow aliases emit a module const + Definition associated const (2026-05-13); signal/query/update/activity have no alias field in cludden's schema. | R1 |
| Worker handler surface | Definition trait + registration + child-workflow markers/start + continue-as-new + external-signal markers/helpers shipped 2026-05-13; signal-receive/select helpers, query/update handler hooks still pending. | R2 |
| Activity calls from workflows | `<RPC>Activity` markers + `execute_<activity>` + `execute_<activity>_local` + `<activity>_default_options()` factory shipped 2026-05-13 (non-Empty in/out only); Empty-input/output helpers still pending. | R3 |
| Client cancel/terminate/top-level operations | `cancel_workflow`, `terminate_workflow`, `run_id()`, signal/query/update-by-id all shipped 2026-05-13. | R4 |
| Workflow retry/search/versioning options | `enable_eager_start`, `workflow_id_conflict_policy`, `retry_policy`, `parent_close_policy` shipped 2026-05-13; search attrs (need R7 Bloblang), `wait_for_cancellation` (child-only — needs more design), `versioning_behavior` (worker-side, no SDK 0.4 support) still pending. | R5 |
| Activity runtime options | All six fields graduated to `<activity>_default_options()` 2026-05-13 (incl. `wait_for_cancellation` → `ActivityCancellationType::WaitCancellationCompleted`). | R5/R3 |
| Update ids/default wait stage | All shipped 2026-05-13: `UpdateOptions.id` → `<update>_by_template`; `wait_for_stage` + deprecated `wait_policy` → `Option<WaitPolicy>` with proto-default fold. | R5 |
| CLI command execution | Parser scaffold only. | R6 |
| Bloblang | Only simple `{{ .Field }}` workflow id templates are supported. | R7 |
| Codec server / test clients / patch handling | Not generated. | R8 |
| XNS / Nexus / generated docs / Go-specific naming knobs | Out of scope — see R8 "Explicitly out of scope". | — |

## Verification Gate

Each roadmap phase should include:

- Parser/validation fixtures for the annotation shapes it changes.
- Generated golden output or generated-surface compile tests.
- Runtime facade documentation for every new emitted symbol.
- Job-queue or another realistic example update when the feature is user-facing.
- Wire-format compatibility audit if payload construction or metadata changes.

## Permanent Constraints

- No custom annotation schema.
- No wire-format drift from the Go and TS compatibility contract.
- No generated business logic for workflow or activity bodies.
- No direct dependency from generated code to unstable SDK internals when the
  `crate::temporal_runtime` facade can isolate the dependency.
