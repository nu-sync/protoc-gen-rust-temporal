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
| R8 | Advanced subsystems | Nexus, XNS, codec server, docs, test clients, and other lower-frequency Go features. |

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

- XNS helpers.
- Nexus service and operation generation.
- Codec server generation.
- Generated Markdown or API docs.
- Generated test clients or mocks, gated by Rust SDK support.
- Patch/protopatch handling.
- Go-specific naming knobs only when they have a Rust equivalent.

Done when:

- Each subsystem has a dedicated design note explaining the Rust API shape,
  SDK dependencies, and test strategy before implementation starts.

## Current Unsupported Items

This list is not exhaustive. It is the working set to keep visible while moving
toward majority parity.

| Area | Current behavior | Roadmap |
|---|---|---|
| Method co-annotations | Refused at parse with a clear diagnostic (2026-05-13); generator still models one primary kind per rpc. Full support is the next R1 step. | R1 |
| Cross-service refs | Same-service only; fully-qualified refs surface an explicit "cross-service refs are not yet supported" diagnostic at validate (2026-05-13). | R1 |
| Aliases | Workflow aliases emit a module const + Definition associated const (2026-05-13); signal/query/update/activity have no alias field in cludden's schema. | R1 |
| Worker handler surface | Emits contracts and registration helpers, not handler adapters. | R2 |
| Activity calls from workflows | Not generated. | R3 |
| Client cancel/terminate/top-level operations | Not generated. | R4 |
| Workflow retry/search/versioning options | Rejected or not emitted depending on field. | R5 |
| Activity runtime options | Mostly not emitted. | R5 |
| Update ids/default wait stage | Not fully emitted. | R5 |
| CLI command execution | Parser scaffold only. | R6 |
| Bloblang | Only simple `{{ .Field }}` workflow id templates are supported. | R7 |
| XNS/Nexus/codec/docs/test clients | Not generated. | R8 |

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
