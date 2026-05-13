# Worker emit v1 scope

**Status:** scoped after SDK probe.
**Date:** 2026-05-13.
**Inputs:** `docs/sdk-shape.md`, `docs/sdk-shape-worker.md`, Phase 2 and
Phase 3 spike findings.

## Decision

Ship a conservative worker emit slice:

- `activities=true` emits the existing `<Service>Activities` typed trait,
  activity name constants, and a thin `register_<service>_activities(...)`
  helper.
- `workflows=true` emits one `<Workflow>Definition` trait per workflow rpc,
  handler name constants, and a thin `register_<workflow>_workflow::<W>(...)`
  helper.
- Generated code still routes through `crate::temporal_runtime`.
- Consumers still write the SDK macro-bearing worker bodies:
  `#[activities]` adapters for activities and `#[workflow]` /
  `#[workflow_methods]` impls for workflows.
- `test_client` emit is out of this slice because `temporalio-sdk` 0.4.0 has
  no `TestWorkflowEnvironment` equivalent.

## Why this scope

The Rust SDK does not have Go-style runtime `RegisterWorkflow` /
`RegisterActivity` APIs keyed only by strings. Registration is static and
macro-driven:

- activities need marker structs implementing `ActivityDefinition` and
  `ExecutableActivity`, tied to the consumer's concrete implementer type;
- workflows need macro-generated `WorkflowImplementer` and
  `WorkflowImplementation` impls on the consumer's workflow struct.

The plugin cannot honestly generate those concrete SDK impls from a proto
alone because the consumer-owned Rust types do not exist at codegen time.
The useful v1 boundary is therefore:

1. generated names and typed proto contracts so the consumer cannot drift from
   the proto silently;
2. registration helpers that delegate to SDK implementers after the consumer's
   macro adapter has created those implementers;
3. no generated workflow bodies and no hidden test harness.

## Out of v1

- Worker construction helpers beyond `register_*` functions.
- Worker options projection, interceptors, worker versioning, Nexus, XNS, codec,
  generated docs, and CLI dispatch.
- Declarative macros emitted by this plugin to replace `temporalio-sdk` macros.
- Test-client or time-skipping wrapper generation.

## Acceptance mapping

This scope satisfies the worker-side requirement by emitting:

- activity trait + `register_<service>_activities(...)`;
- workflow definition trait + `register_<workflow>_workflow::<W>(...)`;
- constants for workflow, task queue, signal, query, update, and activity names;
- runtime API documentation for the additional worker facade symbols.

It intentionally does not satisfy the conditional test-client deliverable,
because the SDK probe found no upstream test environment to wrap.
