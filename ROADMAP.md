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
| R8 | Advanced subsystems | Codec server and test clients — both blocked on upstream SDK 0.4 gaps. (XNS, Nexus, generated docs, Go-specific naming knobs, and Patch/protopatch handling are explicitly out of scope — see R8 below.) |

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
- 2026-05-13 (R7 slice 2 — `double` literals wired through plugin):
  closes the gap deferred in the previous commit: the Bloblang
  slice-2 lexer now recognises f64 literals (tokens with `.` or
  `e`/`E`) and produces a new `SearchAttributeLiteral::Double(f64)`
  variant. Render emits
  `encode_search_attribute_double(<v>f64).expect("compile-time-finite
  f64 literal")` (the bridge encoder returns Result; the `.expect()`
  marks the parse-time finite guarantee). `{:?}` formatting preserves
  the decimal on whole-number values so generated code stays a JSON
  number on the wire. Non-finite literals fall through to the
  standard unsupported-`search_attributes` diagnostic. `SearchAttributesSpec`
  and `SearchAttributeLiteral` drop `Eq` (kept `PartialEq`) since
  f64 can't satisfy `Eq`. One new parse_validate test pins the
  positive emit (fraction + whole-number + scientific notation).
  `docs/SUPPORT-STATUS.md` row updated; double moves out of the
  rejected-scalars list. 131 parse_validate / 26 bridge tests green.
- 2026-05-13 (bridge — `encode_search_attribute_double` + decoder):
  rounds out the search-attribute encoder set (string / int / bool /
  now double). NaN and infinities refused at the encoder boundary
  (neither has a valid JSON literal — silent serialisation would
  drift across languages). Whole-number doubles emit with the
  decimal point preserved (`1.0`, not `1`) so the wire shape stays
  an unambiguous JSON number. Decoder validates `json/plain`
  encoding and refuses non-finite decoded values as a corruption
  guard. Plugin doesn't call them yet — the Bloblang slice 2 parser
  still only recognises string / int / bool literals (would need a
  `SearchAttributeLiteral::Double` variant + lexer extension to wire
  through). Three new bridge unit tests pin round-trip including
  whole-number formatting, NaN/infinity rejection, and non-numeric
  decode rejection. 26 bridge tests. Bridge bumped to 0.1.6 in
  `docs/RUNTIME-API.md`.
- 2026-05-13 (bridge — `encode_proto_payload` / `decode_proto_payload` made public):
  the two bridge helpers that build and validate `binary/protobuf`
  payloads against `WIRE-FORMAT.md` were previously `fn`-private,
  used only by the bridge's own internal call paths. They're now
  `pub fn` so downstream tooling (custom dispatch layers, payload
  migrators, codec servers, payload routers in proxies) can
  construct + validate byte-identical payloads the generated client
  uses without duplicating the metadata-triple logic. No behavioural
  change — only visibility. Existing `encode_decode_round_trip`
  bridge test now covers the public surface. `docs/RUNTIME-API.md`
  bumped to 0.1.5.
- 2026-05-13 (bridge — search-attribute decoder helpers):
  the bridge gains `decode_search_attribute_string` /
  `decode_search_attribute_int` / `decode_search_attribute_bool` as
  the inverses of the encoder triple shipped in R7 slice 2. They
  validate the `json/plain` encoding contract (surface a precise
  diagnostic on mis-typed payloads — `binary/protobuf` etc.) and
  round-trip every value the encoders emit, including the minimal
  JSON-escape (`\\` and `\"`) on strings. Plugin doesn't call them
  yet; downstream consumers reading server-supplied search attributes
  use them directly. Six new bridge unit tests pin the round-trip,
  wrong-encoding rejection, non-numeric int rejection, and
  not-`true`/`false` bool rejection. Bridge version bumped to 0.1.4
  in `docs/RUNTIME-API.md`. 23 bridge tests total. No plugin signature
  change.
- 2026-05-13 (R4 — per-workflow attached-handler name consts):
  every workflow that refs at least one signal / query / update via
  `WorkflowOptions.{signal,query,update}[]` now emits per-kind
  `<RPC>_ATTACHED_SIGNAL_NAMES` / `_QUERY_NAMES` / `_UPDATE_NAMES`
  `&'static [&'static str]` consts listing the *registered* (Temporal-
  wire) names of those handlers. Resolves both same-service refs and
  cross-service refs (the cross-service target's `registered_name` is
  captured at parse). Workflows with no attached refs of a given
  kind produce no const for that kind, so workflows with no handler
  attachments stay surface-clean. One new positive parse_validate
  test pins the populated + empty cases. 130 parse_validate tests
  total. Several fixture goldens reblessed (every fixture with
  workflow-attached handlers gained the corresponding const block).
  No bridge signature change.
- 2026-05-13 (R1 — cross-workflow alias collision validation):
  extends the per-workflow alias-collision parse check (previous
  commit) to a service-wide validation pass: two workflows on the
  same service cannot register the same Temporal name via overlapping
  `aliases` or via an alias colliding with another workflow's
  `registered_name`. Either case attempts duplicate registration at
  runtime, so refuse at codegen. New `reject_workflow_alias_collisions_
  across_workflows` runs in `validate.rs` after the per-workflow parse
  check; diagnostics name both offending workflows + the colliding
  literal. Two new positive parse_validate tests pin both collision
  shapes (alias-vs-alias and alias-vs-other-name). 129 parse_validate
  tests total. No bridge signature change; no fixture goldens touched.
- 2026-05-13 (R1 — workflow alias collision validation):
  `(temporal.v1.workflow).aliases` now rejects two real footguns at
  parse: (1) an alias that equals the workflow's own `registered_name`
  (would register the workflow twice under the same Temporal name),
  and (2) the same alias listed more than once within the list (same
  duplicate-registration outcome). Both diagnostics name the
  offending alias literal so the user can fix the proto without
  hunting. Two new parse_validate tests pin both rejections. No
  bridge signature change; no fixture goldens touched (no fixture
  declared a colliding alias).
- 2026-05-13 (R4 — `<Service>Client::SOURCE_FILE` const):
  every generated `<Service>Client` now exposes a `SOURCE_FILE:
  &'static str` const carrying the proto file path exactly as protoc
  saw it. Lets tooling correlate generated code back to its source
  proto without parsing `build.rs` outputs. One new positive
  parse_validate test pins the const shape. 16 fixture goldens
  reblessed (every fixture gained one line in its Client impl). No
  bridge signature change.
- 2026-05-13 (R4 — per-activity `<RPC>_ACTIVITY_TASK_QUEUE` consts):
  every activity that declares `(temporal.v1.activity).task_queue`
  now emits a `pub const <RPC>_ACTIVITY_TASK_QUEUE: &str = …`. Mirrors
  the per-workflow `<RPC>_TASK_QUEUE` const shape. Activities that
  omit the task_queue field produce no const, so existing fixtures
  stay clean (no fixture declares an activity task_queue). One new
  positive parse_validate test pins both the emitted-when-declared
  and omitted-otherwise behaviours. No bridge signature change; no
  fixture goldens touched.
- 2026-05-13 (R4 — `<Service>Client` identity consts: `PACKAGE` / `SERVICE_NAME` / `FULLY_QUALIFIED_SERVICE_NAME`):
  three more `&'static str` consts on the generated client impl,
  carrying the proto namespace identity (e.g. `PACKAGE = "jobs.v1"`,
  `SERVICE_NAME = "JobService"`, `FULLY_QUALIFIED_SERVICE_NAME =
  "jobs.v1.JobService"`). Lets tooling read the proto identity at
  runtime without re-parsing import paths. Always emitted on every
  service (not gated by per-kind list emptiness). One new positive
  parse_validate test pins the const shapes. 16 fixture goldens
  reblessed. No bridge signature change.
- 2026-05-13 (R4 — handler `_INPUT_TYPE` / `_OUTPUT_TYPE` consts for signals/queries/updates/activities):
  extends the previous workflow-only commit to all rpc kinds. Each
  signal emits `<RPC>_SIGNAL_INPUT_TYPE` (signal outputs are always
  Empty so no output const); queries / updates / activities emit
  both `<RPC>_<KIND>_INPUT_TYPE` and `<RPC>_<KIND>_OUTPUT_TYPE`.
  Same const shape (`pub const X: &str = "pkg.Type"`), same
  Empty-handling (canonical `google.protobuf.Empty`). One new
  positive parse_validate test covers all four rpc kinds in one
  service. 6 fixture goldens reblessed (every fixture that has at
  least one signal / query / update / activity gained the
  corresponding const block). No bridge signature change.
- 2026-05-13 (R4 — per-workflow `<RPC>_INPUT_TYPE` / `_OUTPUT_TYPE` consts):
  every workflow rpc now emits two `&str` consts carrying the fully-
  qualified proto type names for its input and output messages. Empty
  sides land as the canonical `"google.protobuf.Empty"` (preserved
  verbatim from `ProtoType.full_name`). Lets consumer tooling
  (codecs, payload routers, cross-language test harnesses) look up
  the proto message name without re-traversing the descriptor pool.
  One new positive parse_validate test pins the typed + both Empty
  variants; 14 fixture goldens reblessed (every fixture with at
  least one workflow gained the two-line const block). No bridge
  signature change.
- 2026-05-13 (R4 — service-level name aggregates on `<Service>Client`):
  the generated `<Service>Client` now exposes five aggregate
  `&'static [&'static str]` consts: `WORKFLOW_NAMES`, `SIGNAL_NAMES`,
  `QUERY_NAMES`, `UPDATE_NAMES`, `ACTIVITY_NAMES`. Each only emits
  when the corresponding model list is non-empty (so a workflow-only
  service doesn't get four empty consts). Lets tooling enumerate
  every registered name without reproducing the snake-case +
  default-name resolution logic the plugin does at codegen. Two new
  parse_validate tests pin the positive emit and the
  empty-omission behaviour. 14 of the 16 fixture goldens reblessed
  (every fixture with at least one workflow gained a const block on
  the Client impl). No bridge signature change.
- 2026-05-13 (R6 — `(temporal.v1.query).cli` + `(temporal.v1.update).cli` method-level honoured):
  parallel of the signal-method-level work. Both `QueryOptions.cli`
  and `UpdateOptions.cli` move from intentionally-ignored to supported.
  Queries have no per-ref `cli` knob (`WorkflowOptions.query[N]`
  carries only `ref` + `xns`), so the method-level annotation is the
  only override path; render's new `query_cli_attrs` threads it into
  the `Query<Name>` clap variant. Updates layer on top of the existing
  per-ref work — `update_ref_cli_attrs` falls back to the method-level
  `UpdateModel.cli_*` fields when no workflow ref carries overrides,
  same precedence as signals. `QueryModel` and `UpdateModel` each gain
  `cli_name` / `cli_aliases` / `cli_usage`; the `fabricate_*` paths
  emit `None`s for cross-service refs. Two new positive parse_validate
  tests pin both override paths. With this commit, every method-level
  CLI override field across signals, queries, and updates is wired
  through. No bridge signature change; no fixture goldens touched.
  118 parse_validate / 17 bridge tests green.
- 2026-05-13 (R6 — `(temporal.v1.signal).cli` method-level fallback honoured):
  the method-level `cli` annotation on signal rpcs moves from
  intentionally-ignored to supported. It acts as the fallback default
  for the `Signal<Name>` CLI subcommand's `#[command(name, alias,
  about)]` when no `WorkflowOptions.signal[N].cli` workflow ref
  carries overrides. Per-ref overrides win when both are set —
  same first-ref-wins policy as before.
  `SignalModel` gains `cli_name` / `cli_aliases` / `cli_usage`;
  `signal_ref_cli_attrs` falls back to those when no workflow override
  is present. Two new positive parse_validate tests: method-level
  fallback emit and ref-wins-over-method-level priority. No bridge
  signature change; no fixture goldens touched. 116 parse_validate /
  17 bridge tests green.
- 2026-05-13 (R6 — `WorkflowOptions.update[N].cli` per-ref overrides honoured):
  parallel of the signal-ref work. The nested `cli` field on update
  refs moves from rejected to supported. `UpdateRef` gains
  `cli_name` / `cli_aliases` / `cli_usage`; render's new
  `update_ref_cli_attrs` helper picks the first workflow ref
  carrying overrides and threads them into the `Update<Name>` clap
  variant as `#[command(name = "update-<name>", alias = [...],
  about = …)]`. The existing rejection test
  `workflow_update_ref_with_cli_is_rejected_at_parse` was flipped to
  a positive emit assertion (`...threads_into_subcommand`). No bridge
  signature change; no fixture goldens touched (no fixture uses
  update-ref cli overrides). 114 parse_validate / 17 bridge / all
  other tests green.
- 2026-05-13 (R6 — `WorkflowOptions.signal[N].cli` per-ref overrides honoured):
  the nested `cli` field on signal refs moves from rejected to
  supported. `SignalRef` gains `cli_name` / `cli_aliases` /
  `cli_usage`; render's `signal_ref_cli_attrs` helper picks the first
  workflow ref carrying overrides for a given signal (service-scoped
  CLI emit means multiple workflows can't disagree usefully) and
  threads them into the `Signal<Name>` clap variant as
  `#[command(name = "signal-<name>", alias = [...], about = …)]`.
  The Signal[].cli diagnostic-coverage case in
  `unsupported_field_support_status_table` is gone since the field is
  no longer rejected; one new positive test pins the override emit.
  No bridge signature change; no fixture goldens touched (no fixture
  uses signal-ref cli overrides).
- 2026-05-13 (R1 — cross-service with-start free fns emit):
  `render_with_start_functions` previously dropped cross-service
  refs silently when looking up the SignalModel / UpdateModel from
  `svc.signals` / `svc.updates`. It now falls back to
  `fabricate_signal_model` / `fabricate_update_model` (the same
  fabricate path the typed-handle-method emit already uses for
  cross-service refs), so a workflow can attach a cross-service signal
  with `start: true` and get a `<signal>_with_start` free function
  that calls into the bridge with the cross-service target's input /
  registered name. Three new positive parse_validate tests:
  cross-service signal-with-start emit, cross-service update-ref
  handle method, cross-service query-ref handle method. The
  SUPPORT-STATUS row for `signal[]/query[]/update[]` was rewritten
  to reflect cross-service-supported state (the row had said "rejected
  by validate.rs::check_ref" which was stale since the mid-session
  cross-service ref work landed). No bridge signature change; no
  fixture goldens touched.
- 2026-05-13 (R5 — per-update `workflow_id_conflict_policy` honoured):
  the nested `WorkflowOptions.update[].workflow_id_conflict_policy`
  field on update refs moves from rejected to supported. The bridge's
  `update_with_start_workflow_proto[_unit]` fns grow a trailing
  `id_conflict_policy: Option<WorkflowIdConflictPolicy>` arg; `None`
  keeps the historical `UseExisting` default in place, `Some(...)`
  honours the proto override. `UpdateRef` gains the new field; render
  threads it through the `<update>_with_start` free function body.
  The existing rejection test was flipped to a positive emit assertion
  that pins both the model state and the rendered
  `Some(temporal_runtime::WorkflowIdConflictPolicy::<Variant>)` arg.
  Stub runtime in `generated_surface_compile.rs` updated for the new
  arg. Two fixture goldens (`empty_output_query_update`,
  `full_workflow`) reblessed. Stale `retry_policy | rejected | R5`
  row removed from SUPPORT-STATUS — that row contradicted the
  already-supported entry two lines below.
- 2026-05-13 (R6 — service-level `(temporal.v1.cli)` honoured):
  cludden's plugin uses a distinct extension `(temporal.v1.cli)`
  (separate from `(temporal.v1.service)`) to configure the top-level
  CLI binary. We previously read the workflow-level `cli` block but
  silently dropped the service-level one. Now every nested field
  threads through: `ignore = true` suppresses the entire CLI module
  (overriding the per-workflow `cli.ignore` heuristic); `name` /
  `usage` / `aliases` override the `Cli` struct's
  `#[command(name = …, about = …, alias = […])]` attributes (default
  fallbacks: service name in snake_case, `"Generated Temporal CLI for
  <pkg>.<Svc>"`, no aliases). New `ServiceCliSpec` on `ServiceModel`;
  new `SERVICE_CLI_EXT = "temporal.v1.cli"` extension lookup wired
  into `ExtensionSet`. Two new inline parse_validate tests: positive
  override emit, and ignore-suppresses-module. No bridge signature
  change; no fixture goldens touched (no fixture uses the annotation,
  and the unannotated render still produces byte-identical output
  via the default fallbacks).
- 2026-05-13 (R6 — `--wait` prints the typed workflow result):
  the generated CLI's `Start<Wf>` and `Attach<Wf>` variants previously
  discarded the workflow result when `--wait` was set (`let _ =
  handle.result().await?;`). They now bind it and debug-print
  (`result={:?}`), matching the print pattern queries / updates / start
  attach the typed output to. This is the smallest UX-correctness fix
  left — waiting and then silently discarding was a footgun for users
  driving long-running workflows from the shell. The existing
  `cli_emit_renders_run_with_dispatch` test's `--wait` assertion was
  tightened to check for the typed print. `cli_emit` and `cli_ignore`
  golden fixtures reblessed. No bridge signature change.
- 2026-05-13 (R6 — `update-<name>` CLI subcommands per update rpc):
  every `(temporal.v1.update)` rpc on a service now gains a clap
  `Update<Name>(Update<Name>Args)` variant in the `cli=true` scaffold.
  Empty-input updates carry only the positional `workflow_id`;
  non-Empty updates add the `--input-file` prost-json flag pattern.
  Dispatch in `Cli::run_with` calls `client.<update>(workflow_id,
  input?, None)` so the proto-declared default wait policy applies,
  and debug-prints the typed output. With signal + query + update CLI
  all shipped, the CLI now mirrors every same-service handler rpc the
  workflow declares. One new inline parse_validate test pins both
  variants, both Args structs, the input_file gating by Empty input,
  and both dispatch shapes. No bridge signature change; no fixture
  goldens touched.
- 2026-05-13 (R6 — `query-<name>` CLI subcommands per query rpc):
  every `(temporal.v1.query)` rpc on a service now gains a clap
  `Query<Name>(Query<Name>Args)` variant in the `cli=true` scaffold.
  Empty-input queries carry only the positional `workflow_id`;
  non-Empty queries add the `--input-file` prost-json flag pattern.
  Dispatch in `Cli::run_with` calls into the existing client-level
  `<query>(workflow_id, input)` method and debug-prints the typed
  output (`result={:?}`). One new inline parse_validate test pins:
  both variants, both Args structs, the input_file gating by Empty,
  both dispatch shapes, and the debug-print path. No bridge signature
  change; no fixture goldens touched (no fixture combines `cli=true`
  with queries).
- 2026-05-13 (R6 — `signal-<name>` CLI subcommands per signal rpc):
  every `(temporal.v1.signal)` rpc on a service now gains a clap
  `Signal<Name>(Signal<Name>Args)` variant in the `cli=true` scaffold.
  Empty-input signals carry only the positional `workflow_id`;
  non-Empty signals add the same `--input-file` prost-json pattern
  used by workflow starts. Dispatch in `Cli::run_with` calls into the
  existing client-level `<signal>(workflow_id, input)` method (or the
  Empty-input overload). One new inline parse_validate test pins:
  both variants, both Args structs, the input_file gating by Empty,
  and both dispatch shapes. No bridge signature change; no fixture
  goldens touched (no fixture combines `cli=true` with signals).
- 2026-05-13 (R6 — `cancel-<wf>` + `terminate-<wf>` CLI subcommands):
  the `cli=true` scaffold gains two new variants per workflow:
  `Cancel<Wf>(Cancel<Wf>Args)` and `Terminate<Wf>(Terminate<Wf>Args)`,
  each carrying a positional `workflow_id` and a `--reason` flag
  (defaults to empty string). Dispatch in `Cli::run_with` calls into
  the existing `Handle::cancel_workflow(&reason)` /
  `terminate_workflow(&reason)` methods, so the wire surface is just
  the bridge calls the scaffold already exposed on the typed handle.
  Per-workflow `cli.name` / `cli.aliases` / `cli.usage` overrides now
  apply uniformly across all four verbs (start / attach / cancel /
  terminate). One new positive parse_validate test pins the variants,
  args, default flag, and dispatch lines. The `cli.usage` test
  upgraded its occurrence count from 2 to 4 since the override now
  reaches the new variants too. `cli_emit` and `cli_ignore` golden
  fixtures reblessed.
- 2026-05-13 (R6 — `cli.usage` per-workflow honoured):
  `(temporal.v1.workflow).cli.usage` moves from rejected to supported.
  Emits as `#[command(about = "<usage>")]` on both the start and attach
  subcommand variants, overriding clap's docstring-derived default.
  Completes the `WorkflowOptions.cli` block — every nested field
  (`ignore`, `name`, `usage`, `aliases`) now threads through to the
  generated CLI. The existing `cli.usage` rejection test was rewritten
  as a positive emit test asserting the attribute lands twice (one per
  variant). The dead `reject_unsupported_workflow_cli_options` helper
  is gone; no other call sites referenced it. No bridge signature
  change; no fixture goldens touched (no fixture sets `cli.usage`).
- 2026-05-13 (R6 — `cli.name` + `cli.aliases` per-workflow honoured):
  `(temporal.v1.workflow).cli.name` and `cli.aliases` move from
  rejected to supported. The CLI emit threads them into the per-workflow
  `Start<Wf>` and `Attach<Wf>` subcommand variants as
  `#[command(name = "start-<override>", alias = ["start-<a>", …])]` /
  `#[command(name = "attach-<override>", alias = ["attach-<a>", …])]`
  so the rename + aliases apply uniformly to both verbs. `cli.usage`
  (help text override) still stays rejected — emitting it requires
  rewriting the per-variant docstring path. Two new parse_validate
  tests: positive override emit (start + attach in lock-step), and the
  surviving `cli.usage` rejection. No bridge signature change; no
  fixture goldens touched (no fixture sets `cli.name` or `cli.aliases`,
  and the codegen emits attributes only when at least one is present).
  Existing `workflow_cli_name_is_rejected` test was rewritten as the
  positive emit test since the rejection no longer holds.
- 2026-05-13 (R7 — slice 3b lands: `this.<field>` for int64 + bool):
  the field-ref support graduated from strings-only to also cover
  singular `int64` and `bool` input fields. `SearchAttributeLiteral`
  picks up `IntField` / `BoolField` variants alongside `StringField`,
  the parser routes the per-`prost_reflect::Kind` mapping, and render
  emits `encode_search_attribute_int(input.<field>)` /
  `encode_search_attribute_bool(input.<field>)`. Other scalar types
  (`int32`, `uint64`, `float`, `double`, `bytes`, enums) and any
  repeated / map / message field still fall through to the standard
  unsupported-`search_attributes` diagnostic — the bridge encoders
  are scalar-only and the matrix stays in lock-step. Three new
  parse_validate tests: positive int + bool emit, unsupported scalar
  type rejection, repeated-field rejection. The slice-3a "non-string
  rejected" test was replaced by the slice-3b "non-int/bool rejected"
  one. No bridge signature change; no fixture goldens touched. The
  remaining R7 work is `typed_search_attributes` (slice 3c — needs the
  `SearchAttributeKey<T>` surface from `temporalio-common`).
- 2026-05-13 (R7 — slice 3a lands: `this.<field>` for strings):
  `(temporal.v1.workflow).search_attributes` Bloblang expressions of
  the form `root = { "K": this.<field>, … }` now resolve at parse time
  against the workflow's input message descriptor and emit per-entry
  `encode_search_attribute_string(input.<field>.as_str())` calls in
  the start path body. Scope locked to singular `string` fields for
  this cut — int / bool / repeated / message refs and richer Bloblang
  remain refused with the standard unsupported-`search_attributes`
  diagnostic so the encoder coverage stays in lock-step with what the
  bridge offers. Slice 3b (non-string field refs +
  `typed_search_attributes`) is the remaining R7 work. Three new
  parse_validate tests pin: the positive emit + snake-case mapping,
  the missing-field rejection, and the non-string-field rejection.
  No bridge signature change, no fixture goldens touched (each
  fixture either declares no `search_attributes` or stays on slice 2
  primitives).
- 2026-05-13 (R7 — slice 2 lands end-to-end): literal-map
  search-attribute Bloblang expressions now flow from proto to wire.
  `(temporal.v1.workflow).search_attributes = "root = { \"Env\":
  \"prod\", \"Priority\": 5, \"Critical\": true }"` parses into
  `SearchAttributesSpec::Static(Vec<(String, Literal)>)`, render emits
  a `HashMap<String, Payload>` construction at the start path that
  calls the bridge's `encode_search_attribute_*` helpers, and the
  bridge's `start_workflow_proto` / `_empty` thread the option through
  to `WorkflowStartOptions.search_attributes`. Supported value types:
  string literals, signed integers, booleans. Field references
  (`this.<field>`) and richer expressions stay rejected for slice 3.
  Bridge re-exports `Payload as ProtoPayload` so generated code spells
  the map value type by name. New positive test pins the model state,
  each literal type's encoder invocation, and the `Some(HashMap)` flow
  into the bridge call. All 16 fixture goldens reblessed (every
  workflow start path gained a `let search_attributes = …;` line).
- 2026-05-13 (R7 — slice-2 bridge primitives): the bridge now exposes
  `encode_search_attribute_string(&str)`,
  `encode_search_attribute_int(i64)`, and
  `encode_search_attribute_bool(bool)` helpers. They build the
  `json/plain`-encoded `Payload` triples Temporal expects for static
  search attributes. The plugin doesn't call them yet (slice 1 only
  models the empty map); slice 2 emit will route literal map entries
  through these. Pure addition — no signature changes, no goldens
  reblessed. Four new bridge unit tests pin the encoding shapes.
- 2026-05-13 (R7 — slice 1 lands): the canonical empty-map Bloblang
  expression (`root = {}`, whitespace-tolerant) is accepted at parse
  and stored on the model as `SearchAttributesSpec::Empty`. Runtime
  emit treats Empty as a no-op — semantically equivalent to declaring
  no search attributes, which faithfully implements the proto's stated
  intent. Richer expressions (field references, non-empty literals,
  typed search attrs) remain rejected with the standard "does not yet
  honour" diagnostic; slices 2 and 3 land them per the design note.
  Three new tests pin the accepted form, whitespace tolerance, and
  the still-rejected boundary.
- 2026-05-13 (R7 — design note): published `docs/R7-BLOBLANG.md` —
  the pre-implementation design note that captures the SDK contract
  (`WorkflowStartOptions.search_attributes: Option<HashMap<String, Payload>>`
  takes a pre-built map, no Bloblang interpreter), a proposed
  minimum-viable subset (literal map → field references → typed search
  attributes, three independently shippable slices), and the per-slice
  test strategy. Lets a future contributor scope an R7 PR to one slice
  without reading the Bloblang spec end-to-end.
- 2026-05-13 (R1 — full cross-service ref emit): well-formed dotted
  refs (`xs.v1.OtherService.Cancel`) now produce typed Handle methods
  on the parent workflow. Parse-time resolution captures the target's
  registered name + input/output types into a new
  `CrossServiceTarget` field on the ref; validate stops rejecting
  resolved refs; render fabricates a transient `SignalModel` /
  `QueryModel` / `UpdateModel` from the target metadata and feeds it
  into the existing per-handler render fns. Method-snake-case derives
  from the last `.`-segment of the dotted ref so the generated method
  stays short (`handle.cancel(...)` rather than
  `handle.other_v1_other_cancel(...)`). The wire-format registered
  name still points at the cross-service target so the SDK routes to
  the right workflow. Old "cross-service refs are not yet supported"
  rejection test deleted, replaced by a positive
  `cross_service_signal_ref_emits_handle_method` test. Closes R1's
  cross-service ref work.
- 2026-05-13 (R2 — per-handler typed I/O aliases): under `workflows=true`,
  every non-Empty signal input, non-Empty query input/output, and
  non-Empty update input/output now ships a `pub type
  <Rpc>{Signal,Query,Update}{Input,Output} = <prost message>` alias.
  Lets workflow body code spell handler types by role
  (`CancelSignalInput`, `StatusQueryOutput`, …) instead of repeating
  the proto message names — matches one of R2's "generated typed names
  and input/output structs where Go exposes them" deliverables.
  Empty sides are skipped (aliasing `()` adds no value). Two new
  tests; existing fixtures with handler aliases regen their goldens.
- 2026-05-13 (R1 — cross-service ref parse-time resolution): dotted
  refs (`xs.v1.OtherService.Cancel`) now resolve through the
  DescriptorPool at parse. Typos produce
  "doesn't resolve to any rpc in the descriptor pool" with the offending
  ref echoed; targets that resolve but lack the expected
  `(temporal.v1.{signal,query,update})` annotation produce a
  wrong-kind diagnostic. validate.rs's "cross-service refs are not yet
  supported" rejection still fires for well-formed cross-service refs —
  full emit support remains the last R1 step. Two new positive tests
  cover the typo and wrong-kind paths.
- 2026-05-13 (R1 — co-annotation support): the rejection diagnostic
  relaxes into actual support for the combinations cludden's Go plugin
  permits. `parse.rs::method_kinds` (formerly `method_kind`) now returns
  *all* `temporal.v1.*` extensions declared on a single rpc, and
  `parse_service` pushes the method into every relevant model bucket.
  Activity emit lives in a separate trait surface that doesn't share
  symbols with the client / handler emit, so `activity` may co-occur
  with `workflow`, `signal`, or `update`. Combinations involving two
  *primary* kinds (workflow + signal, etc.) remain refused — they would
  collide on generated symbols. `validate.rs::reject_rpc_collisions`
  reworked to allow the same method name in multiple buckets when at
  most one is non-activity. Three new tests cover workflow+activity,
  signal+activity, and the still-rejected two-primary path. Old
  three-case rejection test deleted.
- 2026-05-13 (R3 — Empty-side activity markers + helpers): Empty-input
  and Empty-output activities now also ship per-rpc markers + execute
  helpers. New `temporal_runtime::ProtoEmpty` (a real prost message
  defined in `temporal-proto-runtime`) carries the Empty side so
  `TypedProtoMessage<ProtoEmpty>` satisfies the SDK's serializable
  bounds. Helper signatures hide the wrapper: Empty-input helpers omit
  the `input` arg (construct `ProtoEmpty {}` internally), Empty-output
  helpers return `()` (discard the typed wrapper after the await).
  Closes the last R3 gap. `<activity>_default_options()` factory still
  emits only when the proto declares a close-timeout (orthogonal to
  Empty-side support).
- 2026-05-13 (R6 — `Cli::run_with` dispatch): under `cli=true`, every
  generated `<service>_cli::Cli` now also ships
  `pub async fn run_with<F, Fut>(self, client, mut read_input: F) ->
  Result<(), Box<dyn Error + Send + Sync>>` where
  `F: FnMut(&Path, &'static str) -> Future<Output = Result<Box<dyn Any + Send>, …>>`.
  The closure is the consumer-supplied deserializer — it decides
  JSON / pbjson / raw prost bytes / etc. and returns a type-erased
  `Box<dyn Any>` so heterogeneous workflow inputs work from one
  closure. Each `Start<Wf>` arm downcasts to the typed input and
  forwards to `<Service>Client::<rpc>(input, opts)`; `Attach<Wf>`
  arms use `<rpc>_handle(workflow_id)`. `--wait` is honoured.
  Empty-input workflows bypass the closure entirely. With this,
  `cli=true` is finally a functional CLI instead of a parser
  scaffold. Closes R6.
- 2026-05-13 (R5 — workflow `wait_for_cancellation`): graduates from
  rejected to supported. `(temporal.v1.workflow).wait_for_cancellation = true`
  folds into the per-workflow `<rpc>_default_child_options()` factory as
  `cancel_type: ChildWorkflowCancellationType::WaitCancellationCompleted`.
  `false` (default) leaves the SDK's default `Abandon` in place.
  Bridge re-exports `ChildWorkflowCancellationType` from
  `temporalio_common::protos::coresdk::child_workflow`. Factory now
  emits when *either* `parent_close_policy` or `wait_for_cancellation`
  is declared; both setters compose. Two new tests pin the alone-and-
  combined paths; support-status drift table loses another row.
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

Shipped (2026-05-13):

- Per-workflow `<Workflow>Definition` trait + registration helper
  (`workflows=true`).
- `<RPC>Workflow` markers + `WorkflowDefinition` impl + typed
  `start_<workflow>_child` helper.
- `continue_<workflow>_as_new` helper.
- `<RPC>Signal` markers + `signal_<rpc>_external` workflow-side helper.
- Per-handler I/O type aliases (`<Rpc>SignalInput`, `<Rpc>QueryOutput`,
  …) so workflow body code spells handler types by role.

Blocked on upstream SDK shape:

- **Signal-receive / select helpers** — the SDK only exposes signal
  *sending* (`StartedChildWorkflow::signal`, `ExternalWorkflowHandle::signal`).
  There's no `WorkflowContext::signal_channel()` analog for the body to
  receive on; signals reach the workflow through the
  `#[workflow_methods]` macro's generated dispatch, which owns the
  channel layer. A typed receive helper would either need the SDK to
  publish a `signal_channel<S>()` surface or this plugin would have to
  ship a parallel macro that duplicates the SDK's dispatch — out of
  scope for the v1 emit.
- **Query / update handler hooks** — same constraint. The SDK macro
  generates the handler dispatch from method attributes on the
  consumer's struct. The plugin's emit has no clean place to inject a
  typed hook without conflicting with the macro.

Re-evaluate these when `temporalio-sdk` exposes a public channel /
hook API.

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

Design note: [`docs/R7-BLOBLANG.md`](docs/R7-BLOBLANG.md) (2026-05-13)
captures the SDK contract, proposed minimum-viable subset, three-slice
implementation strategy, and per-slice test plan. A contributor picking
up R7 starts from that document rather than from scratch.

Done when:

- Common Bloblang templates accepted by cludden's examples behave the same in
  Rust and Go fixtures.

## R8 - Advanced and Lower-Frequency Go Features

These features matter for eventual majority parity but are lower priority than
worker/activity/client coverage. Both remaining candidates are **blocked on
upstream Rust SDK gaps** (see `docs/sdk-shape-worker.md`) — neither has a
clean Rust shape until those gaps close.

Candidates (blocked):

- **Codec server generation.** No Rust SDK surface to target; the codec-server
  pattern is a separate Go service today.
- **Generated test clients or mocks.** `temporalio-sdk` 0.4 does not expose a
  `TestWorkflowEnvironment` equivalent. Blocked until the upstream SDK
  publishes a stable test environment, or until this project explicitly
  accepts owning a separate test-harness facade.

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
- **Patch / protopatch handling.** cludden's `Patch` annotation
  (`PV_64` + `PVM_*` modes) controls how the Go plugin stages fix-version
  migrations for inline Bloblang expression evaluation. The Rust plugin
  compiles templates at codegen time, so the inline-eval pattern doesn't
  exist and there's no equivalent staging concern. The `patches` proto
  field is rejected at parse so users see the no-op explicitly.

## Current Unsupported Items

This list is not exhaustive. It is the working set to keep visible while moving
toward majority parity.

| Area | Current behavior | Roadmap |
|---|---|---|
| Method co-annotations | Shipped 2026-05-13: `activity` co-occurs with `workflow` / `signal` / `update`; both buckets populate. Two-primary combinations (workflow+signal etc.) still refused because their generated symbols would collide. | R1 |
| Cross-service refs | Shipped 2026-05-13: parse-time resolution captures target metadata on the ref, render emits typed Handle methods using the cross-service registered name and proto I/O types. | R1 |
| Aliases | Workflow aliases emit a module const + Definition associated const (2026-05-13); signal/query/update/activity have no alias field in cludden's schema. | R1 |
| Worker handler surface | Definition trait + registration + child-workflow markers/start + continue-as-new + external-signal markers/helpers + per-handler I/O type aliases shipped 2026-05-13. Signal-receive/select helpers and query/update handler hooks are blocked on the SDK macro shape — see R2 "Blocked on upstream SDK shape". | R2 |
| Activity calls from workflows | `<RPC>Activity` markers + `execute_<activity>` + `execute_<activity>_local` + `<activity>_default_options()` shipped 2026-05-13. Empty-input/output sides supported via `temporal_runtime::ProtoEmpty` wrapping; helper signatures hide the wrapper (no input arg for Empty-input, `()` return for Empty-output). | R3 |
| Client cancel/terminate/top-level operations | `cancel_workflow`, `terminate_workflow`, `run_id()`, signal/query/update-by-id all shipped 2026-05-13. | R4 |
| Workflow retry/search/versioning options | `enable_eager_start`, `workflow_id_conflict_policy`, `retry_policy`, `parent_close_policy`, `wait_for_cancellation` shipped 2026-05-13; search attrs (need R7 Bloblang) and `versioning_behavior` (worker-side, no SDK 0.4 support) still pending. | R5 |
| Activity runtime options | All six fields graduated to `<activity>_default_options()` 2026-05-13 (incl. `wait_for_cancellation` → `ActivityCancellationType::WaitCancellationCompleted`). | R5/R3 |
| Update ids/default wait stage | All shipped 2026-05-13: `UpdateOptions.id` → `<update>_by_template`; `wait_for_stage` + deprecated `wait_policy` → `Option<WaitPolicy>` with proto-default fold. | R5 |
| CLI command execution | `Cli::run_with(&Client, deserialize_fn)` dispatch shipped 2026-05-13 (closure-based decoder keeps JSON-vs-pbjson choice with the consumer). | R6 |
| Bloblang | Only simple `{{ .Field }}` workflow id templates are supported. | R7 |
| Codec server / test clients | Not generated; blocked on upstream SDK 0.4 gaps. | R8 |
| XNS / Nexus / generated docs / Go-specific naming knobs / Patch handling | Out of scope — see R8 "Explicitly out of scope". | — |

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
