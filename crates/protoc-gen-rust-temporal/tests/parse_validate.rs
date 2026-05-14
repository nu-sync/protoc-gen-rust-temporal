//! Integration tests for the `parse + validate` pipeline.
//!
//! Each test invokes the real `protoc` binary (the same one `prost-build`
//! uses at workspace build time) against a fixture `.proto`, then feeds the
//! resulting `FileDescriptorSet` into a `DescriptorPool` and runs the
//! plugin's parse + validate stages. That mirrors what the plugin sees in
//! production when `protoc` invokes it as a child process.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use prost_reflect::DescriptorPool;

use protoc_gen_rust_temporal::model::ServiceModel;
use protoc_gen_rust_temporal::{parse, render, validate};

const ANNOTATIONS_DIR: &str = "proto";

fn fixture_path(name: &str) -> PathBuf {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_root.join("tests").join("fixtures").join(name)
}

fn protoc_binary() -> PathBuf {
    if let Ok(p) = std::env::var("PROTOC") {
        return PathBuf::from(p);
    }
    PathBuf::from("protoc")
}

/// Compile `proto_root/input.proto` with cludden's schema reachable on the
/// import path, returning a descriptor pool plus the
/// `files_to_generate` set.
fn compile_fixture_at(proto_root: &Path, file: &str) -> (DescriptorPool, HashSet<String>) {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let annotations = crate_root.join(ANNOTATIONS_DIR);

    let tmp = tempfile::tempdir().expect("tempdir");
    let fds_path = tmp.path().join("out.fds");

    let status = Command::new(protoc_binary())
        .arg(format!("-I{}", proto_root.display()))
        .arg(format!("-I{}", annotations.display()))
        .arg(format!("--descriptor_set_out={}", fds_path.display()))
        .arg("--include_imports")
        .arg(file)
        .status()
        .expect("invoke protoc — install protoc or set $PROTOC");
    assert!(status.success(), "protoc failed: {status}");

    let bytes = std::fs::read(&fds_path).expect("read fds");
    let mut pool = DescriptorPool::new();
    pool.decode_file_descriptor_set(bytes.as_slice())
        .expect("decode_file_descriptor_set");

    let files_to_generate: HashSet<String> = std::iter::once(file.to_string()).collect();
    (pool, files_to_generate)
}

fn compile_fixture(name: &str) -> (DescriptorPool, HashSet<String>) {
    compile_fixture_at(&fixture_path(name), "input.proto")
}

/// Drop `source` into a temp dir as `input.proto` and run `protoc` against
/// it. Used by the negative tests.
fn compile_fixture_inline(source: &str) -> (DescriptorPool, HashSet<String>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(tmp.path().join("input.proto"), source).expect("write input.proto");
    let (pool, files_to_generate) = compile_fixture_at(tmp.path(), "input.proto");
    (pool, files_to_generate, tmp)
}

fn parse_and_validate(name: &str) -> Vec<ServiceModel> {
    let (pool, files_to_generate) = compile_fixture(name);
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let options = protoc_gen_rust_temporal::options::RenderOptions::default();
    for service in &services {
        validate::validate(service, &options).expect("validate");
    }
    services
}

fn load_fixture_options(name: &str) -> protoc_gen_rust_temporal::options::RenderOptions {
    let p = fixture_path(name).join("options.txt");
    if !p.exists() {
        return protoc_gen_rust_temporal::options::RenderOptions::default();
    }
    let s = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    protoc_gen_rust_temporal::options::parse_options(s.trim())
        .unwrap_or_else(|e| panic!("parse {}: {e}", p.display()))
}

#[test]
fn minimal_workflow_parses_and_validates() {
    let services = parse_and_validate("minimal_workflow");
    assert_eq!(services.len(), 1);
    let svc = &services[0];

    assert_eq!(svc.package, "jobs.v1");
    assert_eq!(svc.service, "JobService");
    assert_eq!(svc.default_task_queue.as_deref(), Some("jobs"));

    assert_eq!(svc.workflows.len(), 1);
    let wf = &svc.workflows[0];
    assert_eq!(wf.rpc_method, "RunJob");
    assert_eq!(wf.registered_name, "jobs.v1.JobService.RunJob");
    assert_eq!(wf.input_type.full_name, "jobs.v1.JobInput");
    assert_eq!(wf.output_type.full_name, "jobs.v1.JobOutput");
    {
        use protoc_gen_rust_temporal::model::IdTemplateSegment;
        let segments = wf.id_expression.as_deref().expect("id template parsed");
        assert_eq!(
            segments,
            &[IdTemplateSegment::Field("name".to_string())],
            "minimal_workflow's `{{{{ .Name }}}}` template should compile to a single Field segment"
        );
    }
    assert!(wf.id_reuse_policy.is_none());

    assert_eq!(wf.attached_signals.len(), 1);
    assert_eq!(wf.attached_signals[0].rpc_method, "CancelJob");
    assert!(wf.attached_signals[0].start);

    assert_eq!(wf.attached_queries.len(), 1);
    assert_eq!(wf.attached_queries[0].rpc_method, "GetStatus");

    assert_eq!(wf.attached_updates.len(), 1);
    assert_eq!(wf.attached_updates[0].rpc_method, "Reconfigure");

    assert_eq!(svc.signals.len(), 1);
    assert_eq!(svc.signals[0].rpc_method, "CancelJob");
    assert!(svc.signals[0].output_type.is_empty);

    assert_eq!(svc.queries.len(), 1);
    assert_eq!(svc.queries[0].rpc_method, "GetStatus");
    assert!(svc.queries[0].input_type.is_empty);

    assert_eq!(svc.updates.len(), 1);
    assert_eq!(svc.updates[0].rpc_method, "Reconfigure");
    assert!(svc.updates[0].validate);

    assert_eq!(svc.activities.len(), 1);
    assert_eq!(svc.activities[0].rpc_method, "ProcessChunk");
}

// Regression guard for cross-language interop with cludden's Go plugin.
// Default registration name must be the fully-qualified proto method name
// (`<package>.<Service>.<Rpc>`), matching the Go plugin's
// `string(method.Desc.FullName())` default — not the bare rpc name and
// *not* the `<package>.<Service>/<Rpc>` slash form. Mixed-language workers
// where one side has an explicit `name:` and the other relies on the
// default would silently never connect otherwise.
#[test]
fn default_registration_names_match_go_full_name() {
    let services = parse_and_validate("minimal_workflow");
    let svc = &services[0];
    assert_eq!(
        svc.workflows[0].registered_name,
        "jobs.v1.JobService.RunJob"
    );
    assert_eq!(
        svc.signals[0].registered_name,
        "jobs.v1.JobService.CancelJob"
    );
    assert_eq!(
        svc.queries[0].registered_name,
        "jobs.v1.JobService.GetStatus"
    );
    assert_eq!(
        svc.updates[0].registered_name,
        "jobs.v1.JobService.Reconfigure"
    );
    assert_eq!(
        svc.activities[0].registered_name,
        "jobs.v1.JobService.ProcessChunk"
    );
}

#[test]
fn explicit_name_overrides_default() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package ex.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              name: "custom.workflow.name"
              signal: [{ ref: "Cancel" }]
            };
          }
          rpc Cancel(In) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = { name: "custom.signal.name" };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let services = protoc_gen_rust_temporal::parse::parse(&pool, &files_to_generate)
        .expect("parse should succeed");
    let svc = &services[0];
    assert_eq!(svc.workflows[0].registered_name, "custom.workflow.name");
    assert_eq!(svc.signals[0].registered_name, "custom.signal.name");
}

#[test]
fn workflow_with_bad_signal_ref_fails_validation() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "NoSuchSignal" }]
            };
          }
        }
        message In {}
        message Out {}
        "#,
    );

    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let err = validate::validate(&services[0], &Default::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("NoSuchSignal"),
        "validation error should name the missing ref, got: {err}"
    );
}

#[test]
fn workflow_signal_ref_with_xns_is_rejected_at_parse() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Tick", xns: { task_queue: "other" } }]
            };
          }
          rpc Tick(In) returns (In) {
            option (temporal.v1.signal) = {};
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .expect_err("xns on signal ref must be rejected at parse")
        .to_string();
    assert!(
        err.contains("xns") && err.contains("signal[ref=Tick]"),
        "parse error must surface xns + signal ref name, got: {err}"
    );
}

#[test]
fn workflow_update_ref_with_cli_threads_into_subcommand() {
    // R6 — `WorkflowOptions.update[N].cli` overrides flow into the
    // `Update<Name>` clap subcommand attributes. Service-scoped CLI
    // emit picks the first workflow ref carrying overrides.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package upd_cli.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{
                ref: "Touch"
                cli: { name: "bump", aliases: ["nudge"], usage: "Bump the run." }
              }]
            };
          }
          rpc Touch(In) returns (Out) {
            option (temporal.v1.update) = {};
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let services =
        parse::parse(&pool, &files_to_generate).expect("update[].cli must parse cleanly");
    let uref = services[0].workflows[0]
        .attached_updates
        .iter()
        .find(|u| u.rpc_method == "Touch")
        .expect("Touch update ref must be in model");
    assert_eq!(uref.cli_name.as_deref(), Some("bump"));
    assert_eq!(uref.cli_aliases, vec!["nudge"]);
    assert_eq!(uref.cli_usage.as_deref(), Some("Bump the run."));

    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            "#[command(name = \"update-bump\", alias = [\"update-nudge\"], about = \"Bump the run.\")]"
        ),
        "update-ref cli overrides must surface on the UpdateTouch variant: {source}"
    );
}

#[test]
fn workflow_query_ref_with_xns_is_rejected_at_parse() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              query: [{ ref: "Status", xns: { task_queue: "other" } }]
            };
          }
          rpc Status(In) returns (Out) {
            option (temporal.v1.query) = {};
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .expect_err("xns on query ref must be rejected at parse")
        .to_string();
    assert!(
        err.contains("xns") && err.contains("query[ref=Status]"),
        "parse error must surface xns + query ref name, got: {err}"
    );
}

#[test]
fn workflow_without_task_queue_fails_validation() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {};
          }
        }
        message In {}
        message Out {}
        "#,
    );

    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let err = validate::validate(&services[0], &Default::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("task_queue"),
        "validation error should mention task_queue, got: {err}"
    );
}

/// Byte-for-byte golden test for the `minimal_workflow` fixture. Run with
/// `BLESS=1 cargo test --workspace --test parse_validate
/// minimal_workflow_render_golden` to rebless after intentional render
/// changes; the test will write `expected.rs` in place and pass.
#[test]
fn minimal_workflow_render_golden() {
    assert_golden("minimal_workflow");
}

#[test]
fn workflow_only_render_golden() {
    assert_golden("workflow_only");
}

#[test]
fn multiple_workflows_render_golden() {
    assert_golden("multiple_workflows");
}

#[test]
fn full_workflow_render_golden() {
    assert_golden("full_workflow");
}

#[test]
fn empty_input_workflow_render_golden() {
    assert_golden("empty_input_workflow");
}

#[test]
fn empty_output_query_update_render_golden() {
    assert_golden("empty_output_query_update");
}

/// Sanity: confirm the new empty-output fixture parses and validates
/// cleanly — so any future render-time breakage (e.g. dropping the
/// `_unit` dispatch in `render_query_method`) shows up here.
#[test]
fn empty_output_query_update_parses_and_validates() {
    let services = parse_and_validate("empty_output_query_update");
    assert_eq!(services.len(), 1);
    let svc = &services[0];
    assert_eq!(svc.queries.len(), 2);
    assert_eq!(svc.updates.len(), 3);
    for q in &svc.queries {
        assert!(
            q.output_type.is_empty,
            "fixture invariant: every query must have Empty output (got {})",
            q.output_type.full_name
        );
    }
    for u in &svc.updates {
        assert!(
            u.output_type.is_empty,
            "fixture invariant: every update must have Empty output (got {})",
            u.output_type.full_name
        );
    }
}

#[test]
fn activity_only_render_golden() {
    assert_golden("activity_only");
}

#[test]
fn activities_emit_render_golden() {
    assert_golden("activities_emit");
}

#[test]
fn worker_activities_only_render_golden() {
    assert_golden("worker_activities_only");
}

#[test]
fn activity_default_options_factory_emitted_when_proto_declares_timeouts() {
    // R3 — every activity that declares at least one close-timeout in its
    // `(temporal.v1.activity)` annotation now ships an
    // `<activity>_default_options()` factory that constructs the SDK's
    // `ActivityOptions` with those proto defaults baked in. Other fields
    // (task_queue, schedule_to_start_timeout, heartbeat_timeout,
    // retry_policy) chain onto the builder.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package act_defaults.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "tq" };
          }
          rpc Work(WorkInput) returns (WorkOutput) {
            option (temporal.v1.activity) = {
              task_queue:                "heavy-pool"
              start_to_close_timeout:    { seconds: 30 }
              schedule_to_start_timeout: { seconds: 5 }
              heartbeat_timeout:         { seconds: 10 }
              retry_policy:              { max_attempts: 4 }
            };
          }
        }
        message In {}  message Out {}
        message WorkInput  { string note = 1; }
        message WorkOutput { bool ok = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        activities: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);

    assert!(
        source
            .contains("pub fn work_default_options() -> temporal_runtime::worker::ActivityOptions"),
        "must emit per-activity default-options factory: {source}"
    );
    assert!(
        source.contains(
            "temporal_runtime::worker::ActivityOptions::with_start_to_close_timeout(Duration::new(30, 0))"
        ),
        "only-start-to-close path must kick the builder with that variant: {source}"
    );
    assert!(
        source.contains(".task_queue(\"heavy-pool\".to_string())"),
        "task_queue must chain onto the builder: {source}"
    );
    assert!(
        source.contains(".schedule_to_start_timeout(Duration::new(5, 0))"),
        "schedule_to_start_timeout must chain: {source}"
    );
    assert!(
        source.contains(".heartbeat_timeout(Duration::new(10, 0))"),
        "heartbeat_timeout must chain: {source}"
    );
    assert!(
        source.contains(".retry_policy("),
        "retry_policy must chain via .into() conversion: {source}"
    );
}

#[test]
fn activity_default_options_picks_both_variant_when_proto_sets_both_close_timeouts() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package act_defaults.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "tq" };
          }
          rpc Work(In) returns (Out) {
            option (temporal.v1.activity) = {
              start_to_close_timeout:    { seconds: 30 }
              schedule_to_close_timeout: { seconds: 600 }
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        activities: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            "ActivityCloseTimeouts::Both { start_to_close: Duration::new(30, 0), schedule_to_close: Duration::new(600, 0) }"
        ),
        "both-close-timeout path must produce the `Both` close-timeouts variant: {source}"
    );
}

#[test]
fn activity_default_options_omitted_when_proto_skips_close_timeouts() {
    // No close timeout declared → no factory (SDK can't build
    // ActivityOptions without `close_timeouts`). The activity still gets
    // its name const + marker; just no default-options helper.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package act_defaults.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "tq" };
          }
          rpc Work(In) returns (Out) {
            option (temporal.v1.activity) = {
              task_queue: "ignored"
              heartbeat_timeout: { seconds: 5 }
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        activities: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        !source.contains("work_default_options"),
        "no close-timeout → no default-options factory: {source}"
    );
}

#[test]
fn activity_default_options_honours_wait_for_cancellation() {
    // R3 — `wait_for_cancellation = true` now folds into the per-activity
    // factory as `.cancellation_type(WaitCancellationCompleted)`. `false`
    // (proto default) emits no setter so the SDK's `TryCancel` default
    // stays — matches Go-plugin behaviour.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package act_wait.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Work(In) returns (Out) {
            option (temporal.v1.activity) = {
              start_to_close_timeout: { seconds: 30 }
              wait_for_cancellation:   true
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        activities: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            ".cancellation_type(temporal_runtime::worker::ActivityCancellationType::WaitCancellationCompleted)"
        ),
        "wait_for_cancellation=true must chain WaitCancellationCompleted onto the builder: {source}"
    );
}

#[test]
fn activity_default_options_omits_cancellation_setter_when_proto_omits_it() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package act_wait.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Work(In) returns (Out) {
            option (temporal.v1.activity) = {
              start_to_close_timeout: { seconds: 30 }
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        activities: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        !source.contains(".cancellation_type("),
        "wait_for_cancellation unset → no cancellation_type setter: {source}"
    );
}

#[test]
fn activities_emit_renders_per_activity_marker_structs() {
    // R3 — every activity with non-Empty input AND output gets a marker
    // struct + ActivityDefinition impl, so workflow code can call
    // `ctx.start_activity(<RPC>Activity, input, opts)` against a typed
    // marker. Empty-side activities are skipped because `()` doesn't
    // implement TemporalSerializable/Deserializable in temporalio-common 0.4.
    let services = parse_and_validate("activities_emit");
    let opts = load_fixture_options("activities_emit");
    assert!(opts.activities);
    let source = render::render(&services[0], &opts);
    // ChunkInput / ChunkOutput → Process gets the full marker + impl.
    assert!(
        source.contains("pub struct ProcessActivity;"),
        "Process activity must produce a marker struct: {source}"
    );
    assert!(
        source.contains("impl temporal_runtime::worker::ActivityDefinition for ProcessActivity"),
        "Process activity must impl ActivityDefinition: {source}"
    );
    assert!(
        source.contains("type Input = temporal_runtime::TypedProtoMessage<ChunkInput>;"),
        "marker Input must wrap the prost input in TypedProtoMessage: {source}"
    );
    assert!(
        source.contains("type Output = temporal_runtime::TypedProtoMessage<ChunkOutput>;"),
        "marker Output must wrap the prost output in TypedProtoMessage: {source}"
    );
    assert!(
        source.contains("fn name() -> &'static str { PROCESS_ACTIVITY_NAME }"),
        "marker name() must delegate to the existing name const: {source}"
    );

    // Heartbeat has Empty input — now also gets a marker + helper, with
    // the Empty side carried by `temporal_runtime::ProtoEmpty`.
    assert!(
        source.contains("pub struct HeartbeatActivity;"),
        "Empty-input activity must produce a marker struct: {source}"
    );
    assert!(
        source.contains(
            "type Input = temporal_runtime::TypedProtoMessage<temporal_runtime::ProtoEmpty>;"
        ),
        "Empty input must be wrapped in TypedProtoMessage<ProtoEmpty>: {source}"
    );
    assert!(
        source.contains("pub const HEARTBEAT_ACTIVITY_NAME"),
        "Heartbeat name const must remain available"
    );

    // R3 — workflow-side helper. Wraps `ctx.start_activity(...)` and
    // unwraps the `TypedProtoMessage` envelope so the workflow body
    // sees the raw `ChunkOutput` back. Generic over `W` so it works
    // from any workflow body in the service.
    assert!(
        source.contains("pub async fn execute_process<W>("),
        "must emit `execute_process` workflow-side helper: {source}"
    );
    assert!(
        source.contains("ctx: &temporal_runtime::worker::WorkflowContext<W>,"),
        "helper must take a generic WorkflowContext<W>: {source}"
    );
    assert!(
        source.contains("opts: temporal_runtime::worker::ActivityOptions,"),
        "helper must take an ActivityOptions: {source}"
    );
    assert!(
        source.contains("-> ::std::result::Result<ChunkOutput, temporal_runtime::worker::ActivityExecutionError>"),
        "helper return type must surface the raw output, not the wrapper: {source}"
    );
    assert!(
        source.contains("ctx.start_activity(ProcessActivity, input, opts).await.map(temporal_runtime::TypedProtoMessage::into_inner)"),
        "helper must delegate to start_activity + unwrap: {source}"
    );
    // Empty-input helper: no `input` arg, constructs ProtoEmpty internally.
    assert!(
        source.contains("pub async fn execute_heartbeat<W>("),
        "Empty-input activity must produce a helper: {source}"
    );
    assert!(
        source.contains("ctx.start_activity(HeartbeatActivity, temporal_runtime::ProtoEmpty {}, opts).await.map(temporal_runtime::TypedProtoMessage::into_inner)"),
        "Empty-input helper must construct ProtoEmpty internally: {source}"
    );

    // R3 — local-activity variant. Mirrors the regular helper but uses
    // `start_local_activity` + `LocalActivityOptions`. Same Empty-skip
    // gating so the suppression is consistent.
    assert!(
        source.contains("pub async fn execute_process_local<W>("),
        "must emit `execute_process_local` workflow-side helper: {source}"
    );
    assert!(
        source.contains("opts: temporal_runtime::worker::LocalActivityOptions,"),
        "local helper must take LocalActivityOptions: {source}"
    );
    assert!(
        source.contains("ctx.start_local_activity(ProcessActivity, input, opts).await.map(temporal_runtime::TypedProtoMessage::into_inner)"),
        "local helper must delegate to start_local_activity + unwrap: {source}"
    );
    assert!(
        source.contains("pub async fn execute_heartbeat_local<W>("),
        "Empty-input activity must also produce a local-activity helper: {source}"
    );
}

#[test]
fn activities_emit_renders_trait_and_consts() {
    let services = parse_and_validate("activities_emit");
    let opts = load_fixture_options("activities_emit");
    assert!(
        opts.activities,
        "fixture options.txt should enable activities"
    );
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub trait ChunkServiceActivities: Send + Sync + 'static"),
        "missing activities trait declaration: {source}"
    );
    assert!(
        source.contains("pub const PROCESS_ACTIVITY_NAME"),
        "missing Process name const"
    );
    assert!(
        source.contains("pub const HEARTBEAT_ACTIVITY_NAME"),
        "missing Heartbeat name const"
    );
    assert!(
        source.contains(
            "fn process(&self, ctx: temporal_runtime::ActivityContext, input: ChunkInput)"
        ),
        "Process trait method signature wrong: {source}"
    );
    assert!(
        source.contains("fn heartbeat(&self, ctx: temporal_runtime::ActivityContext, input: ())"),
        "Heartbeat (Empty input) trait method signature wrong: {source}"
    );
    assert!(
        source.contains("pub fn register_chunk_service_activities<I>("),
        "missing activities registration helper: {source}"
    );
    assert!(
        source
            .contains("I: ChunkServiceActivities + temporal_runtime::worker::ActivityImplementer"),
        "registration helper should require both generated trait and SDK implementer: {source}"
    );
}

#[test]
fn activities_emit_off_by_default() {
    let services = parse_and_validate("activities_emit");
    // No options.txt-driven flag, no activities trait.
    let source = render::render(&services[0], &Default::default());
    assert!(!source.contains("pub trait ChunkServiceActivities"));
    assert!(!source.contains("_ACTIVITY_NAME"));
}

#[test]
fn workflows_emit_render_golden() {
    assert_golden("workflows_emit");
}

#[test]
fn worker_workflow_only_render_golden() {
    assert_golden("worker_workflow_only");
}

#[test]
fn worker_full_render_golden() {
    assert_golden("worker_full");
}

#[test]
fn workflow_wait_for_cancellation_emits_cancel_type_in_default_child_options() {
    // R5 — proto `wait_for_cancellation = true` on a workflow folds into
    // `<rpc>_default_child_options()` as
    // `cancel_type: ChildWorkflowCancellationType::WaitCancellationCompleted`.
    // `false` (default) emits no setter so the SDK's default
    // (`ABANDON` per the coresdk proto) stays in place.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package wfc.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:           "tq"
              wait_for_cancellation: true
            };
          }
        }
        message In  { string name = 1; }
        message Out { string id = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let svc = &services[0];
    assert!(svc.workflows[0].wait_for_cancellation);

    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        workflows: true,
        ..Default::default()
    };
    let source = render::render(svc, &opts);
    assert!(
        source.contains("pub fn run_default_child_options()"),
        "wait_for_cancellation alone must still produce a child-options factory: {source}"
    );
    assert!(
        source.contains("cancel_type: temporal_runtime::worker::ChildWorkflowCancellationType::WaitCancellationCompleted,"),
        "factory must set cancel_type to WaitCancellationCompleted: {source}"
    );
}

#[test]
fn workflow_parent_close_policy_and_wait_for_cancellation_combine() {
    // Both fields together → factory body emits *both* setters.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package combine.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:           "tq"
              parent_close_policy:   PARENT_CLOSE_POLICY_ABANDON
              wait_for_cancellation: true
            };
          }
        }
        message In  { string name = 1; }
        message Out { string id = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        workflows: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            "parent_close_policy: temporal_runtime::worker::ParentClosePolicy::Abandon.into(),"
        ),
        "must emit parent_close_policy setter: {source}"
    );
    assert!(
        source.contains("cancel_type: temporal_runtime::worker::ChildWorkflowCancellationType::WaitCancellationCompleted,"),
        "must emit cancel_type setter: {source}"
    );
}

#[test]
fn workflow_parent_close_policy_emits_default_child_options_factory() {
    // R5 — proto `parent_close_policy = PARENT_CLOSE_POLICY_ABANDON` now
    // folds into a per-workflow `<workflow>_default_child_options()`
    // factory that bakes the policy in. Caller passes the result straight
    // into `start_<workflow>_child(ctx, input, opts)`.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package pcp.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              parent_close_policy: PARENT_CLOSE_POLICY_ABANDON
            };
          }
        }
        message In  { string name = 1; }
        message Out { string id = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let svc = &services[0];
    use protoc_gen_rust_temporal::model::ParentClosePolicyKind;
    assert_eq!(
        svc.workflows[0].parent_close_policy,
        Some(ParentClosePolicyKind::Abandon),
        "model must carry the proto-declared policy"
    );

    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        workflows: true,
        ..Default::default()
    };
    let source = render::render(svc, &opts);
    assert!(
        source.contains(
            "pub fn run_default_child_options() -> temporal_runtime::worker::ChildWorkflowOptions"
        ),
        "must emit per-workflow default-child-options factory: {source}"
    );
    assert!(
        source.contains(
            "parent_close_policy: temporal_runtime::worker::ParentClosePolicy::Abandon.into(),"
        ),
        "factory must set parent_close_policy with the proto-declared variant: {source}"
    );
    assert!(
        source.contains("..::std::default::Default::default()"),
        "factory must spread the rest from Default to stay future-proof: {source}"
    );
}

#[test]
fn workflow_without_parent_close_policy_omits_default_child_options() {
    let services = parse_and_validate("workflows_emit");
    let opts = load_fixture_options("workflows_emit");
    let source = render::render(&services[0], &opts);
    assert!(
        !source.contains("run_default_child_options"),
        "no proto-declared parent_close_policy → no factory: {source}"
    );
}

#[test]
fn workflows_emit_renders_child_workflow_marker_and_helper() {
    // R2 — under `workflows=true`, every workflow with non-Empty input AND
    // output ships a `<RPC>Workflow` marker struct + WorkflowDefinition
    // impl plus a `start_<workflow>_child` workflow-side helper. The
    // helper lets workflow code spawn a typed child workflow without
    // hand-writing the WorkflowDefinition impl. Empty-side activities/
    // workflows fall through the same orphan-rule gating documented on
    // the activity emit.
    //
    // workflows_emit's Run rpc is non-Empty input (OrderInput) and
    // non-Empty output (OrderOutput), so the marker + helper must appear.
    let services = parse_and_validate("workflows_emit");
    let opts = load_fixture_options("workflows_emit");
    assert!(opts.workflows);
    let source = render::render(&services[0], &opts);

    assert!(
        source.contains("pub struct RunWorkflow;"),
        "must emit child-workflow marker struct: {source}"
    );
    assert!(
        source.contains("impl temporal_runtime::worker::WorkflowDefinition for RunWorkflow"),
        "marker must impl WorkflowDefinition: {source}"
    );
    assert!(
        source.contains("type Input = temporal_runtime::TypedProtoMessage<OrderInput>;"),
        "Input must be wrapped in TypedProtoMessage (orphan rule): {source}"
    );
    assert!(
        source.contains("type Output = temporal_runtime::TypedProtoMessage<OrderOutput>;"),
        "Output must be wrapped in TypedProtoMessage: {source}"
    );
    assert!(
        source.contains("fn name(&self) -> &str { self::RUN_WORKFLOW_NAME }"),
        "marker name() must delegate to the existing const: {source}"
    );

    assert!(
        source.contains("pub async fn start_run_child<W>("),
        "must emit start_run_child workflow-side helper: {source}"
    );
    assert!(
        source.contains("opts: temporal_runtime::worker::ChildWorkflowOptions,"),
        "helper must take ChildWorkflowOptions: {source}"
    );
    assert!(
        source.contains("-> ::std::result::Result<temporal_runtime::worker::StartedChildWorkflow<RunWorkflow>, temporal_runtime::worker::ChildWorkflowStartError>"),
        "helper must surface the typed StartedChildWorkflow handle: {source}"
    );
    assert!(
        source.contains("ctx.child_workflow(RunWorkflow, input, opts).await"),
        "helper must delegate to ctx.child_workflow: {source}"
    );
}

#[test]
fn workflows_emit_renders_external_signal_marker_and_helper() {
    // R2 — every non-Empty signal attached to a non-Empty workflow gets
    // a `<RPC>Signal` marker + `SignalDefinition` impl plus a
    // `signal_<rpc>_external` helper that opens an ExternalWorkflowHandle
    // and sends the typed signal from inside another workflow's context.
    let services = parse_and_validate("workflows_emit");
    let opts = load_fixture_options("workflows_emit");
    let source = render::render(&services[0], &opts);

    // workflows_emit's Cancel signal has CancelInput → non-Empty.
    assert!(
        source.contains("pub struct CancelSignal;"),
        "must emit signal marker struct: {source}"
    );
    assert!(
        source.contains("impl temporal_runtime::worker::SignalDefinition for CancelSignal"),
        "signal marker must impl SignalDefinition: {source}"
    );
    assert!(
        source.contains("type Workflow = RunWorkflow;"),
        "marker Workflow must point at the first non-Empty attaching workflow: {source}"
    );
    assert!(
        source.contains("type Input = temporal_runtime::TypedProtoMessage<CancelInput>;"),
        "marker Input must wrap CancelInput in TypedProtoMessage: {source}"
    );
    assert!(
        source.contains("fn name(&self) -> &str { self::CANCEL_SIGNAL_NAME }"),
        "marker name() must delegate to the existing const: {source}"
    );

    assert!(
        source.contains("pub async fn signal_cancel_external<W>("),
        "must emit external-signal helper: {source}"
    );
    assert!(
        source.contains("workflow_id: impl Into<String>,"),
        "helper must accept the target workflow id: {source}"
    );
    assert!(
        source.contains("run_id: Option<String>,"),
        "helper must accept an optional run id: {source}"
    );
    assert!(
        source.contains("-> temporal_runtime::worker::SignalExternalWfResult"),
        "helper must return SignalExternalWfResult: {source}"
    );
    assert!(
        source.contains("let handle = ctx.external_workflow(workflow_id, run_id);"),
        "helper must open the external handle: {source}"
    );
    assert!(
        source.contains(
            "handle.signal(CancelSignal, temporal_runtime::TypedProtoMessage::from(input)).await"
        ),
        "helper must dispatch the typed signal via the external handle: {source}"
    );
}

#[test]
fn workflows_emit_renders_continue_as_new_helper() {
    // R2 — continue-as-new helper. Wraps `ctx.continue_as_new(&input, opts)`
    // so workflow code can finish the current run and start a new one of
    // the same type with fresh input. Bound to
    // `WorkflowImplementation<Run = <RPC>Workflow>` so it only applies
    // to workflows whose macro-derived Run matches our marker.
    let services = parse_and_validate("workflows_emit");
    let opts = load_fixture_options("workflows_emit");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub fn continue_run_as_new<W>("),
        "must emit `continue_run_as_new` helper: {source}"
    );
    assert!(
        source.contains("opts: temporal_runtime::worker::ContinueAsNewOptions,"),
        "helper must take ContinueAsNewOptions: {source}"
    );
    assert!(
        source.contains(
            "-> ::std::result::Result<::std::convert::Infallible, temporal_runtime::worker::WorkflowTermination>"
        ),
        "helper return type must mirror the SDK's always-Err shape: {source}"
    );
    assert!(
        source.contains("W: temporal_runtime::worker::WorkflowImplementation<Run = RunWorkflow>,"),
        "helper must bind W to the marker via WorkflowImplementation::Run: {source}"
    );
    assert!(
        source.contains("let wrapped = temporal_runtime::TypedProtoMessage::from(input);"),
        "helper must wrap the raw input before forwarding: {source}"
    );
    assert!(
        source.contains("ctx.continue_as_new(&wrapped, opts)"),
        "helper must delegate to ctx.continue_as_new: {source}"
    );
}

#[test]
fn child_workflow_marker_suppressed_for_empty_io() {
    // Empty-input workflows fall through the orphan-rule gate. They keep
    // the Definition trait but skip the marker + helper.
    let services = parse_and_validate("empty_input_workflow");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        workflows: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    // Whatever the empty_input workflow rpc is named, the marker name
    // includes "Workflow" — check that no `pub struct *Workflow;` line
    // with a WorkflowDefinition impl appears.
    assert!(
        !source.contains("impl temporal_runtime::worker::WorkflowDefinition for"),
        "Empty-input workflow must not produce a WorkflowDefinition impl: {source}"
    );
    assert!(
        !source.contains("ctx.child_workflow("),
        "Empty-input workflow must not produce a child_workflow helper: {source}"
    );
}

#[test]
fn workflow_aliases_render_golden() {
    assert_golden("workflow_aliases");
}

#[test]
fn worker_workflow_aliases_render_golden() {
    assert_golden("worker_workflow_aliases");
}

#[test]
fn workflow_aliases_parses_and_emits_const() {
    let services = parse_and_validate("workflow_aliases");
    assert_eq!(services.len(), 1);
    let svc = &services[0];
    let wf = &svc.workflows[0];
    assert_eq!(
        wf.aliases,
        vec![
            "aliases.v1.AliasService.RunLegacy".to_string(),
            "aliases.v1.AliasService.RunV0".to_string(),
        ],
        "(temporal.v1.workflow).aliases must survive into the model"
    );

    let source = render::render(svc, &Default::default());
    assert!(
        source.contains("pub const RUN_WORKFLOW_ALIASES: &[&str] = &[\"aliases.v1.AliasService.RunLegacy\", \"aliases.v1.AliasService.RunV0\"];"),
        "missing or malformed workflow aliases const: {source}"
    );
}

#[test]
fn workflow_aliases_const_omitted_when_empty() {
    // Regression guard: existing fixtures that don't set aliases must not
    // grow an aliases const, so previously-blessed goldens stay clean.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        !source.contains("_WORKFLOW_ALIASES"),
        "fixture without aliases should not emit an aliases const: {source}"
    );
}

#[test]
fn workflow_definition_trait_exposes_id_template_when_declared() {
    // R4 — the `<Workflow>Definition` trait re-exposes the
    // `<RPC>_WORKFLOW_ID_TEMPLATE` const as `ID_TEMPLATE` when
    // proto declares `id:`. Skipped when no template is declared.
    let services = parse_and_validate("workflows_emit");
    let opts = load_fixture_options("workflows_emit");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("const ID_TEMPLATE: &'static str = self::"),
        "Definition trait must re-expose ID_TEMPLATE when proto declares `id:`: {source}"
    );
}

#[test]
fn workflow_id_template_emits_control_char_guard() {
    // R1 — the substituted workflow id is also checked for control
    // characters at runtime (newline, tab, etc.). Such characters
    // round-trip on the wire but make production logs and
    // dashboards unable to disambiguate one workflow id from
    // another, so panic locally with a precise diagnostic.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("id.chars().find(|c| c.is_control())"),
        "id fn must emit control-char guard: {source}"
    );
    assert!(
        source.contains("containing control character"),
        "guard message must mention the failure mode: {source}"
    );
}

#[test]
fn workflow_id_template_emits_runtime_emptiness_guard() {
    // R1 — workflow id template with field substitution emits a
    // runtime `assert!` guarding against an empty result, so an
    // input with empty string fields surfaces immediately as a
    // panic with the template literal instead of a Temporal-side
    // "workflow id is required" error.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("assert!(!id.is_empty(),"),
        "id template fn must emit emptiness guard: {source}"
    );
    assert!(
        source.contains("resolved to an empty string at runtime"),
        "guard message must mention the resolution failure: {source}"
    );
}

#[test]
fn workflow_and_update_id_template_source_consts_emit() {
    // R4 — when proto declares `id:` on a workflow or update, the
    // generator now emits a `<RPC>_WORKFLOW_ID_TEMPLATE` /
    // `<RPC>_UPDATE_ID_TEMPLATE: &str` const carrying the verbatim
    // template source. Lets debug tools inspect what the user
    // declared without reconstructing from parsed segments.
    // Workflows / updates without `id:` produce no const.
    let services = parse_and_validate("full_workflow");
    let opts = load_fixture_options("full_workflow");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub const RUN_WORKFLOW_ID_TEMPLATE: &str = \"run-{{ .Name }}\";"),
        "missing RUN_WORKFLOW_ID_TEMPLATE const: {source}"
    );
}

#[test]
fn workflow_id_template_const_omitted_when_unset() {
    // No `id:` declared → no template const emitted. Workflow_only
    // declares `id:` so use a fresh inline fixture without one.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package no_id.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "tq" };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files).expect("parse");
    let source = render::render(&services[0], &Default::default());
    assert!(
        !source.contains("WORKFLOW_ID_TEMPLATE"),
        "no `id:` declared so no template const must emit: {source}"
    );
}

#[test]
fn child_workflow_marker_exposes_name_const() {
    // R4 — child-workflow markers (`<Wf>Workflow`) round out the
    // marker `NAME` const surface. Pairs with the prior signal /
    // activity NAME shipment.
    let services = parse_and_validate("worker_full");
    let opts = load_fixture_options("worker_full");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub const NAME: &'static str = self::RUN_WORKFLOW_NAME;"),
        "child-workflow marker must re-expose NAME: {source}"
    );
}

#[test]
fn signal_and_activity_markers_expose_name_const() {
    // R4 — every marker struct now also re-exposes the registered
    // `NAME` as an inherent const. The SDK's `name(&self)` /
    // `name()` requires an instance / type-import; the const lets
    // generic code read the wire name with just `<S>::NAME` /
    // `<A>::NAME` regardless of which trait is in scope.
    let services = parse_and_validate("worker_full");
    let opts = load_fixture_options("worker_full");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub const NAME: &'static str = self::CANCEL_SIGNAL_NAME;"),
        "signal marker must re-expose NAME: {source}"
    );
    let services_act = parse_and_validate("activities_emit");
    let opts_act = load_fixture_options("activities_emit");
    let source_act = render::render(&services_act[0], &opts_act);
    assert!(
        source_act.contains("pub const NAME: &'static str = self::"),
        "activity marker must re-expose NAME: {source_act}"
    );
}

#[test]
fn marker_structs_derive_standard_traits() {
    // R6 ergonomics — marker structs (`<Activity>Activity`,
    // `<Sig>Signal`, `<Wf>Workflow`) hold no state; deriving the
    // standard ergonomic traits (`Debug, Default, Clone, Copy,
    // PartialEq, Eq`) lets callers `dbg!()` them, store them in
    // structs that derive `Debug`, copy without ceremony, and use
    // `Default::default()`.
    let services = parse_and_validate("worker_full");
    let opts = load_fixture_options("worker_full");
    let source = render::render(&services[0], &opts);
    // Should appear at every marker struct decl (activity + signal +
    // child-workflow). worker_full has all three kinds.
    let derive_line = "#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]";
    assert!(
        source.matches(derive_line).count() >= 2,
        "expected marker derive on at least two struct kinds: {source}"
    );
}

#[test]
fn child_workflow_marker_exposes_task_queue_const() {
    // R4 — the child-workflow marker (`<Wf>Workflow`, emitted under
    // workflows=true when both input + output are non-Empty) now
    // also carries an inherent `TASK_QUEUE` const re-exposing the
    // workflow's effective task queue. Parallel of the activity-
    // marker shipment.
    let services = parse_and_validate("worker_full");
    let opts = load_fixture_options("worker_full");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub const TASK_QUEUE: &'static str = self::RUN_TASK_QUEUE;"),
        "child-workflow marker must re-expose TASK_QUEUE: {source}"
    );
}

#[test]
fn child_workflow_marker_re_exposes_id_template_const_when_declared() {
    // R6 ergonomics — completes the identity-const matrix on the child-
    // workflow marker, mirroring the parallel `<Wf>Handle::ID_TEMPLATE`
    // ship. NAME / INPUT_TYPE / OUTPUT_TYPE / TASK_QUEUE were already
    // re-exposed; ID_TEMPLATE was previously only on the Definition
    // trait, forcing generic worker code holding a `<W>Workflow` marker
    // to drag in the trait import. Now spellable as `<W>::ID_TEMPLATE`.
    // `workflows_emit` declares `id: "order-{{ .Id }}"` on its workflow
    // so the const should appear on `RunWorkflow`.
    let services = parse_and_validate("workflows_emit");
    let opts = load_fixture_options("workflows_emit");
    let source = render::render(&services[0], &opts);
    // Locate the inherent impl block on the child-workflow marker so we
    // know we're checking the right struct (not the Handle's ID_TEMPLATE
    // shipped in the previous turn).
    let marker_block_start = source
        .find("impl RunWorkflow {")
        .expect("RunWorkflow inherent impl present");
    let after_block = &source[marker_block_start..];
    let marker_block_end = after_block.find("\n    }\n").expect("inherent impl closer");
    let marker_block = &after_block[..marker_block_end];
    assert!(
        marker_block
            .contains("pub const ID_TEMPLATE: &'static str = self::RUN_WORKFLOW_ID_TEMPLATE;"),
        "child-workflow marker must re-expose ID_TEMPLATE when workflow declares one: {marker_block}"
    );
}

#[test]
fn child_workflow_marker_omits_id_template_const_when_not_declared() {
    // Skip-guard parity with the existing module-const emit. When the
    // workflow declares no id template, the marker must not bake an
    // empty string — a baked "" would mislead diagnostic code into
    // thinking a template existed. `worker_full` declares no `id` on
    // its workflow, so the const should NOT appear in its
    // `RunWorkflow` inherent impl.
    let services = parse_and_validate("worker_full");
    let opts = load_fixture_options("worker_full");
    let source = render::render(&services[0], &opts);
    let marker_block_start = source
        .find("impl RunWorkflow {")
        .expect("RunWorkflow inherent impl present");
    let after_block = &source[marker_block_start..];
    let marker_block_end = after_block.find("\n    }\n").expect("inherent impl closer");
    let marker_block = &after_block[..marker_block_end];
    assert!(
        !marker_block.contains("pub const ID_TEMPLATE:"),
        "child-workflow marker must omit ID_TEMPLATE when workflow declares none: {marker_block}"
    );
}

#[test]
fn child_workflow_and_signal_markers_expose_input_output_type_consts() {
    // R4 — child-workflow markers (`<Wf>Workflow`) and signal
    // markers (`<Sig>Signal`) gain inherent `INPUT_TYPE` /
    // `OUTPUT_TYPE` consts (signal markers omit OUTPUT — signals
    // are always Empty-output). Mirrors the activity-marker shipment.
    let services = parse_and_validate("worker_full");
    let opts = load_fixture_options("worker_full");
    let source = render::render(&services[0], &opts);
    // Child-workflow marker.
    assert!(
        source.contains("impl RunWorkflow {"),
        "expected `impl RunWorkflow {{` for inherent consts: {source}"
    );
    assert!(
        source.contains("pub const INPUT_TYPE: &'static str = self::RUN_INPUT_TYPE;"),
        "child-workflow marker must re-expose RUN_INPUT_TYPE: {source}"
    );
    assert!(
        source.contains("pub const OUTPUT_TYPE: &'static str = self::RUN_OUTPUT_TYPE;"),
        "child-workflow marker must re-expose RUN_OUTPUT_TYPE: {source}"
    );
    // Signal marker — INPUT_TYPE only (signals are Empty-output).
    assert!(
        source.contains("pub const INPUT_TYPE: &'static str = self::CANCEL_SIGNAL_INPUT_TYPE;"),
        "signal marker must re-expose CANCEL_SIGNAL_INPUT_TYPE: {source}"
    );
}

#[test]
fn activity_marker_exposes_task_queue_const_when_declared() {
    // R4 — activity marker structs gain `TASK_QUEUE: &'static str`
    // when proto declares `(temporal.v1.activity).task_queue`,
    // re-exposing the per-rpc `<RPC>_ACTIVITY_TASK_QUEUE` const.
    // Markers without a declared task_queue stay surface-clean so
    // tooling can disambiguate "activity overrides queue" vs
    // "activity inherits workflow queue".
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package amk_tq.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc DoWorkLocal(In) returns (Out) {
            option (temporal.v1.activity) = {
              task_queue: "specialised-queue"
              start_to_close_timeout: { seconds: 30 }
            };
          }
          rpc DoWorkShared(In) returns (Out) {
            option (temporal.v1.activity) = {
              start_to_close_timeout: { seconds: 30 }
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files).expect("parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        activities: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            "pub const TASK_QUEUE: &'static str = self::DO_WORK_LOCAL_ACTIVITY_TASK_QUEUE;"
        ),
        "DoWorkLocal marker must re-expose TASK_QUEUE: {source}"
    );
    // DoWorkShared must NOT carry a TASK_QUEUE const.
    let shared_block_start = source.find("impl DoWorkSharedActivity").unwrap();
    let shared_block_end = shared_block_start + source[shared_block_start..].find("    }").unwrap();
    let shared_block = &source[shared_block_start..shared_block_end];
    assert!(
        !shared_block.contains("TASK_QUEUE"),
        "DoWorkShared marker must omit TASK_QUEUE when no override: {shared_block}"
    );
}

#[test]
fn activity_marker_struct_exposes_input_output_type_consts() {
    // R4 — each activity marker struct gains inherent
    // `INPUT_TYPE` / `OUTPUT_TYPE` `&'static str` consts (sourced
    // from the per-rpc module-level proto-FQN consts) so generic
    // code holding a typed marker can pull the wire type name
    // without going through the SDK's `ActivityDefinition` trait.
    let services = parse_and_validate("activities_emit");
    let opts = load_fixture_options("activities_emit");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub const INPUT_TYPE: &'static str = self::"),
        "activity marker struct must expose INPUT_TYPE: {source}"
    );
    assert!(
        source.contains("pub const OUTPUT_TYPE: &'static str = self::"),
        "activity marker struct must expose OUTPUT_TYPE: {source}"
    );
}

#[test]
fn workflow_definition_trait_exposes_input_output_type_consts() {
    // R4 — `<Workflow>Definition` trait re-exposes the per-rpc
    // `<RPC>_INPUT_TYPE` / `_OUTPUT_TYPE` proto FQN consts as
    // associated `&'static str`. Lets generic worker code spell
    // `<W as <Wf>Definition>::INPUT_TYPE` for payload routing.
    let services = parse_and_validate("worker_workflow_only");
    let opts = load_fixture_options("worker_workflow_only");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("const INPUT_TYPE: &'static str = self::"),
        "Definition trait must re-expose INPUT_TYPE const: {source}"
    );
    assert!(
        source.contains("const OUTPUT_TYPE: &'static str = self::"),
        "Definition trait must re-expose OUTPUT_TYPE const: {source}"
    );
}

#[test]
fn worker_workflow_aliases_surfaces_on_definition_trait() {
    let services = parse_and_validate("worker_workflow_aliases");
    let opts = load_fixture_options("worker_workflow_aliases");
    assert!(opts.workflows);
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub const RUN_WORKFLOW_ALIASES: &[&str] = &[\"aliases.v1.AliasService.RunLegacy\", \"aliases.v1.AliasService.RunV0\"];"),
        "missing module-level aliases const: {source}"
    );
    assert!(
        source.contains(
            "const WORKFLOW_ALIASES: &'static [&'static str] = self::RUN_WORKFLOW_ALIASES;"
        ),
        "Definition trait should re-expose the aliases const: {source}"
    );
}

#[test]
fn workflows_emit_renders_handler_io_aliases() {
    // R2 — per-handler I/O aliases let workflow bodies spell handler
    // input/output types by role (`CancelSignalInput`, `StatusQueryOutput`,
    // …) instead of repeating the prost message names. Skipped on the
    // Empty side since aliasing `()` adds no value.
    let services = parse_and_validate("workflows_emit");
    let opts = load_fixture_options("workflows_emit");
    let source = render::render(&services[0], &opts);
    // Cancel signal: non-Empty input → CancelSignalInput alias.
    assert!(
        source.contains("pub type CancelSignalInput = CancelInput;"),
        "must emit signal-input alias: {source}"
    );
    // Status query: Empty input, non-Empty output → only output alias.
    assert!(
        !source.contains("pub type StatusQueryInput"),
        "Empty-input query must not produce an input alias: {source}"
    );
    assert!(
        source.contains("pub type StatusQueryOutput = StatusOutput;"),
        "non-Empty-output query must produce an output alias: {source}"
    );
    // Confirm update: non-Empty input AND non-Empty output → both aliases.
    assert!(
        source.contains("pub type ConfirmUpdateInput = ConfirmInput;"),
        "must emit update-input alias: {source}"
    );
    assert!(
        source.contains("pub type ConfirmUpdateOutput = ConfirmOutput;"),
        "must emit update-output alias: {source}"
    );
    // Header banner appears exactly once.
    let header_hits = source.matches("Workflow handler I/O aliases").count();
    assert_eq!(
        header_hits, 1,
        "alias section header must appear once: {source}"
    );
}

#[test]
fn workflow_handler_io_aliases_skipped_when_workflows_off() {
    // The aliases live under the existing `render_workflow_handler_name_consts`
    // emit which is gated by `workflows=true`. Confirm the default
    // RenderOptions doesn't produce them.
    let services = parse_and_validate("workflows_emit");
    let source = render::render(&services[0], &Default::default());
    assert!(
        !source.contains("CancelSignalInput"),
        "aliases must not appear without workflows=true: {source}"
    );
}

#[test]
fn workflows_emit_renders_handler_name_consts() {
    let services = parse_and_validate("workflows_emit");
    let opts = load_fixture_options("workflows_emit");
    assert!(
        opts.workflows,
        "fixture options.txt should enable workflows"
    );
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub const CANCEL_SIGNAL_NAME: &str = \"wf.v1.OrderService.Cancel\";"),
        "missing Cancel signal name const: {source}"
    );
    assert!(
        source.contains("pub const STATUS_QUERY_NAME: &str = \"wf.v1.OrderService.Status\";"),
        "missing Status query name const"
    );
    assert!(
        source.contains("pub const CONFIRM_UPDATE_NAME: &str = \"wf.v1.OrderService.Confirm\";"),
        "missing Confirm update name const"
    );
    assert!(
        source.contains("pub trait RunDefinition: 'static"),
        "missing workflow definition trait: {source}"
    );
    assert!(
        source.contains("const WORKFLOW_NAME: &'static str = self::RUN_WORKFLOW_NAME;"),
        "missing workflow name associated const"
    );
    assert!(
        source.contains("const TASK_QUEUE: &'static str = self::RUN_TASK_QUEUE;"),
        "missing task queue associated const"
    );
    assert!(
        source.contains("pub fn register_run_workflow<W>("),
        "missing workflow registration helper"
    );
    assert!(
        source.contains("W: temporal_runtime::worker::WorkflowImplementer + RunDefinition<Input = OrderInput, Output = OrderOutput>"),
        "registration helper should bind SDK implementer to generated definition trait: {source}"
    );
}

#[test]
fn cli_emit_render_golden() {
    assert_golden("cli_emit");
}

#[test]
fn cli_ignore_render_golden() {
    assert_golden("cli_ignore");
}

#[test]
fn cli_ignore_filters_workflows_from_command_enum() {
    let services = parse_and_validate("cli_ignore");
    let opts = load_fixture_options("cli_ignore");
    assert!(opts.cli);
    let source = render::render(&services[0], &opts);

    // The non-ignored workflow must drive a StartGenerate/AttachGenerate
    // subcommand pair…
    assert!(
        source.contains("StartGenerate(StartGenerateArgs)"),
        "non-ignored workflow must appear in Command enum: {source}"
    );
    assert!(
        source.contains("AttachGenerate(AttachGenerateArgs)"),
        "non-ignored workflow must appear in Command enum"
    );
    // …and the ignored workflow must NOT appear anywhere in the CLI
    // module — neither as a subcommand variant nor as an Args struct.
    assert!(
        !source.contains("StartInternal"),
        "cli.ignore workflow must be filtered out of the CLI: {source}"
    );
    assert!(
        !source.contains("AttachInternal"),
        "cli.ignore workflow must be filtered out of the CLI: {source}"
    );
    assert!(
        !source.contains("pub struct StartInternalArgs"),
        "ignored workflow must not produce an Args struct"
    );
}

#[test]
fn cli_scaffold_suppressed_when_every_workflow_ignored() {
    // If every workflow is ignored, emitting the CLI module would produce a
    // clap Subcommand enum with no variants — clap fails to derive that.
    // Suppress the entire scaffold instead.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package cli_off.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              cli: { ignore: true }
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        !source.contains("pub mod svc_cli"),
        "fully-ignored services must not emit a CLI module at all: {source}"
    );
}

#[test]
fn workflow_cli_name_and_aliases_emit_clap_overrides() {
    // R6 — `(temporal.v1.workflow).cli.name` overrides the kebab-case
    // clap subcommand name and `cli.aliases` add extra subcommand names,
    // applied uniformly to the generated `Start<Wf>` and `Attach<Wf>`
    // variants. `cli.usage` (help text override) still stays rejected
    // because emitting it requires rewriting the per-variant docstring
    // path.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              cli: { name: "custom" aliases: ["alt-1", "alt-2"] }
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services =
        parse::parse(&pool, &files_to_generate).expect("cli.name + cli.aliases must parse");
    assert_eq!(services[0].workflows[0].cli_name.as_deref(), Some("custom"));
    assert_eq!(services[0].workflows[0].cli_aliases, vec!["alt-1", "alt-2"]);

    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            "#[command(name = \"start-custom\", alias = [\"start-alt-1\", \"start-alt-2\"])]"
        ),
        "start variant must carry the per-workflow clap overrides: {source}"
    );
    assert!(
        source.contains(
            "#[command(name = \"attach-custom\", alias = [\"attach-alt-1\", \"attach-alt-2\"])]"
        ),
        "attach variant must mirror the overrides with its own verb prefix: {source}"
    );
}

#[test]
fn workflow_cli_usage_emits_clap_about_override() {
    // R6 — `(temporal.v1.workflow).cli.usage` lands as
    // `#[command(about = "<usage>")]` on both the start and attach
    // variants, overriding clap's docstring-derived default.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              cli: { usage: "Run the thing." }
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("cli.usage must parse cleanly");
    assert_eq!(
        services[0].workflows[0].cli_usage.as_deref(),
        Some("Run the thing.")
    );
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("#[command(about = \"Run the thing.\")]"),
        "cli.usage must surface as #[command(about = ...)] on the variants: {source}"
    );
    assert_eq!(
        source
            .matches("#[command(about = \"Run the thing.\")]")
            .count(),
        4,
        "cli.usage must apply to all four variants (start/attach/cancel/terminate): {source}"
    );
}

#[test]
fn cli_emit_renders_run_with_dispatch() {
    // R6 — `Cli::run_with(&Client, deserialize_fn)` impl. Generic over
    // a `FnMut(&Path, &'static str) -> Future<Result<Box<dyn Any>>>` so
    // the consumer plugs JSON / pbjson / raw-bytes decode without us
    // committing to one.
    let services = parse_and_validate("cli_emit");
    let opts = load_fixture_options("cli_emit");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub async fn run_with<F, Fut>("),
        "must emit run_with dispatch fn: {source}"
    );
    assert!(
        source.contains("F: FnMut(&::std::path::Path, &'static str) -> Fut,"),
        "closure takes path + fully-qualified message type: {source}"
    );
    assert!(
        source.contains("Fut: ::std::future::Future<Output = ::std::result::Result<::std::boxed::Box<dyn ::std::any::Any + ::std::marker::Send>, ::std::boxed::Box<dyn ::std::error::Error + Send + Sync>>>,"),
        "closure must return Box<dyn Any + Send> so heterogeneous inputs work: {source}"
    );
    assert!(
        source.contains("Command::StartGenerate(args) =>"),
        "must dispatch on each Start<Wf> variant: {source}"
    );
    assert!(
        source.contains("Command::AttachGenerate(args) =>"),
        "must dispatch on each Attach<Wf> variant: {source}"
    );
    assert!(
        source.contains(
            "let dyn_input = read_input(&args.input_file, \"cli.v1.GenerateInput\").await?;"
        ),
        "must invoke the closure with the input file path + FQ message type: {source}"
    );
    assert!(
        source.contains("let input: GenerateInput = *dyn_input.downcast::<GenerateInput>()"),
        "must downcast the boxed Any into the typed input: {source}"
    );
    assert!(
        source.contains("let handle = client.generate(input, opts).await?;"),
        "must forward to <Service>Client::<rpc>(input, opts): {source}"
    );
    assert!(
        source.contains(
            "if args.wait { let out = handle.result().await?; ::std::println!(\"result={:?}\", out); }"
        ),
        "must wait on result when --wait was passed and print the typed output: {source}"
    );
}

#[test]
fn cli_emit_renders_clap_subcommands() {
    let services = parse_and_validate("cli_emit");
    let opts = load_fixture_options("cli_emit");
    assert!(opts.cli, "fixture options.txt should enable cli");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub mod report_service_cli {"),
        "missing CLI module: {source}"
    );
    assert!(
        source.contains("#[derive(Debug, temporal_runtime::clap::Parser)]"),
        "missing Cli derive"
    );
    assert!(
        source.contains("StartGenerate(StartGenerateArgs),"),
        "missing StartGenerate subcommand variant"
    );
    assert!(
        source.contains("AttachAggregate(AttachAggregateArgs),"),
        "missing AttachAggregate subcommand variant"
    );
    assert!(
        source.contains("pub struct StartGenerateArgs {"),
        "missing StartGenerateArgs struct"
    );
}

#[test]
fn cli_emit_renders_cancel_and_terminate_subcommands() {
    // R6 — per-workflow `Cancel<Wf>` / `Terminate<Wf>` subcommands
    // call into the existing `Handle::cancel_workflow` /
    // `Handle::terminate_workflow` methods. Both accept a positional
    // workflow id and an optional `--reason` flag forwarded to the
    // bridge call.
    let services = parse_and_validate("cli_emit");
    let opts = load_fixture_options("cli_emit");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("CancelGenerate(CancelGenerateArgs),"),
        "missing CancelGenerate variant: {source}"
    );
    assert!(
        source.contains("TerminateGenerate(TerminateGenerateArgs),"),
        "missing TerminateGenerate variant: {source}"
    );
    assert!(
        source.contains("pub struct CancelGenerateArgs {"),
        "missing CancelGenerateArgs struct: {source}"
    );
    assert!(
        source.contains("pub struct TerminateGenerateArgs {"),
        "missing TerminateGenerateArgs struct: {source}"
    );
    // The reason flag must be optional with an empty-string default so
    // callers can omit it.
    assert!(
        source.contains("#[arg(long, default_value = \"\")]"),
        "reason flag must default to empty string: {source}"
    );
    // Dispatch must forward the reason to the existing Handle methods.
    assert!(
        source.contains("handle.cancel_workflow(&args.reason).await?;"),
        "cancel dispatch must forward reason: {source}"
    );
    assert!(
        source.contains("handle.terminate_workflow(&args.reason).await?;"),
        "terminate dispatch must forward reason: {source}"
    );
}

#[test]
fn workflow_attached_handler_name_consts_emit() {
    // R4 — per-workflow `<RPC>_ATTACHED_{SIGNAL,QUERY,UPDATE}_NAMES`
    // consts list the registered names of handlers the workflow refs
    // via `WorkflowOptions.{signal,query,update}[]`. Only emits when
    // the attached list is non-empty.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package att.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Cancel" }, { ref: "Pause" }]
              query:  [{ ref: "Status" }]
              update: [{ ref: "Touch" }]
            };
          }
          rpc Cancel(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
          rpc Pause(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
          rpc Status(google.protobuf.Empty) returns (StatusOutput) {
            option (temporal.v1.query) = {};
          }
          rpc Touch(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.update) = {};
          }
          rpc Bare(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "tq" };
          }
        }
        message In  {}
        message Out {}
        message StatusOutput { string phase = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains(
            "pub const RUN_ATTACHED_SIGNAL_NAMES: &'static [&'static str] = &[\"att.v1.Svc.Cancel\", \"att.v1.Svc.Pause\"];"
        ),
        "RUN_ATTACHED_SIGNAL_NAMES missing or wrong: {source}"
    );
    assert!(
        source.contains(
            "pub const RUN_ATTACHED_QUERY_NAMES: &'static [&'static str] = &[\"att.v1.Svc.Status\"];"
        ),
        "RUN_ATTACHED_QUERY_NAMES missing or wrong: {source}"
    );
    assert!(
        source.contains(
            "pub const RUN_ATTACHED_UPDATE_NAMES: &'static [&'static str] = &[\"att.v1.Svc.Touch\"];"
        ),
        "RUN_ATTACHED_UPDATE_NAMES missing or wrong: {source}"
    );
    // Workflow with no attached refs must NOT emit empty consts.
    assert!(
        !source.contains("BARE_ATTACHED_SIGNAL_NAMES"),
        "must not emit empty BARE attached-signal const: {source}"
    );
    assert!(
        !source.contains("BARE_ATTACHED_QUERY_NAMES"),
        "must not emit empty BARE attached-query const: {source}"
    );
    assert!(
        !source.contains("BARE_ATTACHED_UPDATE_NAMES"),
        "must not emit empty BARE attached-update const: {source}"
    );
}

#[test]
fn cli_name_override_colliding_with_default_derived_fails_validation() {
    // A workflow's `cli.name` override that matches the kebab-case
    // default-derived subcommand value of another workflow would
    // produce duplicate clap subcommand names. Catch at codegen.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package cli_default_clash.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          // Default-derived clap subcommand value: "alpha-flow".
          rpc AlphaFlow(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "tq" };
          }
          // Explicit override claims the same value.
          rpc Beta(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              cli: { name: "alpha-flow" }
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("override colliding with default-derived value must be rejected")
        .to_string();
    assert!(
        err.contains("`alpha-flow`") && err.contains("AlphaFlow") && err.contains("Beta"),
        "diagnostic must name value + both workflows, got: {err}"
    );
}

#[test]
fn cross_workflow_cli_name_collision_fails_validation() {
    // Two workflows on the same service can't claim the same
    // `cli.name` — they'd produce identical clap subcommand names
    // (`start-go` etc.) and clap rejects duplicates at runtime.
    // Refuse at codegen with a diagnostic naming both workflows.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package cli_name_clash.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Alpha(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              cli: { name: "go" }
            };
          }
          rpc Beta(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              cli: { name: "go" }
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("duplicate cli.name across workflows must be rejected")
        .to_string();
    assert!(
        err.contains("cli subcommand")
            && err.contains("`go`")
            && err.contains("Alpha")
            && err.contains("Beta"),
        "diagnostic must name value + both workflows, got: {err}"
    );
}

#[test]
fn cross_workflow_cli_alias_vs_name_collision_fails_validation() {
    // The same value showing up as workflow A's `cli.name` and
    // workflow B's `cli.aliases` entry is the same duplicate-
    // subcommand bug.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package cli_alias_clash.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Alpha(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              cli: { name: "go" }
            };
          }
          rpc Beta(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              cli: { name: "beta-cmd" aliases: ["go"] }
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("alias-vs-name cli collision must be rejected")
        .to_string();
    assert!(
        err.contains("cli subcommand")
            && err.contains("`go`")
            && err.contains("Alpha")
            && err.contains("Beta"),
        "diagnostic must name value + both workflows, got: {err}"
    );
}

#[test]
fn conflicting_signal_ref_cli_overrides_across_workflows_fail_validation() {
    // The CLI emit is service-scoped — only one `Signal<Name>`
    // variant per signal — so contradictory per-ref overrides across
    // workflows would silently pick the first and surface as a
    // "why did the CLI use that name?" mystery. Reject at codegen.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sig_conflict.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Alpha(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Cancel" cli: { name: "abort" } }]
            };
          }
          rpc Beta(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Cancel" cli: { name: "halt" } }]
            };
          }
          rpc Cancel(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("conflicting cli overrides must be rejected")
        .to_string();
    assert!(
        err.contains("signal")
            && err.contains("Cancel")
            && err.contains("Alpha")
            && err.contains("Beta"),
        "diagnostic must name kind + ref + both workflows, got: {err}"
    );
}

#[test]
fn matching_signal_ref_cli_overrides_across_workflows_pass_validation() {
    // If multiple workflows declare the *same* override values for
    // the same ref, that's not a conflict — both can register the
    // same intent. Validation must allow this.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sig_match.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Alpha(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Cancel" cli: { name: "abort" } }]
            };
          }
          rpc Beta(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Cancel" cli: { name: "abort" } }]
            };
          }
          rpc Cancel(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect("matching cli overrides on the same ref must not be rejected");
}

#[test]
fn workflow_task_queue_with_space_fails_validation() {
    // Task queue names with embedded whitespace make worker
    // assignment nearly impossible to diagnose. Reject at codegen.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package tq_ws.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "my queue" };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("whitespace in task_queue must be rejected")
        .to_string();
    assert!(
        err.contains("task_queue") && err.contains("whitespace") && err.contains("Run"),
        "diagnostic must name site + flavour + workflow, got: {err}"
    );
}

#[test]
fn service_level_task_queue_with_newline_fails_validation() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package tq_nl.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          option (temporal.v1.service) = { task_queue: "bad\nqueue" };
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {};
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("control char in service task_queue must be rejected")
        .to_string();
    assert!(
        err.contains("service-level") && err.contains("control"),
        "diagnostic must name site + control flavour, got: {err}"
    );
}

#[test]
fn workflow_cli_name_with_space_fails_validation() {
    // clap subcommand names must be a single shell token — a value
    // with a space splits into two args at runtime. Reject at codegen.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package cli_ws.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              cli: { name: "run command" }
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("whitespace in cli.name must be rejected")
        .to_string();
    assert!(
        err.contains("cli.name") && err.contains("whitespace") && err.contains("Run"),
        "diagnostic must name site + flavour + workflow, got: {err}"
    );
}

#[test]
fn signal_ref_cli_alias_with_newline_fails_validation() {
    // Same printable-name guard applies to signal-ref cli aliases.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package cli_nl.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Cancel" cli: { aliases: ["bad\nalias"] } }]
            };
          }
          rpc Cancel(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("newline in signal-ref cli alias must be rejected")
        .to_string();
    assert!(
        err.contains("signal[ref=Cancel]") && err.contains("control"),
        "diagnostic must name site + control-char flavour, got: {err}"
    );
}

#[test]
fn workflow_registered_name_with_whitespace_fails_validation() {
    // A `name:` override containing whitespace would round-trip but
    // make production logs ambiguous — reject at codegen.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package ws_name.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              name:       "My Workflow"
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("whitespace in registered name must be rejected")
        .to_string();
    assert!(
        err.contains("whitespace") && err.contains("My Workflow"),
        "diagnostic must name whitespace + offending value, got: {err}"
    );
}

#[test]
fn signal_registered_name_with_newline_fails_validation() {
    // Same check applies to signal names.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package nl_sig.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Cancel" }]
            };
          }
          rpc Cancel(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = { name: "bad\nname" };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("newline in signal registered name must be rejected")
        .to_string();
    assert!(
        err.contains("signal") && err.contains("control"),
        "diagnostic must name kind + control-character flavour, got: {err}"
    );
}

#[test]
fn duplicate_activity_registered_name_fails_validation() {
    // Two activities registering under the same Temporal name would
    // silently dedupe at the worker — refuse at codegen.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package dup_act.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc AlphaWork(In) returns (Out) {
            option (temporal.v1.activity) = {
              name: "shared-activity"
              start_to_close_timeout: { seconds: 30 }
            };
          }
          rpc BetaWork(In) returns (Out) {
            option (temporal.v1.activity) = {
              name: "shared-activity"
              start_to_close_timeout: { seconds: 30 }
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("duplicate activity registered_name must be rejected")
        .to_string();
    assert!(
        err.contains("activity")
            && err.contains("shared-activity")
            && err.contains("AlphaWork")
            && err.contains("BetaWork"),
        "diagnostic must name kind + value + both rpcs, got: {err}"
    );
}

#[test]
fn duplicate_signal_registered_name_fails_validation() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package dup_sig.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Halt" }, { ref: "Stop" }]
            };
          }
          rpc Halt(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = { name: "shared-signal" };
          }
          rpc Stop(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = { name: "shared-signal" };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("duplicate signal registered_name must be rejected")
        .to_string();
    assert!(
        err.contains("signal")
            && err.contains("shared-signal")
            && err.contains("Halt")
            && err.contains("Stop"),
        "diagnostic must name kind + value + both rpcs, got: {err}"
    );
}

#[test]
fn workflow_alias_collision_across_workflows_fails_validation() {
    // Two workflows on the same service can't share a Temporal name —
    // would register both under the same name and route to either at
    // runtime. Either alias-vs-alias overlap or alias-vs-registered_name
    // overlap must be refused.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package wf_alias_cross.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Alpha(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              aliases:    ["shared"]
            };
          }
          rpc Beta(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              aliases:    ["shared"]
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("cross-workflow alias collision must be rejected by validate")
        .to_string();
    assert!(
        err.contains("alias `shared`") && err.contains("Alpha") && err.contains("Beta"),
        "expected cross-workflow alias-collision diagnostic naming both workflows + alias, got: {err}"
    );
}

#[test]
fn workflow_alias_collides_with_other_workflows_registered_name_fails_validation() {
    // An alias on workflow B that equals workflow A's `registered_name`
    // is the same duplicate-registration footgun.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package wf_alias_vs_name.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Alpha(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              name:       "fixed-name"
            };
          }
          rpc Beta(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              aliases:    ["fixed-name"]
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(&services[0], &render_opts)
        .expect_err("alias-vs-other-name collision must be rejected by validate")
        .to_string();
    assert!(
        err.contains("alias `fixed-name`") && err.contains("Beta") && err.contains("Alpha"),
        "expected alias-vs-name diagnostic naming both workflows + value, got: {err}"
    );
}

#[test]
fn workflow_alias_collision_with_registered_name_fails_at_parse() {
    // Catch a real bug: a workflow alias that equals the workflow's
    // own registered name would attempt to register the same workflow
    // twice under the same Temporal name. Refuse at parse rather than
    // ship the duplicate registration.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package wf_alias_self.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              name:       "explicit-name"
              aliases:    ["explicit-name", "extra-alias"]
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .expect_err("alias colliding with registered_name must be rejected at parse")
        .to_string();
    assert!(
        err.contains("collides with the workflow's registered name")
            && err.contains("explicit-name"),
        "expected alias-self-collision diagnostic, got: {err}"
    );
}

#[test]
fn workflow_alias_duplicate_within_list_fails_at_parse() {
    // Same alias listed twice would also register the workflow twice
    // under that name. Reject so the bug surfaces at codegen.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package wf_alias_dup.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              aliases:    ["a", "b", "a"]
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .expect_err("duplicate alias in same list must be rejected at parse")
        .to_string();
    assert!(
        err.contains("more than once") && err.contains("\"a\""),
        "expected duplicate-alias diagnostic, got: {err}"
    );
}

#[test]
fn start_options_exposes_proto_defaults_constructor() {
    // R6 ergonomics — `<Wf>StartOptions::proto_defaults() -> Self`
    // returns the options struct with every proto-declared default
    // already filled in. Distinct from `Default::default()` which
    // leaves everything None. Lets callers spell:
    //     `MyOpts::proto_defaults().with_workflow_id("...")`
    // to start from the proto-baked baseline.
    // Only emitted when at least one `default_*` exists.
    let services = parse_and_validate("full_workflow");
    let opts_fixture = load_fixture_options("full_workflow");
    let source = render::render(&services[0], &opts_fixture);
    assert!(
        source.contains("pub fn proto_defaults() -> Self {"),
        "missing proto_defaults constructor: {source}"
    );
    assert!(
        source.contains("let mut opts = Self::default();"),
        "proto_defaults must start from Self::default(): {source}"
    );
    // Body should fold in at least one declared default.
    assert!(
        source.contains("Self::default_run_timeout()")
            || source.contains("Self::default_execution_timeout()")
            || source.contains("Self::default_task_timeout()")
            || source.contains("Self::default_id_reuse_policy()"),
        "proto_defaults body must reference at least one default_* fn: {source}"
    );
}

#[test]
fn start_options_exposes_is_empty_predicate() {
    // R6 ergonomics — `<Wf>StartOptions::is_empty(&self) -> bool`
    // returns true when no field is set. Lets callers detect the
    // "use proto-declared defaults for everything" state without
    // manually pattern-matching every Option field.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn is_empty(&self) -> bool {"),
        "missing is_empty predicate: {source}"
    );
    // Body must check all nine fields.
    for field in [
        "self.workflow_id.is_none()",
        "self.task_queue.is_none()",
        "self.id_reuse_policy.is_none()",
        "self.id_conflict_policy.is_none()",
        "self.execution_timeout.is_none()",
        "self.run_timeout.is_none()",
        "self.task_timeout.is_none()",
        "self.enable_eager_workflow_start.is_none()",
        "self.retry_policy.is_none()",
    ] {
        assert!(
            source.contains(field),
            "is_empty body missing `{field}`: {source}"
        );
    }
}

#[test]
fn start_options_exposes_merge_method() {
    // R6 ergonomics — `<Wf>StartOptions::merge(other)` layers two
    // option structs together with `other`'s `Some`-fields winning.
    // Lets callers fold env-driven overrides over a base config
    // without re-deriving each field manually.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn merge(mut self, other: Self) -> Self {"),
        "missing merge fn signature: {source}"
    );
    // Each field must be folded via `other.<f>.or(self.<f>)`.
    for field in [
        "workflow_id",
        "task_queue",
        "id_reuse_policy",
        "id_conflict_policy",
        "execution_timeout",
        "run_timeout",
        "task_timeout",
        "enable_eager_workflow_start",
        "retry_policy",
    ] {
        let line = format!("self.{field} = other.{field}.or(self.{field});");
        assert!(
            source.contains(&line),
            "merge body missing fold for `{field}`: {source}"
        );
    }
}

#[test]
fn start_options_exposes_with_field_builders() {
    // R6 ergonomics — `<Wf>StartOptions` gains `with_<field>`
    // builder-style setters complementing struct-init. Each takes the
    // bare type (not Option) and wraps in Some, returning Self for
    // chaining.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    for (sig, body) in [
        (
            "with_workflow_id(mut self, v: impl ::std::convert::Into<String>) -> Self",
            "self.workflow_id = Some(v.into());",
        ),
        (
            "with_task_queue(mut self, v: impl ::std::convert::Into<String>) -> Self",
            "self.task_queue = Some(v.into());",
        ),
        (
            "with_id_reuse_policy(mut self, v: temporal_runtime::WorkflowIdReusePolicy) -> Self",
            "self.id_reuse_policy = Some(v);",
        ),
        (
            "with_id_conflict_policy(mut self, v: temporal_runtime::WorkflowIdConflictPolicy) -> Self",
            "self.id_conflict_policy = Some(v);",
        ),
        (
            "with_execution_timeout(mut self, v: Duration) -> Self",
            "self.execution_timeout = Some(v);",
        ),
        (
            "with_run_timeout(mut self, v: Duration) -> Self",
            "self.run_timeout = Some(v);",
        ),
        (
            "with_task_timeout(mut self, v: Duration) -> Self",
            "self.task_timeout = Some(v);",
        ),
        (
            "with_enable_eager_workflow_start(mut self, v: bool) -> Self",
            "self.enable_eager_workflow_start = Some(v);",
        ),
        (
            "with_retry_policy(mut self, v: temporal_runtime::RetryPolicy) -> Self",
            "self.retry_policy = Some(v);",
        ),
    ] {
        assert!(
            source.contains(sig),
            "missing builder signature `{sig}` in: {source}"
        );
        assert!(
            source.contains(body),
            "missing builder body `{body}` in: {source}"
        );
    }
}

#[test]
fn client_struct_implements_display() {
    // R6 ergonomics — `<Service>Client` carries a manual `Display`
    // impl producing the fully-qualified service name (same as the
    // `FULLY_QUALIFIED_SERVICE_NAME` const). Lets `info!("starting
    // {client}")` produce a concise readable token.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("impl ::std::fmt::Display for JobServiceClient {"),
        "missing Display impl: {source}"
    );
    assert!(
        source.contains("f.write_str(Self::FULLY_QUALIFIED_SERVICE_NAME)"),
        "Display body must write the FQN const directly: {source}"
    );
}

#[test]
fn client_struct_implements_debug() {
    // R6 ergonomics — `<Service>Client` carries a manual `Debug`
    // impl that prints `package`, `service`, `plugin_version`
    // (`finish_non_exhaustive` since the inner client is opaque).
    // Lets `tracing::info!(?client, ...)` emit useful structured
    // output without dumping connection internals.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("impl ::std::fmt::Debug for JobServiceClient {"),
        "missing Debug impl: {source}"
    );
    assert!(
        source.contains(".field(\"package\", &Self::PACKAGE)"),
        "Debug impl must include package: {source}"
    );
    assert!(
        source.contains(".field(\"service\", &Self::SERVICE_NAME)"),
        "Debug impl must include service: {source}"
    );
    assert!(
        source.contains(".field(\"plugin_version\", &Self::GENERATED_BY_PLUGIN_VERSION)"),
        "Debug impl must include plugin_version: {source}"
    );
    assert!(
        source.contains(".finish_non_exhaustive()"),
        "Debug impl must use finish_non_exhaustive() since inner client is opaque: {source}"
    );
}

#[test]
fn handle_implements_from_workflow_handle_trait() {
    // R6 ergonomics — sugar over the explicit `from_inner`
    // constructor: `From<WorkflowHandle> for <Wf>Handle` lets
    // consumers spell `let h: MyHandle = bridge_h.into();` when
    // the destination type is inferred. The inherent `from_inner`
    // stays as the explicit named constructor.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains(
            "impl ::std::convert::From<temporal_runtime::WorkflowHandle> for RunJobHandle {"
        ),
        "missing From<WorkflowHandle> impl: {source}"
    );
    assert!(
        source.contains("Self::from_inner(inner)"),
        "From impl body must delegate to from_inner: {source}"
    );
}

#[test]
fn handle_exposes_from_inner_constructor() {
    // R6 ergonomics — `<Wf>Handle::from_inner(WorkflowHandle)` is
    // the inverse of `into_inner`. Lets test harnesses construct
    // a typed handle from a hand-built bridge handle (e.g. fake
    // handles for unit tests) without going through the typed
    // start path. With both directions in place the wrapper
    // round-trips: `Handle::from_inner(h.into_inner())` == h.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn from_inner(inner: temporal_runtime::WorkflowHandle) -> Self {"),
        "missing from_inner constructor: {source}"
    );
    assert!(
        source.contains("Self { inner }"),
        "from_inner body must wrap the bridge handle in the typed wrapper: {source}"
    );
}

#[test]
fn handle_exposes_clone_inner_accessor() {
    // R6 ergonomics — `<Wf>Handle::clone_inner(&self) ->
    // WorkflowHandle` parallels `<Service>Client::clone_inner`.
    // Lets callers obtain an owned bridge handle without
    // consuming the typed wrapper, useful for handing the bridge
    // handle to a custom polling loop while continuing to use
    // the typed surface.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn clone_inner(&self) -> temporal_runtime::WorkflowHandle {"),
        "missing clone_inner accessor on handle: {source}"
    );
    assert!(
        source.contains("self.inner.clone()"),
        "clone_inner body must clone the inner bridge handle: {source}"
    );
}

#[test]
fn handle_exposes_into_inner_consuming_accessor() {
    // R6 ergonomics — `<Wf>Handle::into_inner(self)` returns the
    // underlying `WorkflowHandle` by value, letting downstream
    // code use the bridge surface directly (e.g. custom polling
    // loops that don't fit the typed wrapper). Pairs with the
    // `<Service>Client::into_inner` shipment so both wrappers
    // expose the borrow + own duality.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn into_inner(self) -> temporal_runtime::WorkflowHandle {"),
        "missing handle into_inner consuming accessor: {source}"
    );
}

#[test]
fn handle_struct_implements_display() {
    // R6 ergonomics — `<Wf>Handle` carries a manual `Display` impl
    // producing a concise `<WorkflowName>(<workflow_id>)` form for
    // log lines like `info!("handling {handle}")` where the
    // structured Debug form would be too verbose.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("impl ::std::fmt::Display for RunJobHandle {"),
        "missing Display impl: {source}"
    );
    assert!(
        source.contains("write!(f, \"{}({})\", Self::WORKFLOW_NAME, self.inner.workflow_id())"),
        "Display body must format `<name>(<id>)` from the const + bridge accessor: {source}"
    );
}

#[test]
fn handle_struct_implements_debug() {
    // R6 ergonomics — `<Wf>Handle` carries a manual `Debug` impl
    // that prints `workflow_name`, `workflow_id`, `run_id`. Bridge
    // `WorkflowHandle` doesn't derive Debug (its inner SDK client
    // is opaque), so a derive is unavailable; the manual impl gives
    // logging frameworks a structured form.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("impl ::std::fmt::Debug for RunJobHandle {"),
        "missing Debug impl: {source}"
    );
    assert!(
        source.contains(".field(\"workflow_name\", &Self::WORKFLOW_NAME)"),
        "Debug impl must include workflow_name: {source}"
    );
    assert!(
        source.contains(".field(\"workflow_id\", &self.inner.workflow_id())"),
        "Debug impl must include workflow_id: {source}"
    );
    assert!(
        source.contains(".field(\"run_id\", &self.inner.run_id())"),
        "Debug impl must include run_id: {source}"
    );
}

#[test]
fn handle_struct_exposes_identity_consts() {
    // R4 — every `<Wf>Handle` struct now exposes inherent identity
    // consts (`WORKFLOW_NAME`, `INPUT_TYPE`, `OUTPUT_TYPE`,
    // `TASK_QUEUE` when declared) re-exposing the per-rpc
    // module-level consts. Lets diagnostic logging spell
    // `<MyHandle>::WORKFLOW_NAME` directly off the typed handle
    // without a bridge round-trip.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub const WORKFLOW_NAME: &'static str = self::RUN_JOB_WORKFLOW_NAME;"),
        "Handle must re-expose WORKFLOW_NAME: {source}"
    );
    assert!(
        source.contains("pub const INPUT_TYPE: &'static str = self::RUN_JOB_INPUT_TYPE;"),
        "Handle must re-expose INPUT_TYPE: {source}"
    );
    assert!(
        source.contains("pub const OUTPUT_TYPE: &'static str = self::RUN_JOB_OUTPUT_TYPE;"),
        "Handle must re-expose OUTPUT_TYPE: {source}"
    );
    assert!(
        source.contains("pub const TASK_QUEUE: &'static str = self::RUN_JOB_TASK_QUEUE;"),
        "Handle must re-expose TASK_QUEUE when declared: {source}"
    );
}

#[test]
fn cli_command_exposes_handler_name_accessor() {
    // R6 ergonomics — `Command::handler_name(&self) -> &'static str`
    // returns the registered (cross-language) name of the handler each
    // subcommand variant targets. Lets dispatch middleware tag tracing
    // spans / structured logs / metrics with the handler name without
    // pattern-matching every variant at the call site.
    //
    // The mapping is uniform: Start/Attach/Cancel/Terminate share the
    // workflow's name; Signal/Query/Update each return their own
    // handler's name.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package hn.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          option (temporal.v1.service) = { task_queue: "tq" };
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              signal: [{ ref: "Cancel" }]
              query:  [{ ref: "Status" }]
              update: [{ ref: "Touch" }]
            };
          }
          rpc Cancel(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
          rpc Status(google.protobuf.Empty) returns (StatusOutput) {
            option (temporal.v1.query) = {};
          }
          rpc Touch(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.update) = {};
          }
        }
        message In  {}
        message Out {}
        message StatusOutput { string phase = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    // Method signature.
    assert!(
        source.contains("pub fn handler_name(&self) -> &'static str {"),
        "missing handler_name fn signature: {source}"
    );
    // Workflow verbs share the workflow's registered name.
    for verb in ["Start", "Attach", "Cancel", "Terminate"] {
        let arm = format!("Self::{verb}Run(_) => \"hn.v1.Svc.Run\",");
        assert!(
            source.contains(&arm),
            "missing workflow-verb arm `{arm}`: {source}"
        );
    }
    // Per-handler arms each return their own registered name.
    for (variant, expected_name) in [
        ("Self::SignalCancel(_)", "hn.v1.Svc.Cancel"),
        ("Self::QueryStatus(_)", "hn.v1.Svc.Status"),
        ("Self::UpdateTouch(_)", "hn.v1.Svc.Touch"),
    ] {
        let arm = format!("{variant} => \"{expected_name}\",");
        assert!(
            source.contains(&arm),
            "missing handler arm `{arm}`: {source}"
        );
    }
}

#[test]
fn client_exposes_task_queues_aggregate_const() {
    // R6 ergonomics — `<Service>Client::TASK_QUEUES: &'static [&'static str]`
    // is the union of every distinct task queue used across the
    // service's workflows + activities, in declaration order. Lets
    // worker setup validate "I'm configured for every queue this
    // service needs" via:
    //     for q in MyClient::TASK_QUEUES { assert!(workers.contains(q)); }
    // Distinct from `DEFAULT_TASK_QUEUE` (just the service-level
    // fallback) and from per-rpc `<RPC>_TASK_QUEUE` (one queue per
    // workflow). This is the deduped union — handy when activities
    // override their own queue and you need to start workers on
    // multiple queues.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package tq.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          option (temporal.v1.service) = { task_queue: "default-tq" };
          rpc Alpha(In) returns (Out) {
            option (temporal.v1.workflow) = {};
          }
          rpc Beta(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "beta-tq" };
          }
          rpc Gamma(In) returns (Out) {
            option (temporal.v1.activity) = {
              start_to_close_timeout: { seconds: 30 }
              task_queue: "gamma-tq"
            };
          }
          rpc Delta(In) returns (Out) {
            option (temporal.v1.activity) = {
              start_to_close_timeout: { seconds: 30 }
            };
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let source = render::render(&services[0], &Default::default());
    // Alpha resolves to service-default "default-tq".
    // Beta overrides to "beta-tq".
    // Gamma activity declares its own "gamma-tq".
    // Delta activity has no task_queue declaration (inherits at runtime).
    // Order: declaration order, deduped.
    assert!(
        source.contains(
            "pub const TASK_QUEUES: &'static [&'static str] = &[\"default-tq\", \"beta-tq\", \"gamma-tq\"];"
        ),
        "TASK_QUEUES must dedupe and follow declaration order: {source}"
    );
}

#[test]
fn client_omits_task_queues_const_when_empty() {
    // Skip-emit guard: when no workflow declares (or inherits) a queue
    // and no activity overrides one, the union is empty and the const
    // must not emit. Construct an activities-only service where the
    // activity declares no task_queue and the service has no default —
    // the union is empty.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package no_tq.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc DoWork(In) returns (Out) {
            option (temporal.v1.activity) = {
              start_to_close_timeout: { seconds: 30 }
            };
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let source = render::render(&services[0], &Default::default());
    assert!(
        !source.contains("pub const TASK_QUEUES:"),
        "TASK_QUEUES must omit when union is empty: {source}"
    );
}

#[test]
fn cli_command_exposes_verb_accessor() {
    // R6 ergonomics — `Command::verb(&self) -> &'static str` is the
    // action-side counterpart of `handler_name()`. Returns one of
    // `start` / `attach` / `cancel` / `terminate` / `signal` / `query`
    // / `update` classifying the subcommand independently of the
    // target handler. Together `(verb, handler_name)` is the full
    // dispatch tuple — useful for tagging tracing spans / metrics
    // labels with two clean dimensions instead of one composite.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package vbs.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          option (temporal.v1.service) = { task_queue: "tq" };
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              signal: [{ ref: "Cancel" }]
              query:  [{ ref: "Status" }]
              update: [{ ref: "Touch" }]
            };
          }
          rpc Cancel(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
          rpc Status(google.protobuf.Empty) returns (StatusOutput) {
            option (temporal.v1.query) = {};
          }
          rpc Touch(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.update) = {};
          }
        }
        message In  {}
        message Out {}
        message StatusOutput { string phase = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("pub fn verb(&self) -> &'static str {"),
        "missing verb fn signature: {source}"
    );
    // Each workflow verb arm.
    for (variant, expected_verb) in [
        ("Self::StartRun(_)", "start"),
        ("Self::AttachRun(_)", "attach"),
        ("Self::CancelRun(_)", "cancel"),
        ("Self::TerminateRun(_)", "terminate"),
    ] {
        let arm = format!("{variant} => \"{expected_verb}\",");
        assert!(
            source.contains(&arm),
            "missing workflow-verb arm `{arm}`: {source}"
        );
    }
    // Per-handler-kind arms.
    for (variant, expected_verb) in [
        ("Self::SignalCancel(_)", "signal"),
        ("Self::QueryStatus(_)", "query"),
        ("Self::UpdateTouch(_)", "update"),
    ] {
        let arm = format!("{variant} => \"{expected_verb}\",");
        assert!(
            source.contains(&arm),
            "missing handler arm `{arm}`: {source}"
        );
    }
}

#[test]
fn cli_command_handler_name_skipped_when_no_subcommand_variants() {
    // Skip-emit guard: when the service has no usable workflows AND no
    // signals/queries/updates, the Command enum has no variants and
    // `handler_name` would have no match arms (an empty match is
    // unreachable but rustc still rejects the function signature
    // returning `&'static str` from `match self {}` since `self: &!`
    // doesn't apply — this also keeps the surface clean).
    //
    // Construct an activities-only service: activities don't generate
    // CLI subcommands, so the Command enum stays empty.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package hn_empty.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc DoWork(In) returns (Out) {
            option (temporal.v1.activity) = { start_to_close_timeout: { seconds: 30 } };
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        !source.contains("pub fn handler_name(&self) -> &'static str"),
        "no handler_name fn should emit when Command enum is empty: {source}"
    );
}

#[test]
fn update_default_wait_policy_helper_emits_when_proto_declares_it() {
    // R6 ergonomics — `<update>_default_wait_policy() -> WaitPolicy` is
    // a module-level static accessor parallel to
    // `<Wf>StartOptions::default_id_reuse_policy()` and
    // `<wf>_default_child_options()`. Lets callers opt into the
    // proto-declared default explicitly:
    //     handle.<update>(input, Some(<update>_default_wait_policy())).await
    // instead of relying on inline call-site folding (which still
    // happens; the helper just exposes the value as a discoverable
    // static). Skip-emit when proto omits a default so the surface
    // doesn't grow noise functions returning the SDK's hard-coded
    // fallback. Test exercises both arms.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package wp.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [
                { ref: "WithDefault" },
                { ref: "WithoutDefault" }
              ]
            };
          }
          rpc WithDefault(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.update) = { wait_for_stage: WAIT_POLICY_ACCEPTED };
          }
          rpc WithoutDefault(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.update) = {};
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let source = render::render(&services[0], &Default::default());

    // Declared default ⇒ helper emits, returning the matching variant.
    assert!(
        source.contains(
            "pub fn with_default_default_wait_policy() -> temporal_runtime::WaitPolicy { temporal_runtime::WaitPolicy::Accepted }"
        ),
        "expected with_default_default_wait_policy() helper returning WaitPolicy::Accepted: {source}"
    );

    // Proto omits a default ⇒ no helper emits. Mirrors the skip-emit
    // policy on `<Wf>StartOptions::default_*` helpers.
    assert!(
        !source.contains("pub fn without_default_default_wait_policy"),
        "no helper should emit for an update without a declared wait_policy default: {source}"
    );
}

#[test]
fn handle_struct_re_exposes_workflow_aliases_const_when_declared() {
    // R6 ergonomics — `WORKFLOW_ALIASES` was previously only on the
    // Definition trait, forcing diagnostic code that wanted to enumerate
    // a workflow's aliases (e.g. for compat-name logging during a
    // rename) to drag in the trait. Now also re-exposed inherently on
    // the `<Wf>Handle`. The `worker_workflow_aliases` fixture declares
    // aliases on its Run workflow and is the only render fixture that
    // exercises the alias path.
    let services = parse_and_validate("worker_workflow_aliases");
    let opts = load_fixture_options("worker_workflow_aliases");
    let source = render::render(&services[0], &opts);
    // Scope to the Handle's inherent impl — the same const is also
    // emitted on the child-workflow marker (covered by a sibling test
    // below).
    let handle_block_start = source
        .find("impl RunHandle {")
        .expect("RunHandle inherent impl present");
    let after_block = &source[handle_block_start..];
    let block_end = after_block.find("\n    }\n").expect("inherent impl closer");
    let block = &after_block[..block_end];
    assert!(
        block.contains(
            "pub const WORKFLOW_ALIASES: &'static [&'static str] = self::RUN_WORKFLOW_ALIASES;"
        ),
        "Handle must re-expose WORKFLOW_ALIASES when declared: {block}"
    );
}

#[test]
fn handle_struct_omits_workflow_aliases_const_when_not_declared() {
    // Skip-guard parity with the existing module-const emit. Most
    // workflows declare no aliases; emitting `&[]` would mislead.
    let services = parse_and_validate("workflow_only");
    let source = render::render(&services[0], &Default::default());
    assert!(
        !source.contains("pub const WORKFLOW_ALIASES:"),
        "Handle must omit WORKFLOW_ALIASES when no aliases declared: {source}"
    );
}

#[test]
fn child_workflow_marker_re_exposes_workflow_aliases_const_when_declared() {
    // Parallel parity ship on the child-workflow marker. Lets generic
    // worker code holding a `<W>Workflow` marker enumerate aliases via
    // `<W>::WORKFLOW_ALIASES` without dragging in the Definition trait.
    let services = parse_and_validate("worker_workflow_aliases");
    let opts = load_fixture_options("worker_workflow_aliases");
    let source = render::render(&services[0], &opts);
    let marker_block_start = source
        .find("impl RunWorkflow {")
        .expect("RunWorkflow inherent impl present");
    let after_block = &source[marker_block_start..];
    let block_end = after_block.find("\n    }\n").expect("inherent impl closer");
    let block = &after_block[..block_end];
    assert!(
        block.contains(
            "pub const WORKFLOW_ALIASES: &'static [&'static str] = self::RUN_WORKFLOW_ALIASES;"
        ),
        "child-workflow marker must re-expose WORKFLOW_ALIASES when declared: {block}"
    );
}

#[test]
fn handle_struct_re_exposes_id_template_const_when_declared() {
    // R6 ergonomics — completes the identity-const matrix on the Handle.
    // `WORKFLOW_NAME` / `INPUT_TYPE` / `OUTPUT_TYPE` / `TASK_QUEUE` are
    // already re-exposed; ID_TEMPLATE was previously only on the
    // Definition trait. Now also on the inherent Handle impl when the
    // workflow declares an id template — useful when diagnostic code
    // wants to log "this handle's workflow_id was derived from template
    // `…`" without a trait import dance.
    let services = parse_and_validate("full_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub const ID_TEMPLATE: &'static str = self::RUN_WORKFLOW_ID_TEMPLATE;"),
        "Handle must re-expose ID_TEMPLATE when workflow declares one: {source}"
    );
}

#[test]
fn handle_struct_omits_id_template_const_when_not_declared() {
    // Skip-guard parity with the existing Definition-trait emit. Most
    // workflows declare no id template (the runtime synthesizes a
    // UUID), and emitting `ID_TEMPLATE: ""` would mislead diagnostic
    // code into thinking a template existed.
    let services = parse_and_validate("workflow_only");
    let source = render::render(&services[0], &Default::default());
    assert!(
        !source.contains("pub const ID_TEMPLATE: &'static str"),
        "Handle must omit ID_TEMPLATE when workflow declares none: {source}"
    );
}

#[test]
fn handle_exposes_set_run_id_mutating_setter() {
    // R6 ergonomics — `<Wf>Handle::set_run_id(&mut self,
    // Option<String>)` is the mutating alternative to the
    // consuming `with_run_id`. Lets callers update a handle
    // stored in a struct field without re-binding via
    // take/replace. Uses `clone() + with_run_id` round-trip
    // (cheap — Arc-backed bridge handle).
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn set_run_id(&mut self, run_id: Option<String>) {"),
        "missing set_run_id mutating setter: {source}"
    );
    assert!(
        source.contains("self.inner = self.inner.clone().with_run_id(run_id);"),
        "set_run_id body must clone+with_run_id+assign: {source}"
    );
}

#[test]
fn handle_exposes_without_run_id_convenience() {
    // R6 ergonomics — `<Wf>Handle::without_run_id(self) -> Self`
    // is sugar over `with_run_id(None)`. Lets callers transition
    // a handle from a specific historical run to "latest" semantics
    // without spelling the `Option::None` literal.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn without_run_id(self) -> Self {"),
        "missing without_run_id convenience: {source}"
    );
    assert!(
        source.contains("self.with_run_id(None)"),
        "without_run_id body must delegate to with_run_id(None): {source}"
    );
}

#[test]
fn handle_exposes_with_run_id_consuming_builder() {
    // R6 ergonomics — `<Wf>Handle::with_run_id(self, Option<String>)
    // -> Self` lets callers branch from a current handle to a known
    // historical execution while keeping the same workflow_id
    // binding. Common in audit/debug paths. Bridge gained the
    // matching consuming builder; the typed wrapper passes through.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn with_run_id(self, run_id: Option<String>) -> Self {"),
        "missing with_run_id consuming builder: {source}"
    );
    assert!(
        source.contains("Self { inner: self.inner.with_run_id(run_id) }"),
        "with_run_id body must passthrough to bridge: {source}"
    );
}

#[test]
fn handle_exposes_client_passthrough() {
    // R6 ergonomics — `<Wf>Handle::client(&self) -> &TemporalClient`
    // borrows the bound bridge client. Lets callers construct
    // sibling handles on the same client without round-tripping
    // through the typed `<Service>Client` (or storing it
    // separately). Bridge gained the matching `client()`
    // accessor; the typed wrapper passes through.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn client(&self) -> &temporal_runtime::TemporalClient {"),
        "missing client passthrough on handle: {source}"
    );
    assert!(
        source.contains("self.inner.client()"),
        "client passthrough must call through to bridge handle: {source}"
    );
}

#[test]
fn handle_exposes_same_workflow_as_helper() {
    // R6 ergonomics — `<Wf>Handle::same_workflow_as(&other)`
    // compares two handles by workflow_id only (ignoring run_id).
    // Useful for deduplication in handle collections where one
    // subsystem may have a start-path handle (run_id known) and
    // another may have an attach handle (run_id `None`) for the
    // same logical workflow.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn same_workflow_as(&self, other: &Self) -> bool {"),
        "missing same_workflow_as comparison: {source}"
    );
    assert!(
        source.contains("self.inner.workflow_id() == other.inner.workflow_id()"),
        "comparison body must check workflow_id equality: {source}"
    );
}

#[test]
fn handle_exposes_same_execution_as_strict_equality() {
    // R6 ergonomics — `<Wf>Handle::same_execution_as(&other)` is the
    // strict-equality sibling of `same_workflow_as`. It returns true
    // IFF both handles carry a known run id, the run ids match, and
    // the workflow ids match. Distinguishes "same Temporal execution"
    // from "same workflow id, possibly different run" — continue-as-
    // new produces a new run id under the same workflow id, and
    // confusing the two would silently mask continue-as-new bugs.
    // Returns false when either side lacks a run id (proof requires
    // a run id; absence of one is not proof).
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn same_execution_as(&self, other: &Self) -> bool {"),
        "missing same_execution_as comparison: {source}"
    );
    // Body must match on both sides' run_id Options. The (Some, Some)
    // arm is the only one that should evaluate to true; everything
    // else must fall through to false.
    assert!(
        source.contains("match (self.inner.run_id(), other.inner.run_id()) {"),
        "must pattern-match both run_id Options: {source}"
    );
    assert!(
        source.contains(
            "(Some(a), Some(b)) => a == b && self.inner.workflow_id() == other.inner.workflow_id(),"
        ),
        "Some/Some arm must compare run ids AND workflow ids: {source}"
    );
    assert!(
        source.contains("_ => false,"),
        "fallthrough arm must be false (no run id ⇒ no proof): {source}"
    );
}

#[test]
fn handle_exposes_run_id_owned_accessor() {
    // R6 ergonomics — `<Wf>Handle::run_id_owned()` returns
    // `Option<String>`, the owned-string parallel of the
    // borrowing `run_id() -> Option<&str>` accessor. Useful when
    // the optional id needs to outlive the borrow.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn run_id_owned(&self) -> Option<String> {"),
        "missing run_id_owned accessor: {source}"
    );
    assert!(
        source.contains("self.inner.run_id().map(String::from)"),
        "run_id_owned must map the optional &str through String::from: {source}"
    );
}

#[test]
fn handle_exposes_workflow_id_owned_accessor() {
    // R6 ergonomics — `<Wf>Handle::workflow_id_owned()` returns
    // an owned `String` to save the `.to_string()` ceremony at
    // call sites that need to store the id in a struct, send
    // across a channel, or pass to APIs that take `String` by
    // value. Pairs with the existing `workflow_id() -> &str`
    // borrowing accessor.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn workflow_id_owned(&self) -> String {"),
        "missing workflow_id_owned accessor: {source}"
    );
    assert!(
        source.contains("self.inner.workflow_id().to_string()"),
        "workflow_id_owned must clone the bridge str: {source}"
    );
}

#[test]
fn handle_exposes_has_run_id_predicate() {
    // R6 ergonomics — `<Wf>Handle::has_run_id()` is a cheap
    // predicate over `self.inner.run_id().is_some()` letting
    // diagnostic logging branch on whether a handle was returned
    // by the typed start path (run_id known) vs constructed via
    // attach (run_id `None`). Sugar over the existing
    // `.run_id().is_some()` chain.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn has_run_id(&self) -> bool {"),
        "missing has_run_id predicate: {source}"
    );
    assert!(
        source.contains("self.inner.run_id().is_some()"),
        "has_run_id body must check inner.run_id().is_some(): {source}"
    );
}

#[test]
fn handle_struct_derives_clone() {
    // R6 ergonomics — `<Wf>Handle` derives Clone. Free since the
    // bridge `WorkflowHandle` is itself Clone (Arc-backed
    // `TemporalClient` + short id strings). Lets callers share
    // the typed handle across tasks without `Arc<Handle>` wrapping.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("#[derive(Clone)]\n    pub struct RunJobHandle {"),
        "Handle struct must derive Clone: {source}"
    );
}

#[test]
fn client_exposes_random_workflow_id_static_helper() {
    // R6 ergonomics — `<Service>Client::random_workflow_id() ->
    // String` is a static convenience over the bridge's
    // `random_workflow_id()` UUID generator. Saves a
    // `temporal_runtime::random_workflow_id()` import at call
    // sites that already have the typed client in scope (most
    // common: tests + ad-hoc CLI tooling).
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn random_workflow_id() -> String {"),
        "missing random_workflow_id static helper: {source}"
    );
    assert!(
        source.contains("temporal_runtime::random_workflow_id()"),
        "helper body must call through to the bridge: {source}"
    );
}

#[test]
fn client_exposes_namespace_passthrough() {
    // R6 ergonomics — `<Service>Client::namespace()` returns the
    // Temporal namespace the client is bound to. Saves an
    // `inner().namespace()` chain at call sites that want to
    // log or report the active namespace. SDK returns owned
    // `String`; we mirror that signature.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn namespace(&self) -> String {"),
        "missing namespace passthrough: {source}"
    );
    assert!(
        source.contains("self.client.namespace()"),
        "namespace body must call through to the bridge: {source}"
    );
}

#[test]
fn client_struct_derives_clone() {
    // R6 ergonomics — `<Service>Client` derives Clone. Free since
    // the bridge's `TemporalClient` is `Arc`-backed and derives
    // Clone — cloning the wrapper bumps a refcount, no
    // re-connection. Lets callers freely share the typed client
    // across tasks (`tokio::spawn(async move { svc.run(...) })`),
    // channels, and worker pools.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("#[derive(Clone)]\n    pub struct JobServiceClient {"),
        "Client struct must derive Clone: {source}"
    );
}

#[test]
fn client_implements_from_temporal_client_trait() {
    // R6 ergonomics — sugar over `<Service>Client::new`:
    // `From<TemporalClient> for <Service>Client` lets consumers
    // spell `let svc: MyClient = bridge.into();`. Mirrors the
    // `<Wf>Handle` From shipment so both wrappers expose the
    // trait duality (`From<Bridge>` + `Into<Bridge>` via
    // `into_inner`).
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains(
            "impl ::std::convert::From<temporal_runtime::TemporalClient> for JobServiceClient {"
        ),
        "missing From<TemporalClient> impl: {source}"
    );
    assert!(
        source.contains("Self::new(client)"),
        "From impl body must delegate to Self::new: {source}"
    );
}

#[test]
fn client_exposes_clone_inner_accessor() {
    // R6 ergonomics — `<Service>Client::clone_inner(&self) ->
    // TemporalClient` is sugar over `.inner().clone()`. Lets
    // callers obtain an owned client without consuming the
    // wrapper, useful when the wrapper is borrowed and we want
    // to spawn a sibling `<X>Client` without transferring
    // ownership.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn clone_inner(&self) -> temporal_runtime::TemporalClient {"),
        "missing clone_inner accessor: {source}"
    );
    assert!(
        source.contains("self.client.clone()"),
        "clone_inner body must clone the inner client: {source}"
    );
}

#[test]
fn client_exposes_into_inner_consuming_accessor() {
    // R6 ergonomics — `<Service>Client::into_inner(self)` returns
    // the underlying `TemporalClient` by value. Lets callers
    // transfer ownership for sharing across multiple typed
    // service clients (e.g. wrap the same connection in both an
    // `<A>Client` and a `<B>Client`). Pairs with the existing
    // `inner(&self) -> &TemporalClient` borrowing accessor.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn into_inner(self) -> temporal_runtime::TemporalClient {"),
        "missing into_inner consuming accessor: {source}"
    );
    assert!(
        source.contains("self.client") && source.contains("into_inner"),
        "into_inner body must return self.client by value: {source}"
    );
}

#[test]
fn client_exposes_connect_convenience_constructor() {
    // R6 ergonomics — `<Service>Client::connect(url, namespace)`
    // wraps `temporal_runtime::connect()` + `Self::new()` in one
    // call. Lets `main` skip the explicit two-step setup.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub async fn connect(url: &str, namespace: &str) -> Result<Self>"),
        "missing connect convenience constructor: {source}"
    );
    assert!(
        source.contains("let client = temporal_runtime::connect(url, namespace).await?;"),
        "connect must call temporal_runtime::connect: {source}"
    );
    assert!(
        source.contains("Ok(Self::new(client))"),
        "connect must wrap via Self::new: {source}"
    );
}

#[test]
fn client_exposes_plugin_version_const() {
    // R4 — `<Service>Client::GENERATED_BY_PLUGIN_VERSION: &'static str`
    // embeds the protoc-gen-rust-temporal version that produced the
    // file at codegen time. Forensic tooling (debugging "code doesn't
    // compile, must be a generator bug" reports) reads this to
    // identify the responsible plugin release without needing the
    // build environment.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains(
            "pub const GENERATED_BY_PLUGIN_VERSION: &'static str = \"protoc-gen-rust-temporal "
        ),
        "GENERATED_BY_PLUGIN_VERSION const missing or wrong: {source}"
    );
}

#[test]
fn client_exposes_source_file_const() {
    // R4 — `<Service>Client::SOURCE_FILE: &'static str` carries the
    // proto file path as protoc saw it. Lets tooling correlate
    // generated code back to the input proto without parsing build
    // outputs.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    let svc = &services[0];
    let expected = format!(
        "pub const SOURCE_FILE: &'static str = \"{}\";",
        svc.source_file
    );
    assert!(
        source.contains(&expected),
        "SOURCE_FILE const missing or wrong (expected {expected:?}): {source}"
    );
}

#[test]
fn activity_task_queue_const_emits_when_declared() {
    // R4 — `<RPC>_ACTIVITY_TASK_QUEUE: &str` emits per activity that
    // declares `(temporal.v1.activity).task_queue`. Activities that
    // omit it produce no const (mirrors the workflow-side behaviour
    // where `<RPC>_TASK_QUEUE` only emits when the workflow or
    // service declares one).
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package atq.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc DoWorkA(In) returns (Out) {
            option (temporal.v1.activity) = {
              task_queue: "specialised-queue"
              start_to_close_timeout: { seconds: 30 }
            };
          }
          rpc DoWorkB(In) returns (Out) {
            option (temporal.v1.activity) = {
              start_to_close_timeout: { seconds: 30 }
            };
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub const DO_WORK_A_ACTIVITY_TASK_QUEUE: &str = \"specialised-queue\";"),
        "activity task_queue const missing when declared: {source}"
    );
    assert!(
        !source.contains("DO_WORK_B_ACTIVITY_TASK_QUEUE"),
        "activity task_queue const must NOT emit when not declared: {source}"
    );
}

#[test]
fn client_exposes_service_identity_consts() {
    // R4 — `<Service>Client` carries `PACKAGE`, `SERVICE_NAME`, and
    // `FULLY_QUALIFIED_SERVICE_NAME` consts so tooling that needs the
    // proto namespace at runtime can read them directly instead of
    // re-parsing import paths.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    let svc = &services[0];
    assert!(
        source.contains(&format!(
            "pub const PACKAGE: &'static str = \"{}\";",
            svc.package
        )),
        "PACKAGE const missing: {source}"
    );
    assert!(
        source.contains(&format!(
            "pub const SERVICE_NAME: &'static str = \"{}\";",
            svc.service
        )),
        "SERVICE_NAME const missing: {source}"
    );
    let fqn = format!("{}.{}", svc.package, svc.service);
    assert!(
        source.contains(&format!(
            "pub const FULLY_QUALIFIED_SERVICE_NAME: &'static str = \"{fqn}\";"
        )),
        "FULLY_QUALIFIED_SERVICE_NAME const missing: {source}"
    );
}

#[test]
fn handler_input_output_type_consts_emit_for_all_rpc_kinds() {
    // R4 — per-rpc `_INPUT_TYPE` / `_OUTPUT_TYPE` consts emit for
    // signals, queries, updates, and activities (parallel of the
    // workflow consts). Signal outputs are always `Empty` so we only
    // emit the input const. Activities emit both.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package types.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Cancel" }]
              query:  [{ ref: "Status" }]
              update: [{ ref: "Touch" }]
            };
          }
          rpc Cancel(CancelInput) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
          rpc Status(google.protobuf.Empty) returns (StatusOutput) {
            option (temporal.v1.query) = {};
          }
          rpc Touch(TouchInput) returns (TouchOutput) {
            option (temporal.v1.update) = {};
          }
          rpc DoWork(WorkInput) returns (WorkOutput) {
            option (temporal.v1.activity) = { start_to_close_timeout: { seconds: 30 } };
          }
        }
        message In  {}
        message Out {}
        message CancelInput { string reason = 1; }
        message StatusOutput { string phase = 1; }
        message TouchInput { string key = 1; }
        message TouchOutput { uint64 next = 1; }
        message WorkInput { string id = 1; }
        message WorkOutput { string result = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub const CANCEL_SIGNAL_INPUT_TYPE: &str = \"types.v1.CancelInput\";"),
        "signal input type const missing: {source}"
    );
    assert!(
        source.contains("pub const STATUS_QUERY_INPUT_TYPE: &str = \"google.protobuf.Empty\";"),
        "Empty-input query const must use canonical Empty FQN: {source}"
    );
    assert!(
        source.contains("pub const STATUS_QUERY_OUTPUT_TYPE: &str = \"types.v1.StatusOutput\";"),
        "query output type const missing: {source}"
    );
    assert!(
        source.contains("pub const TOUCH_UPDATE_INPUT_TYPE: &str = \"types.v1.TouchInput\";"),
        "update input type const missing: {source}"
    );
    assert!(
        source.contains("pub const TOUCH_UPDATE_OUTPUT_TYPE: &str = \"types.v1.TouchOutput\";"),
        "update output type const missing: {source}"
    );
    assert!(
        source.contains("pub const DO_WORK_ACTIVITY_INPUT_TYPE: &str = \"types.v1.WorkInput\";"),
        "activity input type const missing: {source}"
    );
    assert!(
        source.contains("pub const DO_WORK_ACTIVITY_OUTPUT_TYPE: &str = \"types.v1.WorkOutput\";"),
        "activity output type const missing: {source}"
    );
}

#[test]
fn workflow_input_output_type_consts_emit() {
    // Per-workflow `<RPC>_INPUT_TYPE` / `<RPC>_OUTPUT_TYPE` consts
    // carry the fully-qualified proto type name so consumer tooling
    // can route payloads without re-traversing the descriptor pool.
    // Empty sides land as `"google.protobuf.Empty"`.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package iot.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "tq" };
          }
          rpc EmptyIn(google.protobuf.Empty) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "tq" };
          }
          rpc EmptyOut(In) returns (google.protobuf.Empty) {
            option (temporal.v1.workflow) = { task_queue: "tq" };
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub const RUN_INPUT_TYPE: &str = \"iot.v1.In\";"),
        "RUN_INPUT_TYPE const missing: {source}"
    );
    assert!(
        source.contains("pub const RUN_OUTPUT_TYPE: &str = \"iot.v1.Out\";"),
        "RUN_OUTPUT_TYPE const missing: {source}"
    );
    assert!(
        source.contains("pub const EMPTY_IN_INPUT_TYPE: &str = \"google.protobuf.Empty\";"),
        "Empty-input type const must use canonical Empty FQN: {source}"
    );
    assert!(
        source.contains("pub const EMPTY_OUT_OUTPUT_TYPE: &str = \"google.protobuf.Empty\";"),
        "Empty-output type const must use canonical Empty FQN: {source}"
    );
}

#[test]
fn client_exposes_service_level_name_aggregates() {
    // R4 — `<Service>Client` exposes `WORKFLOW_NAMES` / `SIGNAL_NAMES`
    // / `QUERY_NAMES` / `UPDATE_NAMES` / `ACTIVITY_NAMES` aggregate
    // `&'static [&'static str]` consts so tooling can enumerate
    // every name a generated service registers without reproducing
    // the snake-case + default-name resolution logic the plugin does
    // at codegen. Each const only emits when the corresponding kind
    // is non-empty.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package agg.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Cancel" }]
              query:  [{ ref: "Status" }]
              update: [{ ref: "Touch" }]
            };
          }
          rpc Cancel(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
          rpc Status(google.protobuf.Empty) returns (StatusOutput) {
            option (temporal.v1.query) = {};
          }
          rpc Touch(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.update) = {};
          }
          rpc DoWork(In) returns (Out) {
            option (temporal.v1.activity) = { start_to_close_timeout: { seconds: 30 } };
          }
        }
        message In  {}
        message Out {}
        message StatusOutput { string phase = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source
            .contains("pub const WORKFLOW_NAMES: &'static [&'static str] = &[\"agg.v1.Svc.Run\"];"),
        "WORKFLOW_NAMES const missing: {source}"
    );
    assert!(
        source.contains(
            "pub const SIGNAL_NAMES: &'static [&'static str] = &[\"agg.v1.Svc.Cancel\"];"
        ),
        "SIGNAL_NAMES const missing: {source}"
    );
    assert!(
        source
            .contains("pub const QUERY_NAMES: &'static [&'static str] = &[\"agg.v1.Svc.Status\"];"),
        "QUERY_NAMES const missing: {source}"
    );
    assert!(
        source
            .contains("pub const UPDATE_NAMES: &'static [&'static str] = &[\"agg.v1.Svc.Touch\"];"),
        "UPDATE_NAMES const missing: {source}"
    );
    assert!(
        source.contains(
            "pub const ACTIVITY_NAMES: &'static [&'static str] = &[\"agg.v1.Svc.DoWork\"];"
        ),
        "ACTIVITY_NAMES const missing: {source}"
    );
    // Aggregate-of-aggregates: ALL_HANDLER_NAMES must list every
    // registered name in WF / SIG / QUERY / UPDATE / ACT order. Lets
    // tooling spell `MyClient::ALL_HANDLER_NAMES` once instead of
    // concatenating five per-kind consts at the call site. Order
    // matches the per-kind emit order (workflows first, activities
    // last) so callers can rely on it.
    assert!(
        source.contains(
            "pub const ALL_HANDLER_NAMES: &'static [&'static str] = &[\"agg.v1.Svc.Run\", \"agg.v1.Svc.Cancel\", \"agg.v1.Svc.Status\", \"agg.v1.Svc.Touch\", \"agg.v1.Svc.DoWork\"];"
        ),
        "ALL_HANDLER_NAMES aggregate const missing or out of order: {source}"
    );
}

#[test]
fn client_exposes_service_default_task_queue_const_when_declared() {
    // R6 ergonomics — when a service declares a default task queue at
    // `(temporal.v1.service).task_queue`, the generated `<Service>Client`
    // exposes it as a `pub const DEFAULT_TASK_QUEUE: &'static str` so
    // worker setup can spell `Worker::new(MyClient::DEFAULT_TASK_QUEUE)`
    // without picking an arbitrary workflow rpc to read it from.
    // Distinct from each per-workflow `<RPC>_TASK_QUEUE` const, which
    // is the *effective* resolved queue (workflow override OR this
    // service-level fallback).
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package svc_tq.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          option (temporal.v1.service) = { task_queue: "service-default-tq" };
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {};
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub const DEFAULT_TASK_QUEUE: &'static str = \"service-default-tq\";"),
        "DEFAULT_TASK_QUEUE const must emit when the service declares one: {source}"
    );
}

#[test]
fn client_omits_default_task_queue_const_when_service_lacks_one() {
    // Mirror skip-guard: when no service-level task queue is declared
    // (each workflow carries its own), the const must NOT emit. Empty
    // string would be a footgun (`Worker::new("")` looks legal until
    // it isn't), so silence is the only correct answer.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package no_svc_tq.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "wf-only-tq" };
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let source = render::render(&services[0], &Default::default());
    assert!(
        !source.contains("pub const DEFAULT_TASK_QUEUE"),
        "DEFAULT_TASK_QUEUE const must not emit when service lacks default: {source}"
    );
}

#[test]
fn workflow_only_service_emits_all_handler_names_with_just_workflows() {
    // The aggregate `ALL_HANDLER_NAMES` is the union of every per-kind
    // aggregate. When a service declares only workflows (no signals /
    // queries / updates / activities), the aggregate must contain
    // exactly the workflow names — not `&[]`, not a synthetic placeholder.
    // Locks the property that the concatenation respects each per-kind
    // list's individual skip-guard (empty kinds contribute nothing
    // rather than a stray "" entry).
    let services = parse_and_validate("workflow_only");
    let source = render::render(&services[0], &Default::default());
    // The workflow_only fixture declares one workflow rpc `Run`.
    assert!(
        source.contains("pub const ALL_HANDLER_NAMES: &'static [&'static str] = &["),
        "workflow_only must still emit ALL_HANDLER_NAMES: {source}"
    );
    // Aggregate must not double-list the workflow (regression against a
    // bug where an empty-kind concat could repeat the previous list's
    // last element).
    let line = source
        .lines()
        .find(|l| l.contains("ALL_HANDLER_NAMES"))
        .expect("ALL_HANDLER_NAMES line present");
    let comma_count = line.matches(',').count();
    // One workflow ⇒ zero commas inside the array literal.
    assert_eq!(
        comma_count, 0,
        "workflow_only ALL_HANDLER_NAMES should contain exactly one entry, got: {line}"
    );
}

#[test]
fn workflow_only_service_omits_empty_aggregates() {
    // A workflow-only service must NOT emit `SIGNAL_NAMES` /
    // `QUERY_NAMES` / `UPDATE_NAMES` / `ACTIVITY_NAMES` (no empty
    // consts).
    let services = parse_and_validate("workflow_only");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub const WORKFLOW_NAMES:"),
        "workflow_only must still emit WORKFLOW_NAMES: {source}"
    );
    assert!(
        !source.contains("SIGNAL_NAMES:"),
        "must not emit empty SIGNAL_NAMES: {source}"
    );
    assert!(
        !source.contains("QUERY_NAMES:"),
        "must not emit empty QUERY_NAMES: {source}"
    );
    assert!(
        !source.contains("UPDATE_NAMES:"),
        "must not emit empty UPDATE_NAMES: {source}"
    );
    assert!(
        !source.contains("ACTIVITY_NAMES:"),
        "must not emit empty ACTIVITY_NAMES: {source}"
    );
}

#[test]
fn query_options_cli_threads_into_subcommand() {
    // R6 — method-level `(temporal.v1.query).cli` overrides flow into
    // the `Query<Name>` clap subcommand's `#[command(name, alias,
    // about)]`. Queries have no per-ref `cli` field, so this is the
    // only override path.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package qry_cli.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              query: [{ ref: "Status" }]
            };
          }
          rpc Status(google.protobuf.Empty) returns (StatusOutput) {
            option (temporal.v1.query) = {
              cli: { name: "show", aliases: ["see"], usage: "Show the workflow phase." }
            };
          }
        }
        message In  {}
        message Out {}
        message StatusOutput { string phase = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate)
        .expect("(temporal.v1.query).cli must parse cleanly");
    let q = services[0]
        .queries
        .iter()
        .find(|q| q.rpc_method == "Status")
        .expect("Status query must be in the model");
    assert_eq!(q.cli_name.as_deref(), Some("show"));
    assert_eq!(q.cli_aliases, vec!["see"]);
    assert_eq!(q.cli_usage.as_deref(), Some("Show the workflow phase."));

    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            "#[command(name = \"query-show\", alias = [\"query-see\"], about = \"Show the workflow phase.\")]"
        ),
        "(temporal.v1.query).cli overrides must surface on the QueryStatus variant: {source}"
    );
}

#[test]
fn update_options_cli_acts_as_fallback_default_for_subcommand() {
    // R6 — method-level `(temporal.v1.update).cli` overrides act as
    // the fallback default for the `Update<Name>` clap subcommand
    // when no `WorkflowOptions.update[N].cli` workflow ref carries
    // overrides. Per-ref overrides win when both are present —
    // mirrors the signal precedence policy.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package upd_default.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Touch" }]
            };
          }
          rpc Touch(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.update) = {
              cli: { name: "bump", aliases: ["nudge"], usage: "Bump the run." }
            };
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate)
        .expect("(temporal.v1.update).cli must parse cleanly");
    let u = services[0]
        .updates
        .iter()
        .find(|u| u.rpc_method == "Touch")
        .expect("Touch update must be in the model");
    assert_eq!(u.cli_name.as_deref(), Some("bump"));
    assert_eq!(u.cli_aliases, vec!["nudge"]);
    assert_eq!(u.cli_usage.as_deref(), Some("Bump the run."));

    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            "#[command(name = \"update-bump\", alias = [\"update-nudge\"], about = \"Bump the run.\")]"
        ),
        "(temporal.v1.update).cli must surface on the UpdateTouch variant when no ref override exists: {source}"
    );
}

#[test]
fn signal_options_cli_acts_as_fallback_default_for_subcommand() {
    // R6 — method-level `(temporal.v1.signal).cli` overrides act as
    // the fallback default for the `Signal<Name>` clap subcommand
    // when no `WorkflowOptions.signal[N].cli` workflow ref carries
    // overrides. Per-ref overrides win when both are present.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sig_default.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Cancel" }]
            };
          }
          rpc Cancel(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {
              cli: { name: "stop", aliases: ["halt"], usage: "Stop the workflow." }
            };
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate)
        .expect("(temporal.v1.signal).cli must parse cleanly");
    let sig = services[0]
        .signals
        .iter()
        .find(|s| s.rpc_method == "Cancel")
        .expect("Cancel signal must be in the model");
    assert_eq!(sig.cli_name.as_deref(), Some("stop"));
    assert_eq!(sig.cli_aliases, vec!["halt"]);
    assert_eq!(sig.cli_usage.as_deref(), Some("Stop the workflow."));

    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            "#[command(name = \"signal-stop\", alias = [\"signal-halt\"], about = \"Stop the workflow.\")]"
        ),
        "method-level signal.cli must surface on the SignalCancel variant when no ref override exists: {source}"
    );
}

#[test]
fn signal_ref_cli_override_wins_over_method_level_default() {
    // The per-ref override wins. The method-level default is left
    // unused when a workflow ref provides its own values.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sig_prio.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Cancel" cli: { name: "abort" } }]
            };
          }
          rpc Cancel(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = { cli: { name: "stop" } };
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("must parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("name = \"signal-abort\""),
        "per-ref override must win: {source}"
    );
    assert!(
        !source.contains("name = \"signal-stop\""),
        "method-level default must not appear when ref override is present: {source}"
    );
}

#[test]
fn signal_ref_cli_override_threads_into_subcommand() {
    // R6 — when a workflow's `WorkflowOptions.signal[N].cli` declares
    // overrides for the signal ref, those override the auto-generated
    // `signal-<name>` clap subcommand's `name` / `alias` / `about`
    // attributes. The CLI emit is service-scoped, so the first
    // workflow ref carrying overrides for a given signal wins.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sig_cli.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{
                ref: "Cancel"
                cli: { name: "abort", aliases: ["halt"], usage: "Halt the run." }
              }]
            };
          }
          rpc Cancel(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services =
        parse::parse(&pool, &files_to_generate).expect("signal[].cli override must parse");
    let sref = services[0].workflows[0]
        .attached_signals
        .iter()
        .find(|s| s.rpc_method == "Cancel")
        .expect("Cancel signal ref must be in model");
    assert_eq!(sref.cli_name.as_deref(), Some("abort"));
    assert_eq!(sref.cli_aliases, vec!["halt"]);
    assert_eq!(sref.cli_usage.as_deref(), Some("Halt the run."));

    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            "#[command(name = \"signal-abort\", alias = [\"signal-halt\"], about = \"Halt the run.\")]"
        ),
        "signal-ref cli overrides must surface on the SignalCancel variant: {source}"
    );
}

#[test]
fn cli_top_level_parser_and_subcommand_derive_debug() {
    // R6 ergonomics — the top-level `Cli` parser + the `Command`
    // subcommand enum both derive `Debug` alongside the clap
    // derives. Lets `tracing::info!(?cli, "parsed")` produce
    // structured output of the matched subcommand + its parsed
    // args during dispatch logging.
    let services = parse_and_validate("cli_emit");
    let opts = load_fixture_options("cli_emit");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("#[derive(Debug, temporal_runtime::clap::Parser)]"),
        "Cli struct must derive Debug + Parser: {source}"
    );
    assert!(
        source.contains("#[derive(Debug, temporal_runtime::clap::Subcommand)]"),
        "Command enum must derive Debug + Subcommand: {source}"
    );
}

#[test]
fn cli_args_structs_derive_debug() {
    // R6 ergonomics — every generated `<Verb><Wf>Args` /
    // `Signal<Name>Args` / `Query<Name>Args` / `Update<Name>Args`
    // struct now derives `Debug` alongside `clap::Args`. Lets
    // dispatch logging spell `tracing::info!(?args, ...)` to print
    // the parsed CLI args structurally.
    let services = parse_and_validate("cli_emit");
    let opts = load_fixture_options("cli_emit");
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("#[derive(Debug, Clone, temporal_runtime::clap::Args)]"),
        "Args structs must derive Debug + Clone alongside clap::Args: {source}"
    );
    // Old single-derive form must not survive.
    assert!(
        !source.contains("#[derive(temporal_runtime::clap::Args)]"),
        "no Args struct should keep the bare clap::Args derive: {source}"
    );
}

#[test]
fn cli_emit_renders_signal_subcommands() {
    // R6 — each `(temporal.v1.signal)` rpc gains a `Signal<Name>` CLI
    // variant. Empty-input signals skip `--input-file`; non-Empty
    // signals carry the same prost-json input-file flag pattern as
    // workflow starts.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sigcli.v1;
        import "temporal/v1/temporal.proto";
        import "google/protobuf/empty.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Pause" }, { ref: "Resume" }]
            };
          }
          rpc Pause(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
          rpc Resume(ResumeInput) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
        }
        message In  {}
        message Out {}
        message ResumeInput { string mode = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("must parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("SignalPause(SignalPauseArgs),"),
        "missing SignalPause variant: {source}"
    );
    assert!(
        source.contains("SignalResume(SignalResumeArgs),"),
        "missing SignalResume variant: {source}"
    );
    assert!(
        source.contains("pub struct SignalPauseArgs {"),
        "missing SignalPauseArgs struct: {source}"
    );
    assert!(
        source.contains("pub struct SignalResumeArgs {"),
        "missing SignalResumeArgs struct: {source}"
    );
    // Empty-input signal must NOT carry an input_file flag.
    let pause_block_start = source.find("pub struct SignalPauseArgs").unwrap();
    let pause_block_end = pause_block_start + source[pause_block_start..].find('}').unwrap();
    let pause_block = &source[pause_block_start..pause_block_end];
    assert!(
        !pause_block.contains("input_file"),
        "Empty-input signal must skip input_file flag: {pause_block}"
    );
    // Non-Empty signal must carry input_file.
    let resume_block_start = source.find("pub struct SignalResumeArgs").unwrap();
    let resume_block_end = resume_block_start + source[resume_block_start..].find('}').unwrap();
    let resume_block = &source[resume_block_start..resume_block_end];
    assert!(
        resume_block.contains("pub input_file: ::std::path::PathBuf,"),
        "non-Empty signal must include input_file flag: {resume_block}"
    );
    // Dispatch must call the client method.
    assert!(
        source.contains("client.pause(args.workflow_id.clone()).await?;"),
        "Empty-input signal dispatch must call client.<snake>(workflow_id): {source}"
    );
    assert!(
        source.contains("client.resume(args.workflow_id.clone(), input).await?;"),
        "non-Empty signal dispatch must call client.<snake>(workflow_id, input): {source}"
    );
}

#[test]
fn cli_emit_renders_query_subcommands() {
    // R6 — each `(temporal.v1.query)` rpc gains a `Query<Name>` CLI
    // variant. Empty-input queries skip `--input-file`; non-Empty
    // queries carry it. Dispatch calls `client.<query>(workflow_id,
    // input?)` and debug-prints the typed output.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package qcli.v1;
        import "temporal/v1/temporal.proto";
        import "google/protobuf/empty.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              query: [{ ref: "Status" }, { ref: "Lookup" }]
            };
          }
          rpc Status(google.protobuf.Empty) returns (StatusOutput) {
            option (temporal.v1.query) = {};
          }
          rpc Lookup(LookupInput) returns (LookupOutput) {
            option (temporal.v1.query) = {};
          }
        }
        message In  {}
        message Out {}
        message StatusOutput { string phase = 1; }
        message LookupInput  { string key = 1; }
        message LookupOutput { string value = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("must parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("QueryStatus(QueryStatusArgs),"),
        "missing QueryStatus variant: {source}"
    );
    assert!(
        source.contains("QueryLookup(QueryLookupArgs),"),
        "missing QueryLookup variant: {source}"
    );
    assert!(
        source.contains("pub struct QueryStatusArgs {"),
        "missing QueryStatusArgs struct: {source}"
    );
    assert!(
        source.contains("pub struct QueryLookupArgs {"),
        "missing QueryLookupArgs struct: {source}"
    );
    let status_start = source.find("pub struct QueryStatusArgs").unwrap();
    let status_end = status_start + source[status_start..].find('}').unwrap();
    let status_block = &source[status_start..status_end];
    assert!(
        !status_block.contains("input_file"),
        "Empty-input query must skip input_file flag: {status_block}"
    );
    let lookup_start = source.find("pub struct QueryLookupArgs").unwrap();
    let lookup_end = lookup_start + source[lookup_start..].find('}').unwrap();
    let lookup_block = &source[lookup_start..lookup_end];
    assert!(
        lookup_block.contains("pub input_file: ::std::path::PathBuf,"),
        "non-Empty query must include input_file flag: {lookup_block}"
    );
    // Empty-input dispatch.
    assert!(
        source.contains("let out = client.status(args.workflow_id.clone()).await?;"),
        "Empty-input query dispatch wrong: {source}"
    );
    // Non-Empty dispatch.
    assert!(
        source.contains("let out = client.lookup(args.workflow_id.clone(), input).await?;"),
        "non-Empty query dispatch wrong: {source}"
    );
    // Output is debug-printed for both.
    assert!(
        source.contains("result={:?}"),
        "query dispatch must debug-print the output: {source}"
    );
}

#[test]
fn cli_emit_renders_update_subcommands() {
    // R6 — each `(temporal.v1.update)` rpc gains an `Update<Name>` CLI
    // variant. Empty-input updates skip `--input-file`; non-Empty
    // updates carry it. Dispatch calls `client.<update>(workflow_id,
    // input?, None)` so the proto-declared default wait policy
    // applies, and debug-prints the typed output (`()` for Empty
    // outputs, the message for typed outputs).
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package ucli.v1;
        import "temporal/v1/temporal.proto";
        import "google/protobuf/empty.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Bump" }, { ref: "Apply" }]
            };
          }
          rpc Bump(google.protobuf.Empty) returns (BumpOutput) {
            option (temporal.v1.update) = {};
          }
          rpc Apply(ApplyInput) returns (google.protobuf.Empty) {
            option (temporal.v1.update) = {};
          }
        }
        message In  {}
        message Out {}
        message BumpOutput { uint64 next = 1; }
        message ApplyInput { string payload = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("must parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("UpdateBump(UpdateBumpArgs),"),
        "missing UpdateBump variant: {source}"
    );
    assert!(
        source.contains("UpdateApply(UpdateApplyArgs),"),
        "missing UpdateApply variant: {source}"
    );
    assert!(
        source.contains("pub struct UpdateBumpArgs {"),
        "missing UpdateBumpArgs struct: {source}"
    );
    assert!(
        source.contains("pub struct UpdateApplyArgs {"),
        "missing UpdateApplyArgs struct: {source}"
    );
    let bump_start = source.find("pub struct UpdateBumpArgs").unwrap();
    let bump_end = bump_start + source[bump_start..].find('}').unwrap();
    let bump_block = &source[bump_start..bump_end];
    assert!(
        !bump_block.contains("input_file"),
        "Empty-input update must skip input_file flag: {bump_block}"
    );
    let apply_start = source.find("pub struct UpdateApplyArgs").unwrap();
    let apply_end = apply_start + source[apply_start..].find('}').unwrap();
    let apply_block = &source[apply_start..apply_end];
    assert!(
        apply_block.contains("pub input_file: ::std::path::PathBuf,"),
        "non-Empty update must include input_file flag: {apply_block}"
    );
    // Empty-input dispatch — `(workflow_id, None)` wait_policy.
    assert!(
        source.contains("let out = client.bump(args.workflow_id.clone(), None).await?;"),
        "Empty-input update dispatch wrong: {source}"
    );
    // Non-Empty dispatch — `(workflow_id, input, None)`.
    assert!(
        source.contains("let out = client.apply(args.workflow_id.clone(), input, None).await?;"),
        "non-Empty update dispatch wrong: {source}"
    );
}

#[test]
fn service_cli_options_override_top_level_command_attrs() {
    // R6 — `(temporal.v1.cli)` at the service level overrides the
    // top-level `#[command(name, about, alias)]` on the generated
    // `Cli` struct. `ignore = true` suppresses the entire CLI module.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package svccli.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          option (temporal.v1.cli) = {
            name: "tempctl"
            usage: "Drive the temporal demo."
            aliases: ["temp", "tctl-demo"]
          };
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "tq" };
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("service-level cli must parse");
    let spec = services[0]
        .cli_options
        .as_ref()
        .expect("ServiceCliSpec must populate");
    assert_eq!(spec.name.as_deref(), Some("tempctl"));
    assert_eq!(spec.usage.as_deref(), Some("Drive the temporal demo."));
    assert_eq!(spec.aliases, vec!["temp", "tctl-demo"]);
    assert!(!spec.ignore);

    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            "#[command(name = \"tempctl\", about = \"Drive the temporal demo.\", alias = [\"temp\", \"tctl-demo\"])]"
        ),
        "service-level cli overrides must surface on the Cli struct's #[command(...)]: {source}"
    );
}

#[test]
fn service_cli_ignore_suppresses_entire_cli_module() {
    // `(temporal.v1.cli).ignore = true` suppresses the entire CLI
    // module — even if `cli=true` plugin option is set and visible
    // workflows exist.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package svccli.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          option (temporal.v1.cli) = { ignore: true };
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "tq" };
          }
        }
        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("must parse");
    let opts = protoc_gen_rust_temporal::options::RenderOptions {
        cli: true,
        ..Default::default()
    };
    let source = render::render(&services[0], &opts);
    assert!(
        !source.contains("pub mod svc_cli"),
        "service-level cli.ignore must suppress the entire CLI module: {source}"
    );
}

#[test]
fn cli_emit_off_by_default() {
    let services = parse_and_validate("cli_emit");
    let source = render::render(&services[0], &Default::default());
    assert!(!source.contains("report_service_cli"));
    assert!(!source.contains("clap::Parser"));
}

#[test]
fn workflows_emit_off_by_default() {
    let services = parse_and_validate("workflows_emit");
    let source = render::render(&services[0], &Default::default());
    // Workflow-level consts always emit (existing behavior).
    assert!(source.contains("pub const RUN_WORKFLOW_NAME"));
    // The new per-rpc handler-name consts only emit when workflows=true.
    assert!(!source.contains("CANCEL_SIGNAL_NAME"));
    assert!(!source.contains("STATUS_QUERY_NAME"));
    assert!(!source.contains("CONFIRM_UPDATE_NAME"));
}

#[test]
fn activity_only_emits_no_workflow_surface() {
    let services = parse_and_validate("activity_only");
    let svc = &services[0];
    assert!(svc.workflows.is_empty());
    assert!(svc.signals.is_empty());
    assert!(svc.queries.is_empty());
    assert!(svc.updates.is_empty());
    assert_eq!(svc.activities.len(), 2);

    let source = render::render(svc, &Default::default());
    // No workflow constants, no handle struct, no _with_start free function.
    assert!(!source.contains("_WORKFLOW_NAME"));
    assert!(!source.contains("Handle {"));
    assert!(!source.contains("_with_start("));
    // The client struct still emits — keeps the import surface consistent
    // with services that have a mix of activities and workflows.
    assert!(source.contains("pub struct WorkerOnlyServiceClient"));
}

#[test]
fn multiple_workflows_parses_correctly() {
    let services = parse_and_validate("multiple_workflows");
    let svc = &services[0];
    assert_eq!(svc.workflows.len(), 2);
    assert_eq!(svc.workflows[0].rpc_method, "Alpha");
    assert_eq!(svc.workflows[1].rpc_method, "Beta");
    // Alpha falls back to service-default task_queue, Beta overrides.
    assert_eq!(svc.workflows[0].task_queue, None);
    assert_eq!(svc.workflows[1].task_queue.as_deref(), Some("multi-beta"));
    assert_eq!(svc.default_task_queue.as_deref(), Some("multi"));
}

#[test]
fn full_workflow_emits_both_with_start_paths() {
    let services = parse_and_validate("full_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub async fn bootstrap_with_start("),
        "missing signal-with-start emission"
    );
    assert!(
        source.contains("pub async fn reconfigure_with_start("),
        "missing update-with-start emission"
    );
    // Regular signal (without start: true) must still emit the handle method
    // but NOT a free function.
    assert!(source.contains("pub async fn cancel(&self,"));
    assert!(!source.contains("pub async fn cancel_with_start("));
}

#[test]
fn workflow_only_parses_and_validates() {
    let services = parse_and_validate("workflow_only");
    assert_eq!(services.len(), 1);
    let svc = &services[0];
    assert_eq!(svc.package, "solo.v1");
    assert_eq!(svc.service, "SoloService");
    // No service-level default — falls back to the workflow's own task_queue.
    assert!(svc.default_task_queue.is_none());
    assert_eq!(svc.workflows.len(), 1);
    let wf = &svc.workflows[0];
    assert_eq!(wf.task_queue.as_deref(), Some("solo-tq"));
    assert!(wf.attached_signals.is_empty());
    assert!(wf.attached_queries.is_empty());
    assert!(wf.attached_updates.is_empty());
    assert!(svc.signals.is_empty());
    assert!(svc.queries.is_empty());
    assert!(svc.updates.is_empty());
    assert!(svc.activities.is_empty());
    assert_eq!(
        wf.execution_timeout,
        Some(std::time::Duration::from_secs(3600))
    );
}

fn assert_golden(name: &str) {
    let services = parse_and_validate(name);
    let opts = load_fixture_options(name);
    let actual = render::render(&services[0], &opts);
    let golden_path = fixture_path(name).join("expected.rs");

    if std::env::var_os("BLESS").is_some() {
        std::fs::write(&golden_path, &actual).expect("write golden");
        return;
    }

    let expected = std::fs::read_to_string(&golden_path).unwrap_or_else(|_| {
        panic!(
            "missing golden file at {}. Run `BLESS=1 cargo test ... {name}` to create it.",
            golden_path.display()
        )
    });
    if actual != expected {
        panic!(
            "rendered output diverges from golden at {}. \
             Rebless with `BLESS=1 cargo test ... {name}`.\n\n--- expected ---\n{expected}\n--- actual ---\n{actual}",
            golden_path.display()
        );
    }
}

/// Smoke check on top of the golden — kept because it pinpoints which
/// fragment changed when the golden diffs.
#[test]
fn minimal_workflow_render_smoke() {
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());

    let must_contain = [
        "// Code generated by protoc-gen-rust-temporal. DO NOT EDIT.",
        "pub mod jobs_v1_job_service_temporal {",
        "use crate::jobs::v1::*;",
        "pub const RUN_JOB_WORKFLOW_NAME: &str = \"jobs.v1.JobService.RunJob\";",
        "pub const RUN_JOB_TASK_QUEUE: &str = \"jobs\";",
        "pub struct JobServiceClient {",
        "pub async fn run_job(",
        "pub fn run_job_handle(&self, workflow_id: impl Into<String>) -> RunJobHandle",
        "pub struct RunJobHandle {",
        "pub async fn result(&self) -> Result<JobOutput>",
        "pub async fn cancel_job(&self, input: CancelJobInput) -> Result<()>",
        "pub async fn get_status(&self) -> Result<JobStatusOutput>",
        "pub async fn reconfigure(&self, input: ReconfigureInput, wait_policy: Option<temporal_runtime::WaitPolicy>)",
        "pub async fn cancel_job_with_start(",
        "fn run_job_id(input: &JobInput) -> String",
        "run_job_id(&input)",
    ];
    for needle in must_contain {
        assert!(
            source.contains(needle),
            "rendered output is missing expected fragment: {needle:?}\n\n--- full output ---\n{source}"
        );
    }

    // Activity rpcs are validate-only — they must not produce client methods.
    assert!(
        !source.contains("process_chunk"),
        "activity-only method leaked into rendered client surface:\n{source}"
    );
}

/// Regression test for issue #1 (lazy ExtensionSet). buf v2 invokes the
/// plugin once per target proto in a module. When the target is the
/// vendored annotation schema itself — `temporal/v1/temporal.proto` —
/// the plugin used to die during `ExtensionSet::load()` because the
/// schema file declares the extensions but doesn't use them, and the
/// CodeGeneratorRequest for that single-target invocation may not be
/// shaped the way the extension lookup expects. Lazy-loading turns that
/// scenario into a no-op (empty output, no error).
#[test]
fn annotation_schema_as_target_is_a_noop() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let annotations = crate_root.join(ANNOTATIONS_DIR);
    let tmp = tempfile::tempdir().expect("tempdir");
    let fds_path = tmp.path().join("out.fds");
    let status = Command::new(protoc_binary())
        .arg(format!("-I{}", annotations.display()))
        .arg(format!("--descriptor_set_out={}", fds_path.display()))
        .arg("--include_imports")
        .arg("temporal/v1/temporal.proto")
        .status()
        .expect("invoke protoc");
    assert!(status.success(), "protoc dump failed: {status}");

    let bytes = std::fs::read(&fds_path).expect("read fds");
    let mut pool = DescriptorPool::new();
    pool.decode_file_descriptor_set(bytes.as_slice())
        .expect("decode fds");

    let files_to_generate: HashSet<String> =
        std::iter::once("temporal/v1/temporal.proto".to_string()).collect();
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    assert!(
        services.is_empty(),
        "annotation schema target should produce no services"
    );
}

#[test]
fn signal_returning_non_empty_fails_validation() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Cancel(In) returns (Out) {
            option (temporal.v1.signal) = {};
          }
        }
        message In {}
        message Out {}
        "#,
    );

    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let err = validate::validate(&services[0], &Default::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("google.protobuf.Empty"),
        "validation error should mention Empty constraint, got: {err}"
    );
}

#[test]
fn client_exposes_update_by_id_methods() {
    // R4 — client-level update-by-id. Mirrors the Empty matrix on the
    // Handle plus a `wait_policy` arg. full_workflow's Reconfigure
    // update is non-Empty in, non-Empty out, so we should see attach_handle
    // + update_proto with wait_policy.
    let services = parse_and_validate("full_workflow");
    let svc = &services[0];
    let source = render::render(svc, &Default::default());
    assert!(
        source.contains("pub async fn reconfigure(&self, workflow_id: impl Into<String>, input: ReconfigureInput, wait_policy: Option<temporal_runtime::WaitPolicy>) -> Result<ReconfigureOutput>"),
        "client must expose Reconfigure update-by-id with wait_policy: {source}"
    );
    assert!(
        source.contains("temporal_runtime::update_proto::<ReconfigureInput, ReconfigureOutput>(&inner, \"full.v1.FullService.Reconfigure\", &input, wait_policy).await"),
        "non-Empty/non-Empty update must route to update_proto"
    );
}

#[test]
fn client_update_by_id_covers_empty_variants() {
    // empty_output_query_update exercises (Empty-in/Empty-out) and
    // (non-Empty-in/Empty-out) update branches. Both must compile to the
    // correct bridge fn at the client level.
    let services = parse_and_validate("empty_output_query_update");
    let svc = &services[0];
    let source = render::render(svc, &Default::default());
    assert!(
        source.contains("temporal_runtime::update_proto_empty_unit(&inner,"),
        "Empty-in/Empty-out update must route to update_proto_empty_unit at client level: {source}"
    );
    assert!(
        source.contains("temporal_runtime::update_unit::<"),
        "non-Empty-in/Empty-out update must route to update_unit at client level: {source}"
    );
}

#[test]
fn client_exposes_query_by_id_methods() {
    // R4 — client-level query-by-id. Mirrors the Empty-variant matrix on
    // the Handle:
    //   (Empty in, non-Empty out)       → query_proto_empty
    //   (Empty in, Empty out)           → query_proto_empty_unit
    //   (non-Empty in, non-Empty out)   → query_proto
    //   (non-Empty in, Empty out)       → query_unit
    //
    // full_workflow's Status query is Empty-in, non-Empty-out
    // (StatusOutput), so we should see attach_handle + query_proto_empty.
    let services = parse_and_validate("full_workflow");
    let svc = &services[0];
    let source = render::render(svc, &Default::default());
    assert!(
        source.contains(
            "pub async fn status(&self, workflow_id: impl Into<String>) -> Result<StatusOutput>"
        ),
        "client must expose Status query-by-id (Empty-in, non-Empty-out): {source}"
    );
    assert!(
        source.contains("temporal_runtime::query_proto_empty::<StatusOutput>(&inner, \"full.v1.FullService.Status\").await"),
        "Empty-in query must route to query_proto_empty"
    );
    assert!(
        source.contains(
            "let inner = temporal_runtime::attach_handle(&self.client, workflow_id.into());"
        ),
        "client query-by-id must attach a handle before calling the bridge"
    );
}

#[test]
fn client_query_by_id_covers_empty_output_variants() {
    // empty_output_query_update covers (Empty-in, Empty-out) and
    // (non-Empty-in, Empty-out). Both must compile to the right bridge fn
    // at the client level too.
    let services = parse_and_validate("empty_output_query_update");
    let svc = &services[0];
    let source = render::render(svc, &Default::default());
    assert!(
        source.contains("temporal_runtime::query_proto_empty_unit(&inner,"),
        "Empty-in/Empty-out query must route to query_proto_empty_unit at client level: {source}"
    );
    assert!(
        source.contains("temporal_runtime::query_unit::<"),
        "non-Empty-in/Empty-out query must route to query_unit at client level: {source}"
    );
}

#[test]
fn client_exposes_signal_by_id_methods() {
    // R4: `<Service>Client::<signal>(workflow_id, input)` lets callers send
    // a signal without first calling `<rpc>_handle(id)`. Mirrors the Go
    // plugin's top-level `client.<Signal>(ctx, id, runID, input)`.
    let services = parse_and_validate("full_workflow");
    let svc = &services[0];
    let source = render::render(svc, &Default::default());

    // Signal with non-Empty input → takes the input by value.
    assert!(
        source.contains("pub async fn cancel(&self, workflow_id: impl Into<String>, input: CancelInput) -> Result<()>"),
        "client must expose Cancel signal-by-id with typed input: {source}"
    );
    assert!(
        source.contains(
            "temporal_runtime::signal_proto(&inner, \"full.v1.FullService.Cancel\", &input).await"
        ),
        "client signal-by-id must call signal_proto with the registered name"
    );
    // Sibling Bootstrap signal too.
    assert!(
        source.contains("pub async fn bootstrap(&self, workflow_id: impl Into<String>, input: BootstrapInput) -> Result<()>"),
        "client must expose Bootstrap signal-by-id too"
    );
}

#[test]
fn client_signal_by_id_handles_empty_input() {
    // Empty-input signal: the method takes only workflow_id and routes to
    // `signal_unit`, not `signal_proto`, matching the existing Handle-side
    // Empty-input emit.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package empty_sig.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Ping" }]
            };
          }
          rpc Ping(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
        }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub async fn ping(&self, workflow_id: impl Into<String>) -> Result<()>"),
        "Empty-input signal client method must not take an `input` arg: {source}"
    );
    assert!(
        source.contains("temporal_runtime::signal_unit(&inner, \"empty_sig.v1.Svc.Ping\").await"),
        "Empty-input variant must route to signal_unit"
    );
}

#[test]
fn every_workflow_handle_exposes_run_id_accessor() {
    // R4: `<Workflow>Handle::run_id(&self) -> Option<&str>` forwards to the
    // facade's `WorkflowHandle::run_id`. `None` for `attach_handle`-produced
    // handles; `Some(...)` after the start path populates it.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub fn run_id(&self) -> Option<&str>"),
        "handle must carry run_id() accessor: {source}"
    );
    assert!(
        source.contains("self.inner.run_id()"),
        "run_id() must delegate to the facade WorkflowHandle: {source}"
    );
}

#[test]
fn every_workflow_handle_exposes_cancel_and_terminate() {
    // R4: cancel & terminate are operations on the execution itself, not
    // proto-driven, so every generated `<Workflow>Handle` carries them
    // unconditionally — even workflows that declare no attached
    // signal/query/update.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("pub async fn cancel_workflow(&self, reason: &str) -> Result<()>"),
        "minimal workflow handle must carry cancel_workflow(): {source}"
    );
    assert!(
        source.contains("temporal_runtime::cancel_workflow(&self.inner, reason).await"),
        "cancel_workflow() must delegate to the runtime facade"
    );
    assert!(
        source.contains("pub async fn terminate_workflow(&self, reason: &str) -> Result<()>"),
        "minimal workflow handle must carry terminate_workflow(): {source}"
    );
    assert!(
        source.contains("temporal_runtime::terminate_workflow(&self.inner, reason).await"),
        "terminate_workflow() must delegate to the runtime facade"
    );
}

#[test]
fn cancel_and_terminate_appear_on_every_fixture_handle() {
    // Belt-and-braces — even fixtures with rich attached refs must keep
    // the cancel/terminate pair on every Handle. Walks every fixture so
    // we catch a future regression like "cancel/terminate emit was tied
    // to attached_signals being non-empty".
    let fixtures = [
        "minimal_workflow",
        "full_workflow",
        "workflow_only",
        "empty_input_workflow",
        "empty_output_query_update",
        "multiple_workflows",
        "activity_only",
        "cli_emit",
    ];
    for fixture in fixtures {
        let services = parse_and_validate(fixture);
        let opts = load_fixture_options(fixture);
        for svc in &services {
            let source = render::render(svc, &opts);
            // Every workflow contributes one Handle struct + one pair.
            let cancels = source
                .matches("pub async fn cancel_workflow(&self, reason: &str)")
                .count();
            let terminates = source
                .matches("pub async fn terminate_workflow(&self, reason: &str)")
                .count();
            assert_eq!(
                cancels,
                svc.workflows.len(),
                "{fixture}: expected one cancel_workflow() per workflow, got {cancels} for {} workflow(s)",
                svc.workflows.len()
            );
            assert_eq!(
                terminates,
                svc.workflows.len(),
                "{fixture}: expected one terminate_workflow() per workflow, got {terminates}"
            );
        }
    }
}

#[test]
fn workflow_retry_policy_flows_into_start_options() {
    // R5: `retry_policy` graduates from rejected to supported. The proto's
    // RetryPolicy lands on the model, then re-emerges as a
    // `temporal_runtime::RetryPolicy` literal that the start path folds
    // into `WorkflowStartOptions.retry_policy`.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package retry.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              retry_policy: {
                initial_interval:    { seconds: 1 }
                backoff_coefficient: 2.0
                max_interval:        { seconds: 60 }
                max_attempts:        5
                non_retryable_error_types: ["ValidationError", "PermanentFailure"]
              }
            };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let svc = &services[0];
    let spec = svc.workflows[0]
        .retry_policy
        .as_ref()
        .expect("model carries the proto-declared retry policy");
    assert_eq!(spec.max_attempts, 5);
    assert!((spec.backoff_coefficient() - 2.0).abs() < f64::EPSILON);
    assert_eq!(
        spec.non_retryable_error_types,
        vec![
            "ValidationError".to_string(),
            "PermanentFailure".to_string(),
        ],
    );

    let source = render::render(svc, &Default::default());
    assert!(
        source.contains("pub retry_policy: Option<temporal_runtime::RetryPolicy>,"),
        "StartOptions must expose the retry-policy field: {source}"
    );
    assert!(
        source.contains("let retry_policy = opts.retry_policy.or_else(|| Some({"),
        "start path must fold the proto-declared default in: {source}"
    );
    assert!(
        source.contains("rp.max_attempts = 5;"),
        "literal should set the max_attempts the proto declared: {source}"
    );
    assert!(
        source.contains("rp.set_backoff_coefficient(2.0)"),
        "literal should set the backoff coefficient: {source}"
    );
    assert!(
        source.contains("\"ValidationError\".to_string(), \"PermanentFailure\".to_string()"),
        "literal should carry the non_retryable_error_types list: {source}"
    );
    assert!(
        source.contains("retry_policy,"),
        "resolved value must be forwarded to the bridge call: {source}"
    );
}

#[test]
fn workflow_without_retry_policy_resolves_to_none() {
    let services = parse_and_validate("minimal_workflow");
    assert!(services[0].workflows[0].retry_policy.is_none());
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("let retry_policy = opts.retry_policy;"),
        "start path must rebind opts directly when no default is declared: {source}"
    );
}

#[test]
fn workflow_with_multiple_unsupported_fields_lists_all() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:          "tq"
              search_attributes:   "string.foo = \"bar\""
              versioning_behavior: VERSIONING_BEHAVIOR_PINNED
              typed_search_attributes: "root = {}"
            };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    for field in [
        "search_attributes",
        "typed_search_attributes",
        "versioning_behavior",
    ] {
        assert!(
            err.contains(field),
            "diagnostic should list {field}, got: {err}"
        );
    }
}

#[test]
fn update_id_template_emits_workflow_id_derivation_and_by_template_method() {
    // R5: `UpdateOptions.id` is a workflow-id template resolved against
    // the update's input. Compile time we materialise it into a private
    // `<update>_workflow_id(input) -> String` fn (mirroring the existing
    // workflow-id derivation) plus a `<update>_by_template` client method
    // that calls the derivation and forwards to the update-by-id helper.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package upd_id.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Patch" }]
            };
          }
          rpc Patch(PatchInput) returns (PatchOutput) {
            option (temporal.v1.update) = { id: "patch-{{ .Field }}" };
          }
        }
        message In {}
        message Out {}
        message PatchInput  { string field = 1; }
        message PatchOutput { bool ok = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let svc = &services[0];
    let update_model = svc
        .updates
        .iter()
        .find(|u| u.rpc_method == "Patch")
        .expect("Patch update");
    let segments = update_model
        .id_expression
        .as_ref()
        .expect("model carries the update-id template");
    use protoc_gen_rust_temporal::model::IdTemplateSegment;
    assert_eq!(
        segments,
        &[
            IdTemplateSegment::Literal("patch-".to_string()),
            IdTemplateSegment::Field("field".to_string()),
        ],
        "template must compile to a literal segment then a field reference"
    );

    let source = render::render(svc, &Default::default());
    assert!(
        source.contains("fn patch_workflow_id(input: &PatchInput) -> String"),
        "derivation fn must take the update input by ref: {source}"
    );
    assert!(
        source.contains("format!(\"patch-{}\", input.field)"),
        "derivation fn must format the template against the input field: {source}"
    );
    assert!(
        source.contains(
            "pub async fn patch_by_template(&self, input: PatchInput, wait_policy: Option<temporal_runtime::WaitPolicy>) -> Result<PatchOutput>"
        ),
        "client must expose `<update>_by_template` convenience: {source}"
    );
    assert!(
        source.contains("let workflow_id = patch_workflow_id(&input);"),
        "by_template method must derive the id via the codegen helper: {source}"
    );
    assert!(
        source.contains("self.patch(workflow_id, input, wait_policy).await"),
        "by_template must forward to the by-id update method: {source}"
    );
}

#[test]
fn update_without_id_template_omits_by_template_method() {
    // The `<update>_by_template` convenience only appears when the proto
    // declares the template — keeps the client surface honest.
    let services = parse_and_validate("full_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        !source.contains("reconfigure_by_template"),
        "full_workflow's Reconfigure update doesn't declare a template, so by_template must not be emitted: {source}"
    );
    assert!(
        !source.contains("fn reconfigure_workflow_id"),
        "the derivation fn should also be absent without a template: {source}"
    );
}

#[test]
fn update_wait_for_stage_folds_into_default() {
    // R5: `UpdateOptions.wait_for_stage` is now honoured. The update method's
    // `wait_policy` arg is `Option<WaitPolicy>` and the proto default folds
    // in at the call site when the caller passes `None`.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package wfs.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Patch" }]
            };
          }
          rpc Patch(In) returns (Out) {
            option (temporal.v1.update) = { wait_for_stage: WAIT_POLICY_ACCEPTED };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let svc = &services[0];
    let patch = svc
        .updates
        .iter()
        .find(|u| u.rpc_method == "Patch")
        .unwrap();
    use protoc_gen_rust_temporal::model::WaitPolicyKind;
    assert_eq!(patch.default_wait_policy, Some(WaitPolicyKind::Accepted));

    let source = render::render(svc, &Default::default());
    assert!(
        source.contains("wait_policy: Option<temporal_runtime::WaitPolicy>"),
        "wait_policy arg must now be Option<WaitPolicy>: {source}"
    );
    assert!(
        source.contains(
            "let wait_policy = wait_policy.unwrap_or(temporal_runtime::WaitPolicy::Accepted);"
        ),
        "proto default must fold in at the call site: {source}"
    );
}

#[test]
fn update_deprecated_wait_policy_folds_into_default() {
    // The deprecated `wait_policy` field is still honoured for legacy
    // Go-ported protos. When `wait_for_stage` is unset and `wait_policy`
    // is set, we use `wait_policy`.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package wp.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Patch" }]
            };
          }
          rpc Patch(In) returns (Out) {
            option (temporal.v1.update) = { wait_policy: WAIT_POLICY_ADMITTED };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let svc = &services[0];
    let patch = svc
        .updates
        .iter()
        .find(|u| u.rpc_method == "Patch")
        .unwrap();
    use protoc_gen_rust_temporal::model::WaitPolicyKind;
    assert_eq!(patch.default_wait_policy, Some(WaitPolicyKind::Admitted));
    let source = render::render(svc, &Default::default());
    assert!(
        source.contains(
            "let wait_policy = wait_policy.unwrap_or(temporal_runtime::WaitPolicy::Admitted);"
        ),
        "deprecated wait_policy must fold in identically to wait_for_stage: {source}"
    );
}

#[test]
fn update_without_wait_policy_default_falls_back_to_completed() {
    // When the proto declares no default, callers can still pass None and
    // the codegen falls back to `Completed` — matching the SDK's prior
    // mandatory-arg behaviour.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains(
            "let wait_policy = wait_policy.unwrap_or(temporal_runtime::WaitPolicy::Completed);"
        ),
        "no proto default → fallback to Completed: {source}"
    );
}

#[test]
fn workflow_update_ref_with_conflict_policy_threads_through() {
    // R5 — per-update `workflow_id_conflict_policy` on
    // `WorkflowOptions.update[]` now flows through the
    // bridge's update-with-start path instead of being refused.
    // The render emits `Some(WorkflowIdConflictPolicy::<Variant>)` as
    // the trailing arg; `None` (proto unset) keeps the bridge's
    // historical `UseExisting` default in place.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package upd_conflict.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Patch" start: true workflow_id_conflict_policy: WORKFLOW_ID_CONFLICT_POLICY_FAIL }]
            };
          }
          rpc Patch(In) returns (Out) {
            option (temporal.v1.update) = {};
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let services =
        parse::parse(&pool, &files_to_generate).expect("conflict_policy on update ref must parse");
    use protoc_gen_rust_temporal::model::IdConflictPolicy;
    let uref = services[0].workflows[0]
        .attached_updates
        .iter()
        .find(|u| u.rpc_method == "Patch")
        .expect("Patch update ref must be in model");
    assert_eq!(uref.id_conflict_policy, Some(IdConflictPolicy::Fail));

    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("Some(temporal_runtime::WorkflowIdConflictPolicy::Fail)"),
        "render must thread the override into update_with_start: {source}"
    );
}

#[test]
fn workflow_id_with_bloblang_expression_is_rejected() {
    // Bloblang `${! ... }` is cludden's search-attribute templating dialect
    // and looks like literal text to the `{{...}}` scanner. Without an
    // explicit reject, every workflow under such an annotation would ship
    // with the same literal ID and collide on every execution.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              id: "user-${! name.or(\"anon\") }"
            };
          }
        }
        message In { string name = 1; }
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate).unwrap_err();
    let full = format!("{err:#}");
    assert!(
        full.contains("Bloblang"),
        "expected Bloblang-rejection diagnostic, got: {full}"
    );
}

// Old `activity_with_timeouts_is_rejected` replaced by the positive
// `activity_default_options_*` tests above — those fields now flow into
// the per-activity factory instead of failing parse.

/// Table-driven coverage of every `reject_unsupported_*` branch in
/// `parse.rs`. When you add a new rejection rule, drop a row here naming
/// the field and an isolating proto snippet. The roadmap (R1) requires that
/// every unsupported-field diagnostic fire under test so silent drops can
#[test]
fn search_attributes_empty_map_bloblang_is_accepted() {
    // R7 slice 1 — `(temporal.v1.workflow).search_attributes = "root = {}"`
    // is the canonical "no search attrs" Bloblang expression. Parse
    // accepts it (no longer rejected) and stores `Some(Empty)` on the
    // model. Runtime emit treats Empty as a no-op — semantically
    // identical to leaving the field unset, which faithfully implements
    // "this workflow declares zero search attributes".
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_empty.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = {}"
            };
          }
        }
        message In {}  message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must accept root = {}");
    use protoc_gen_rust_temporal::model::SearchAttributesSpec;
    assert_eq!(
        services[0].workflows[0].search_attributes,
        Some(SearchAttributesSpec::Empty),
        "model must record the empty-map spec so slice 2 has a foundation"
    );
}

#[test]
fn search_attributes_whitespace_variations_accepted() {
    use protoc_gen_rust_temporal::model::SearchAttributesSpec;
    let cases = ["root = {}", "root={}", "  root  =  {  }  "];
    for raw in cases {
        let proto = format!(
            r#"
            syntax = "proto3";
            package sa_ws.v1;
            import "temporal/v1/temporal.proto";

            service Svc {{
              rpc Run(In) returns (Out) {{
                option (temporal.v1.workflow) = {{
                  task_queue:        "tq"
                  search_attributes: "{}"
                }};
              }}
            }}
            message In {{}} message Out {{}}
            "#,
            raw.escape_default()
        );
        let (pool, files, _tmp) = compile_fixture_inline(&proto);
        let services =
            parse::parse(&pool, &files).unwrap_or_else(|e| panic!("parse failed for {raw:?}: {e}"));
        assert_eq!(
            services[0].workflows[0].search_attributes,
            Some(SearchAttributesSpec::Empty),
            "whitespace variant {raw:?} must parse to Empty"
        );
    }
}

#[test]
fn search_attributes_static_literal_map_compiles_to_hashmap() {
    // R7 slice 2 — `root = { "Key1": "value", "Key2": 42, "Key3": true }`
    // parses to `SearchAttributesSpec::Static(..)` and emits a
    // `HashMap<String, Payload>` construction at the start path that
    // calls the bridge's per-type encoders.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_static.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = { \"Environment\": \"production\", \"Priority\": 5, \"Critical\": true }"
            };
          }
        }
        message In {}  message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files).expect("parse must accept literal-map form");
    use protoc_gen_rust_temporal::model::{SearchAttributeLiteral, SearchAttributesSpec};
    let spec = services[0].workflows[0]
        .search_attributes
        .as_ref()
        .expect("model carries the slice-2 spec");
    let SearchAttributesSpec::Static(entries) = spec else {
        panic!("expected Static spec, got {spec:?}");
    };
    assert_eq!(entries.len(), 3, "all three entries must land: {entries:?}");
    assert!(
        entries.contains(&(
            "Environment".to_string(),
            SearchAttributeLiteral::String("production".to_string())
        )),
        "string entry must parse: {entries:?}"
    );
    assert!(
        entries.contains(&("Priority".to_string(), SearchAttributeLiteral::Int(5))),
        "int entry must parse: {entries:?}"
    );
    assert!(
        entries.contains(&("Critical".to_string(), SearchAttributeLiteral::Bool(true))),
        "bool entry must parse: {entries:?}"
    );

    let opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("temporal_runtime::encode_search_attribute_string(\"production\")"),
        "string encoder must be invoked: {source}"
    );
    assert!(
        source.contains("temporal_runtime::encode_search_attribute_int(5i64)"),
        "int encoder must be invoked: {source}"
    );
    assert!(
        source.contains("temporal_runtime::encode_search_attribute_bool(true)"),
        "bool encoder must be invoked: {source}"
    );
    assert!(
        source.contains("let search_attributes = Some({"),
        "Static spec must produce `Some(HashMap)` rather than `None`: {source}"
    );
    assert!(
        source.contains("    search_attributes,\n"),
        "resolved value must forward to the bridge call: {source}"
    );
}

#[test]
fn search_attributes_double_literal_compiles_to_encoder_call() {
    // R7 slice 2 + bridge double primitive — Bloblang `<key>: 1.5`
    // entries parse to `SearchAttributeLiteral::Double(f64)` and emit
    // `temporal_runtime::encode_search_attribute_double(N).expect(...)`
    // at the start path. Whole-number doubles preserve the decimal in
    // the emitted literal so the wire shape stays an unambiguous JSON
    // number.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_double.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = { \"Score\": 1.5, \"Whole\": 2.0, \"Sci\": 1e6 }"
            };
          }
        }
        message In {}  message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files).expect("parse must accept double literals");
    use protoc_gen_rust_temporal::model::{SearchAttributeLiteral, SearchAttributesSpec};
    let SearchAttributesSpec::Static(entries) = services[0].workflows[0]
        .search_attributes
        .as_ref()
        .expect("model carries the slice-2 spec")
    else {
        panic!("expected Static spec");
    };
    assert!(
        entries.iter().any(|(k, v)| k == "Score"
            && matches!(v, SearchAttributeLiteral::Double(d) if (*d - 1.5).abs() < 1e-12)),
        "Score must parse to Double(1.5): {entries:?}"
    );
    assert!(
        entries.iter().any(|(k, v)| k == "Whole"
            && matches!(v, SearchAttributeLiteral::Double(d) if (*d - 2.0).abs() < 1e-12)),
        "Whole must parse to Double(2.0): {entries:?}"
    );
    assert!(
        entries.iter().any(|(k, v)| k == "Sci"
            && matches!(v, SearchAttributeLiteral::Double(d) if (*d - 1e6).abs() < 1e-6)),
        "Sci must parse to Double(1e6): {entries:?}"
    );

    let opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            "temporal_runtime::encode_search_attribute_double(1.5f64).expect(\"compile-time-finite f64 literal\")"
        ),
        "1.5 literal must emit the bridge encoder call: {source}"
    );
    assert!(
        source.contains("encode_search_attribute_double(2.0f64)"),
        "whole-number f64 must preserve the decimal in the emitted literal: {source}"
    );
}

#[test]
fn workflow_id_template_rejects_repeated_field() {
    // Catch a real footgun: a workflow id template referencing a
    // repeated / map field would emit `format!("{}", input.<field>)`
    // and fail to compile with a generic Display error. Reject at
    // parse with a clear message instead.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package id_repeated.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              id:         "job-{{ .Tags }}"
            };
          }
        }
        message In  { repeated string tags = 1; }
        message Out {}
        "#,
    );
    let err = format!("{:#}", parse::parse(&pool, &files).unwrap_err());
    assert!(
        err.contains("repeated") && err.contains("Tags"),
        "diagnostic must name the repeated kind + field, got: {err}"
    );
}

#[test]
fn workflow_id_template_rejects_message_field() {
    // Nested-message field refs in the id template don't have a
    // stable string form — reject with a clear message.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package id_msg.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              id:         "job-{{ .Meta }}"
            };
          }
        }
        message Inner {}
        message In  { Inner meta = 1; }
        message Out {}
        "#,
    );
    let err = format!("{:#}", parse::parse(&pool, &files).unwrap_err());
    assert!(
        err.contains("nested message") && err.contains("Meta"),
        "diagnostic must name the nested-message kind + field, got: {err}"
    );
}

#[test]
fn workflow_id_template_rejects_enum_field() {
    // prost emits enum fields as bare `i32`, so substituting them
    // via `format!("{}", ...)` would print the numeric tag — almost
    // never what the proto author intends. Reject so the surprise
    // surfaces at codegen.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package id_enum.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              id:         "job-{{ .Status }}"
            };
          }
        }
        enum Status { STATUS_UNSPECIFIED = 0; STATUS_ACTIVE = 1; }
        message In  { Status status = 1; }
        message Out {}
        "#,
    );
    let err = format!("{:#}", parse::parse(&pool, &files).unwrap_err());
    assert!(
        err.contains("enum") && err.contains("numeric tag") && err.contains("Status"),
        "diagnostic must explain enum-as-numeric-tag for `Status`, got: {err}"
    );
}

#[test]
fn workflow_id_template_rejects_bytes_field() {
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package id_bytes.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              id:         "job-{{ .Blob }}"
            };
          }
        }
        message In  { bytes blob = 1; }
        message Out {}
        "#,
    );
    let err = format!("{:#}", parse::parse(&pool, &files).unwrap_err());
    assert!(
        err.contains("bytes") && err.contains("Blob"),
        "diagnostic must name the bytes kind + field, got: {err}"
    );
}

#[test]
fn search_attributes_duplicate_key_is_rejected() {
    // The slice-2 literal map must not declare the same key twice —
    // render would emit `sa.insert(K, V1); sa.insert(K, V2);` and the
    // second silently wins. Fall through to the standard
    // unsupported-`search_attributes` diagnostic instead.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_dup_key.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = { \"Env\": \"prod\", \"Env\": \"staging\" }"
            };
          }
        }
        message In {}  message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files).unwrap_err().to_string();
    assert!(
        err.contains("search_attributes") && err.contains("does not yet honour"),
        "duplicate-key literal map must surface the unsupported diagnostic: {err}"
    );
}

#[test]
fn search_attributes_string_literal_accepts_minimal_json_escapes() {
    // R7 slice 2 — the string lexer accepts the same minimal escape
    // set the encoder emits: `\\` (backslash) and `\"` (double quote).
    // Other escape sequences still fall through to the standard
    // unsupported diagnostic.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_esc.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = { \"Quoted\": \"with\\\"quote\", \"Slashed\": \"path\\\\to\\\\thing\" }"
            };
          }
        }
        message In {}  message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files).expect("parse must accept minimal escapes");
    use protoc_gen_rust_temporal::model::{SearchAttributeLiteral, SearchAttributesSpec};
    let SearchAttributesSpec::Static(entries) = services[0].workflows[0]
        .search_attributes
        .as_ref()
        .expect("model carries the spec")
    else {
        panic!("expected Static spec");
    };
    // Model carries the *unescaped* string.
    assert!(
        entries.contains(&(
            "Quoted".to_string(),
            SearchAttributeLiteral::String("with\"quote".to_string())
        )),
        "escaped quote must unescape in the model: {entries:?}"
    );
    assert!(
        entries.contains(&(
            "Slashed".to_string(),
            SearchAttributeLiteral::String("path\\to\\thing".to_string())
        )),
        "escaped backslashes must unescape in the model: {entries:?}"
    );
}

#[test]
fn search_attributes_string_literal_rejects_unknown_escape() {
    // `\n`, `\t`, etc. are not in the minimal slice-2 escape set;
    // fall through to the standard unsupported diagnostic.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_esc_bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = { \"K\": \"line\\nbreak\" }"
            };
          }
        }
        message In {}  message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files).unwrap_err().to_string();
    assert!(
        err.contains("search_attributes") && err.contains("does not yet honour"),
        "non-minimal escape must surface the unsupported diagnostic: {err}"
    );
}

#[test]
fn search_attributes_string_field_ref_resolves_against_input() {
    // R7 slice 3a — `this.<field>` references against `string`-typed
    // singular input fields land as `SearchAttributeLiteral::StringField`
    // and emit the per-call encoder reading from the start path's
    // `input` binding.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_field.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = { \"CustomerId\": this.customer_id, \"Env\": \"prod\" }"
            };
          }
        }
        message In  { string customer_id = 1; }
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files).expect("parse must accept this.<field> for strings");
    use protoc_gen_rust_temporal::model::{SearchAttributeLiteral, SearchAttributesSpec};
    let SearchAttributesSpec::Static(entries) = services[0].workflows[0]
        .search_attributes
        .as_ref()
        .expect("model carries the slice-3a spec")
    else {
        panic!("expected Static spec");
    };
    assert!(
        entries.contains(&(
            "CustomerId".to_string(),
            SearchAttributeLiteral::StringField("customer_id".to_string())
        )),
        "field-ref entry must parse to StringField with snake_case name: {entries:?}"
    );

    let opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            "temporal_runtime::encode_search_attribute_string(input.customer_id.as_str())"
        ),
        "field-ref encoder must read from the start path's `input` binding: {source}"
    );
}

#[test]
fn search_attributes_field_ref_to_unknown_field_is_rejected() {
    // Field-refs against a non-existent input field fall through to the
    // standard "does not yet honour search_attributes" diagnostic so the
    // user sees the limitation at codegen time.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_field_bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = { \"K\": this.does_not_exist }"
            };
          }
        }
        message In  { string customer_id = 1; }
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files).unwrap_err().to_string();
    assert!(
        err.contains("search_attributes") && err.contains("does not yet honour"),
        "missing-field ref must surface the unsupported diagnostic: {err}"
    );
}

#[test]
fn search_attributes_int_and_bool_field_refs_resolve_against_input() {
    // R7 slice 3b — `this.<field>` resolves against `int64` and `bool`
    // input fields too, emitting the per-type bridge encoder.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_field_int_bool.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = { \"Priority\": this.priority, \"Critical\": this.is_critical }"
            };
          }
        }
        message In  { int64 priority = 1; bool is_critical = 2; }
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files).expect("parse must accept int + bool field refs");
    use protoc_gen_rust_temporal::model::{SearchAttributeLiteral, SearchAttributesSpec};
    let SearchAttributesSpec::Static(entries) = services[0].workflows[0]
        .search_attributes
        .as_ref()
        .expect("model carries the slice-3b spec")
    else {
        panic!("expected Static spec");
    };
    assert!(
        entries.contains(&(
            "Priority".to_string(),
            SearchAttributeLiteral::IntField {
                rust_field: "priority".to_string(),
                widen: false,
            }
        )),
        "int field ref must land as IntField: {entries:?}"
    );
    assert!(
        entries.contains(&(
            "Critical".to_string(),
            SearchAttributeLiteral::BoolField("is_critical".to_string())
        )),
        "bool field ref must land as BoolField: {entries:?}"
    );

    let opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains("temporal_runtime::encode_search_attribute_int(input.priority)"),
        "int field-ref encoder must read from input: {source}"
    );
    assert!(
        source.contains("temporal_runtime::encode_search_attribute_bool(input.is_critical)"),
        "bool field-ref encoder must read from input: {source}"
    );
}

#[test]
fn search_attributes_narrow_int_field_refs_widen_to_i64() {
    // R7 slice 3 — `int32` / `uint32` / `sint32` / `fixed32` /
    // `sfixed32` input fields produce IntField with `widen = true`,
    // emitting `input.<field> as i64` so the bridge encoder's i64
    // signature works uniformly. `int64` / `sint64` / `sfixed64` use
    // the value directly. `uint64` / `fixed64` cannot widen safely
    // and remain rejected.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_int_narrow.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = { \"P32\": this.priority32, \"U32\": this.priority_u32, \"P64\": this.priority64 }"
            };
          }
        }
        message In  {
          int32  priority32     = 1;
          uint32 priority_u32   = 2;
          int64  priority64     = 3;
        }
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files).expect("parse must accept narrow int field refs");
    use protoc_gen_rust_temporal::model::{SearchAttributeLiteral, SearchAttributesSpec};
    let SearchAttributesSpec::Static(entries) = services[0].workflows[0]
        .search_attributes
        .as_ref()
        .expect("model carries the spec")
    else {
        panic!("expected Static spec");
    };
    // 32-bit signed must widen.
    assert!(
        entries
            .iter()
            .any(|(k, v)| k == "P32" && matches!(v, SearchAttributeLiteral::IntField { rust_field, widen } if rust_field == "priority32" && *widen)),
        "int32 ref must land as IntField widen=true: {entries:?}"
    );
    // 32-bit unsigned must widen.
    assert!(
        entries
            .iter()
            .any(|(k, v)| k == "U32" && matches!(v, SearchAttributeLiteral::IntField { rust_field, widen } if rust_field == "priority_u32" && *widen)),
        "uint32 ref must land as IntField widen=true: {entries:?}"
    );
    // i64 doesn't widen.
    assert!(
        entries
            .iter()
            .any(|(k, v)| k == "P64" && matches!(v, SearchAttributeLiteral::IntField { rust_field, widen } if rust_field == "priority64" && !*widen)),
        "int64 ref must land as IntField widen=false: {entries:?}"
    );

    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("encode_search_attribute_int(input.priority32 as i64)"),
        "int32 ref must widen via `as i64`: {source}"
    );
    assert!(
        source.contains("encode_search_attribute_int(input.priority_u32 as i64)"),
        "uint32 ref must widen via `as i64`: {source}"
    );
    assert!(
        source.contains("encode_search_attribute_int(input.priority64)"),
        "int64 ref must NOT widen: {source}"
    );
}

#[test]
fn search_attributes_uint64_field_ref_is_rejected() {
    // `uint64` / `fixed64` exceed i64::MAX and cannot widen safely.
    // Fall through to the standard unsupported-`search_attributes`
    // diagnostic so callers see the limitation at codegen.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_u64.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = { \"K\": this.counter }"
            };
          }
        }
        message In  { uint64 counter = 1; }
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files).unwrap_err().to_string();
    assert!(
        err.contains("search_attributes") && err.contains("does not yet honour"),
        "uint64 field ref must surface the unsupported diagnostic: {err}"
    );
}

#[test]
fn search_attributes_double_and_float_field_refs_resolve_against_input() {
    // R7 slice 3 + bridge double primitive — `this.<field>` resolves
    // against `double` and `float` singular input fields too. `double`
    // fields use the input value directly; `float` widens via
    // `as f64` so the bridge encoder's f64 signature works uniformly.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_field_double.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = { \"Score\": this.score, \"Ratio\": this.ratio }"
            };
          }
        }
        message In  { double score = 1; float ratio = 2; }
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files).expect("parse must accept double/float field refs");
    use protoc_gen_rust_temporal::model::{SearchAttributeLiteral, SearchAttributesSpec};
    let SearchAttributesSpec::Static(entries) = services[0].workflows[0]
        .search_attributes
        .as_ref()
        .expect("model carries the slice-3 spec")
    else {
        panic!("expected Static spec");
    };
    assert!(
        entries
            .iter()
            .any(|(k, v)| k == "Score" && matches!(v, SearchAttributeLiteral::DoubleField { rust_field, is_f32 } if rust_field == "score" && !*is_f32)),
        "Score must parse to DoubleField(score, is_f32=false): {entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|(k, v)| k == "Ratio" && matches!(v, SearchAttributeLiteral::DoubleField { rust_field, is_f32 } if rust_field == "ratio" && *is_f32)),
        "Ratio must parse to DoubleField(ratio, is_f32=true): {entries:?}"
    );

    let opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let source = render::render(&services[0], &opts);
    assert!(
        source.contains(
            "temporal_runtime::encode_search_attribute_double(input.score).expect(\"search_attribute double value must be finite at runtime\")"
        ),
        "double field ref must read input.score directly: {source}"
    );
    assert!(
        source.contains(
            "temporal_runtime::encode_search_attribute_double(input.ratio as f64).expect(\"search_attribute double value must be finite at runtime\")"
        ),
        "float field ref must cast `as f64`: {source}"
    );
}

#[test]
fn search_attributes_field_ref_to_unsupported_type_is_rejected() {
    // bytes / message / enum field refs still fall through to the
    // standard "does not yet honour" diagnostic — encoder coverage
    // now spans string / int64 / bool / double / float scalars, but
    // not these.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_field_bytes.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = { \"K\": this.blob }"
            };
          }
        }
        message In  { bytes blob = 1; }
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files).unwrap_err().to_string();
    assert!(
        err.contains("search_attributes") && err.contains("does not yet honour"),
        "unsupported-type field ref must surface the unsupported diagnostic: {err}"
    );
}

#[test]
fn search_attributes_field_ref_to_repeated_field_is_rejected() {
    // Repeated fields fall through regardless of element type — the
    // encoders are scalar-only.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_field_repeated.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root = { \"K\": this.tags }"
            };
          }
        }
        message In  { repeated string tags = 1; }
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files).unwrap_err().to_string();
    assert!(
        err.contains("search_attributes") && err.contains("does not yet honour"),
        "repeated field ref must surface the unsupported diagnostic: {err}"
    );
}

#[test]
fn search_attributes_richer_expressions_still_rejected() {
    // R7 slice 1 explicitly does NOT support field references or
    // literal key/value entries — those land in slices 2 / 3. The
    // existing rejection diagnostic must still fire so users see the
    // boundary clearly.
    let (pool, files, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package sa_complex.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:        "tq"
              search_attributes: "root.CustomerId = this.customer_id"
            };
          }
        }
        message In  { string customer_id = 1; }
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files).unwrap_err().to_string();
    assert!(
        err.contains("search_attributes") && err.contains("does not yet honour"),
        "expressions beyond `root = {{}}` must still be rejected with the standard diagnostic: {err}"
    );
}

/// never regress.
#[test]
fn unsupported_field_support_status_table() {
    // (case label, proto snippet, expected substring in the error).
    // The case label is only used in failure messages. The proto snippet
    // is wrapped into a full input.proto before compilation. The expected
    // substring is the field name surfaced by the diagnostic — the wrapping
    // `does not yet honour` phrase is asserted once at the end.
    struct Case {
        label: &'static str,
        snippet: &'static str,
        expect_field: &'static str,
    }

    // Each snippet declares its own service to keep cases independent.
    // Workflows always set task_queue so the case fails on the rejection
    // we're targeting, not on the missing-task-queue validator.
    let cases: &[Case] = &[
        Case {
            label: "WorkflowOptions.typed_search_attributes",
            snippet: r#"
              service Svc {
                rpc Run(In) returns (Out) {
                  option (temporal.v1.workflow) = {
                    task_queue: "tq"
                    typed_search_attributes: "root = {}"
                  };
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "typed_search_attributes",
        },
        Case {
            label: "WorkflowOptions.versioning_behavior",
            snippet: r#"
              service Svc {
                rpc Run(In) returns (Out) {
                  option (temporal.v1.workflow) = {
                    task_queue: "tq"
                    versioning_behavior: VERSIONING_BEHAVIOR_PINNED
                  };
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "versioning_behavior",
        },
        Case {
            label: "WorkflowOptions.Update[].xns",
            snippet: r#"
              service Svc {
                rpc Run(In) returns (Out) {
                  option (temporal.v1.workflow) = {
                    task_queue: "tq"
                    update: [{ ref: "Patch", xns: {} }]
                  };
                }
                rpc Patch(In) returns (Out) {
                  option (temporal.v1.update) = {};
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "xns",
        },
        Case {
            label: "WorkflowOptions.patches",
            snippet: r#"
              service Svc {
                rpc Run(In) returns (Out) {
                  option (temporal.v1.workflow) = {
                    task_queue: "tq"
                    patches: [{ version: PV_64, mode: PVM_ENABLED }]
                  };
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "patches",
        },
        Case {
            label: "WorkflowOptions.namespace",
            snippet: r#"
              service Svc {
                rpc Run(In) returns (Out) {
                  option (temporal.v1.workflow) = {
                    task_queue: "tq"
                    namespace: "legacy"
                  };
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "namespace",
        },
        Case {
            label: "ServiceOptions.patches",
            snippet: r#"
              service Svc {
                option (temporal.v1.service) = {
                  task_queue: "tq"
                  patches: [{ version: PV_64, mode: PVM_ENABLED }]
                };
                rpc Run(In) returns (Out) {
                  option (temporal.v1.workflow) = {};
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "patches",
        },
        Case {
            label: "ServiceOptions.namespace",
            snippet: r#"
              service Svc {
                option (temporal.v1.service) = {
                  task_queue: "tq"
                  namespace: "legacy"
                };
                rpc Run(In) returns (Out) {
                  option (temporal.v1.workflow) = {};
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "namespace",
        },
    ];

    for case in cases {
        let source = format!(
            "syntax = \"proto3\";\npackage support_status.v1;\nimport \"temporal/v1/temporal.proto\";\n{}",
            case.snippet,
        );
        let (pool, files_to_generate, _tmp) = compile_fixture_inline(&source);
        let err = match parse::parse(&pool, &files_to_generate) {
            Ok(_) => panic!("{}: expected parse to fail, but it succeeded", case.label),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains(case.expect_field),
            "{}: diagnostic must name `{}`, got: {err}",
            case.label,
            case.expect_field,
        );
        assert!(
            err.contains("does not yet honour"),
            "{}: diagnostic must use the standard 'does not yet honour' phrasing, got: {err}",
            case.label,
        );
    }
}

/// `WorkflowOptions.enable_eager_start` is the first runtime-affecting
/// workflow option to graduate from "rejected" to "supported". It plumbs
/// straight through to the bridge's `WorkflowStartOptions.enable_eager_workflow_start`
/// so the server can satisfy the start request from a local worker.
/// The generated code must:
///  1. Carry an `enable_eager_workflow_start: Option<bool>` on StartOptions.
///  2. Resolve the caller's override against the proto-declared default.
///  3. Forward the resolved bool to `start_workflow_proto`.
#[test]
fn enable_eager_start_flows_into_start_options() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package eager.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue:         "tq"
              enable_eager_start:  true
            };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let svc = &services[0];
    assert!(
        svc.workflows[0].enable_eager_workflow_start,
        "model must carry the proto-declared default"
    );

    let source = render::render(svc, &Default::default());
    assert!(
        source.contains("pub enable_eager_workflow_start: Option<bool>,"),
        "StartOptions must expose the field for caller overrides: {source}"
    );
    assert!(
        source.contains(
            "let enable_eager_workflow_start = opts.enable_eager_workflow_start.unwrap_or(true);"
        ),
        "start path must fold the proto default in (true here): {source}"
    );
    assert!(
        source.contains("enable_eager_workflow_start,"),
        "resolved value must be passed to the runtime bridge call: {source}"
    );
}

#[test]
fn workflow_id_conflict_policy_flows_into_start_options() {
    // R5: `workflow_id_conflict_policy` moves from rejected to supported,
    // wired through to `WorkflowStartOptions.id_conflict_policy` on the
    // bridge. Defaults fold into the start path so callers who leave
    // `StartOptions::id_conflict_policy` as `None` still get the proto-
    // declared default.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package conflict.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              workflow_id_conflict_policy: WORKFLOW_ID_CONFLICT_POLICY_USE_EXISTING
            };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let svc = &services[0];
    use protoc_gen_rust_temporal::model::IdConflictPolicy;
    assert_eq!(
        svc.workflows[0].id_conflict_policy,
        Some(IdConflictPolicy::UseExisting),
        "model must carry the proto-declared default"
    );

    let source = render::render(svc, &Default::default());
    assert!(
        source.contains(
            "pub id_conflict_policy: Option<temporal_runtime::WorkflowIdConflictPolicy>,"
        ),
        "StartOptions must expose the conflict-policy field: {source}"
    );
    assert!(
        source.contains("let id_conflict_policy = opts.id_conflict_policy.or(Some(temporal_runtime::WorkflowIdConflictPolicy::UseExisting));"),
        "start path must fold the proto default in (UseExisting here): {source}"
    );
    assert!(
        source.contains("id_conflict_policy,"),
        "resolved value must be forwarded to the bridge call: {source}"
    );
}

#[test]
fn start_options_exposes_with_proto_defaults_chainable_underlay() {
    // R6 ergonomics — `<Wf>StartOptions::with_proto_defaults(self) -> Self`
    // is the chain-style underlay sibling of `proto_defaults()`. Where
    // `proto_defaults()` discards current state (must be the *first* call
    // in a chain), `with_proto_defaults()` only fills fields that are
    // still `None` (can be the *last* call without overwriting user-set
    // fields). Lets callers spell:
    //     `MyOpts::default().with_workflow_id("x").with_proto_defaults()`
    // without remembering call ordering.
    let services = parse_and_validate("full_workflow");
    let opts_fixture = load_fixture_options("full_workflow");
    let source = render::render(&services[0], &opts_fixture);
    assert!(
        source.contains("pub fn with_proto_defaults(mut self) -> Self {"),
        "missing with_proto_defaults fn signature: {source}"
    );
    // Underlay must guard each fold with `is_none()` so user-set fields
    // survive — that is the whole point of the method versus
    // `proto_defaults()`.
    assert!(
        source.contains("if self.id_reuse_policy.is_none() {")
            || source.contains("if self.execution_timeout.is_none() {")
            || source.contains("if self.run_timeout.is_none() {")
            || source.contains("if self.task_timeout.is_none() {"),
        "with_proto_defaults must guard each fold with is_none(): {source}"
    );
}

#[test]
fn with_proto_defaults_omitted_when_no_defaults_declared() {
    // Mirror of `proto_defaults()`'s emit guard: if the workflow declares
    // no default-bearing fields, neither method should emit. The
    // `if !defaults.is_empty()` block in `render_start_options` must
    // gate both consistently — otherwise tooling that enumerates option
    // helpers would spuriously list a no-op `with_proto_defaults()`.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        !source.contains("with_proto_defaults"),
        "with_proto_defaults must not emit when no defaults declared: {source}"
    );
}

#[test]
fn proto_defaults_folds_id_conflict_policy_and_eager_start() {
    // R6 ergonomics — `<Wf>StartOptions::proto_defaults()` previously folded
    // only id_reuse_policy + the three timeouts. id_conflict_policy and
    // enable_eager_workflow_start also have proto-declared defaults; both
    // must be folded so callers spelling `proto_defaults()` get the same
    // resolved-default state the start path bakes in.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package proto_defaults_extra.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              workflow_id_conflict_policy: WORKFLOW_ID_CONFLICT_POLICY_USE_EXISTING
              enable_eager_start: true
            };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let source = render::render(&services[0], &Default::default());

    // Per-field accessors must exist for both new defaults.
    assert!(
        source.contains(
            "pub fn default_id_conflict_policy() -> temporal_runtime::WorkflowIdConflictPolicy {"
        ),
        "missing default_id_conflict_policy helper: {source}"
    );
    assert!(
        source.contains("pub fn default_enable_eager_workflow_start() -> bool {"),
        "missing default_enable_eager_workflow_start helper: {source}"
    );

    // proto_defaults() must fold both into the returned struct.
    assert!(
        source.contains("opts.id_conflict_policy = Some(Self::default_id_conflict_policy());"),
        "proto_defaults must fold id_conflict_policy: {source}"
    );
    assert!(
        source.contains(
            "opts.enable_eager_workflow_start = Some(Self::default_enable_eager_workflow_start());"
        ),
        "proto_defaults must fold enable_eager_workflow_start: {source}"
    );
}

#[test]
fn proto_defaults_skips_eager_start_when_proto_default_false() {
    // The eager-start fold only fires when the proto explicitly opts in.
    // `false` is `bool::default()` so emitting a helper / fold for it
    // would just be noise — keep proto_defaults silent in that case.
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        !source.contains("default_enable_eager_workflow_start"),
        "no eager helper should emit when proto leaves enable_eager_start unset: {source}"
    );
    assert!(
        !source.contains("opts.enable_eager_workflow_start = Some("),
        "proto_defaults must not fold eager-start when proto omits it: {source}"
    );
}

#[test]
fn workflow_id_conflict_policy_absent_resolves_to_none() {
    // Without the proto field set, the model must hold `None` and the
    // start path should not bake in any default — `None` lets the server
    // pick its own conflict default.
    let services = parse_and_validate("minimal_workflow");
    assert!(
        services[0].workflows[0].id_conflict_policy.is_none(),
        "absent proto field must keep model None"
    );
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("let id_conflict_policy = opts.id_conflict_policy;"),
        "start path must rebind opts directly when no default exists: {source}"
    );
}

#[test]
fn enable_eager_start_defaults_to_false_when_proto_omits_it() {
    let services = parse_and_validate("minimal_workflow");
    assert!(
        !services[0].workflows[0].enable_eager_workflow_start,
        "absent proto field must produce model `false`, matching the SDK default"
    );
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains(
            "let enable_eager_workflow_start = opts.enable_eager_workflow_start.unwrap_or(false);"
        ),
        "start path should baseline to false: {source}"
    );
}

/// `docs/SUPPORT-STATUS.md` is the published index of every annotation
/// field's status. The diagnostic-coverage table above already enforces that
/// each rejection rule fires; this test enforces the *companion* invariant:
/// every field name a rejection rule mentions must also appear in the doc,
/// so users reading the table can find the limitation without spelunking
/// `parse.rs`. Drift between the rejection lists and the doc is the most
/// likely silent-drop regression on this side of R1.
#[test]
fn support_status_doc_lists_every_rejected_field() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let parse_src =
        std::fs::read_to_string(crate_root.join("src/parse.rs")).expect("read parse.rs");
    let doc = std::fs::read_to_string(
        crate_root
            .join("..")
            .join("..")
            .join("docs/SUPPORT-STATUS.md"),
    )
    .expect("read docs/SUPPORT-STATUS.md");

    // Pull every literal in `unsupported.push("…")`. That's the canonical
    // place where each rejected field is named — adding a new rejection
    // without updating the doc fails this assertion.
    let mut rejected_fields: Vec<String> = Vec::new();
    for line in parse_src.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("unsupported.push(\"") {
            if let Some(end) = rest.find("\")") {
                rejected_fields.push(rest[..end].to_string());
            }
        }
    }
    assert!(
        !rejected_fields.is_empty(),
        "regex extraction is wrong: no rejected fields found in parse.rs"
    );

    for field in &rejected_fields {
        // Strip the "(deprecated)" suffix and any trailing whitespace before
        // the lookup — the doc names the bare field, the diagnostic decorates
        // deprecated ones.
        let needle = field
            .split_whitespace()
            .next()
            .expect("non-empty field name");
        assert!(
            doc.contains(&format!("`{needle}`")),
            "docs/SUPPORT-STATUS.md must mention `{needle}` (declared rejected in parse.rs but not documented). \
             Add a row to the relevant Options table."
        );
    }
}

#[test]
fn cross_service_ref_with_typo_fails_at_parse() {
    // R1 — parse-time resolution catches typos before validate's
    // emit-not-implemented rejection fires. `Notifictions` (with the
    // deliberate typo) doesn't resolve to any rpc in the pool.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package xs.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Workflows {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "xs.v1.Notifictions.Cancel" }]
            };
          }
        }

        service Notifications {
          rpc Cancel(In) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
        }

        message In {} message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("doesn't resolve to any rpc in the descriptor pool"),
        "typo must produce an unresolved-target diagnostic: {err}"
    );
    assert!(
        err.contains("xs.v1.Notifictions.Cancel"),
        "diagnostic must echo the offending ref so users can search it: {err}"
    );
}

#[test]
fn cross_service_ref_to_wrong_annotation_kind_fails_at_parse() {
    // The target rpc exists but is annotated as a workflow, not a
    // signal. Parse must catch the wrong-kind mismatch before validate.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package xs.v1;
        import "temporal/v1/temporal.proto";

        service Workflows {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "xs.v1.Notifications.RunIt" }]
            };
          }
        }

        service Notifications {
          rpc RunIt(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "n" };
          }
        }

        message In {} message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("does not carry `(temporal.v1.signal)`"),
        "wrong-kind target must surface the missing annotation: {err}"
    );
}

/// Cross-service refs — Go's plugin resolves `ref: "other.v1.OtherService.Cancel"`
/// R1 — full cross-service ref support: a workflow attaches a signal
/// declared on a *different* service via the fully-qualified
/// `pkg.Service.Method` syntax. Parse resolves the target through the
/// DescriptorPool, validate accepts the ref, and render emits a typed
/// Handle method that uses the target's wire-format registered name
/// and proto I/O types.
#[test]
fn cross_service_signal_with_start_rejects_empty_signal_input() {
    // R1 — the Empty-with-start guard now applies to cross-service
    // refs too. A cross-service signal with `start: true` and Empty
    // input must be rejected with the same diagnostic as the
    // same-service case, since the with_start emit can't payload
    // an empty proto across services either.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package xs_ws_bad.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Workflows {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "xs_ws_bad.v1.Notifications.Cancel" start: true }]
            };
          }
        }

        service Notifications {
          // Empty input — the with_start emit can't carry a payload.
          rpc Cancel(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
        }

        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse must succeed");
    let workflows_svc = services
        .iter()
        .find(|s| s.service == "Workflows")
        .expect("Workflows service must be in the model");
    let render_opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = protoc_gen_rust_temporal::validate::validate(workflows_svc, &render_opts)
        .expect_err("empty-input cross-service signal-with-start must be rejected")
        .to_string();
    assert!(
        err.contains("start:true")
            && err.contains("Empty")
            && err.contains("xs_ws_bad.v1.Notifications.Cancel"),
        "diagnostic must surface the ref + empty + start:true context: {err}"
    );
}

#[test]
fn cross_service_signal_ref_with_start_emits_with_start_fn() {
    // R1 — when a cross-service `signal` ref carries `start: true`,
    // the workflow gains a `<signal>_with_start` free function that
    // atomically starts the workflow and signals the cross-service
    // handler. Previously the with_start emit dropped cross-service
    // refs silently.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package xs_ws.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Workflows {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "xs_ws.v1.Notifications.Cancel" start: true }]
            };
          }
        }

        service Notifications {
          rpc Cancel(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
        }

        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let workflows_svc = services
        .iter()
        .find(|s| s.service == "Workflows")
        .expect("Workflows service must be in the model");
    let source = render::render(workflows_svc, &Default::default());
    assert!(
        source.contains("pub async fn cancel_with_start("),
        "must emit `cancel_with_start` free fn for the cross-service signal ref: {source}"
    );
}

#[test]
fn cross_service_update_ref_emits_handle_method() {
    // R1 — cross-service update refs produce a typed Handle method
    // that the workflow's typed handle exposes. Mirrors the
    // signal-side test that's been in place since the cross-service
    // emit landed.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package xs_u.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Workflows {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "xs_u.v1.Bumps.Apply" }]
            };
          }
        }

        service Bumps {
          rpc Apply(google.protobuf.Empty) returns (google.protobuf.Empty) {
            option (temporal.v1.update) = {};
          }
        }

        message In  {}
        message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let workflows_svc = services
        .iter()
        .find(|s| s.service == "Workflows")
        .expect("Workflows service must be in the model");
    let source = render::render(workflows_svc, &Default::default());
    // The fabricated UpdateModel produces an `apply` handle method on
    // the workflow's typed handle struct.
    assert!(
        source.contains("pub async fn apply(&self"),
        "cross-service update ref must produce a typed handle method: {source}"
    );
}

#[test]
fn cross_service_query_ref_emits_handle_method() {
    // R1 — cross-service query refs produce a typed Handle method
    // that the workflow's typed handle exposes.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package xs_q.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Workflows {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              query: [{ ref: "xs_q.v1.Status.Get" }]
            };
          }
        }

        service Status {
          rpc Get(google.protobuf.Empty) returns (StatusOutput) {
            option (temporal.v1.query) = {};
          }
        }

        message In  {}
        message Out {}
        message StatusOutput { string phase = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let workflows_svc = services
        .iter()
        .find(|s| s.service == "Workflows")
        .expect("Workflows service must be in the model");
    let source = render::render(workflows_svc, &Default::default());
    assert!(
        source.contains("pub async fn get(&self"),
        "cross-service query ref must produce a typed handle method: {source}"
    );
}

#[test]
fn cross_service_signal_ref_emits_handle_method() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package xs.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Workflows {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "xs.v1.Notifications.Cancel" }]
            };
          }
        }

        service Notifications {
          rpc Cancel(In) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
          }
        }

        message In  { string name = 1; }
        message Out { string id = 1; }
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let workflows_svc = services
        .iter()
        .find(|s| s.service == "Workflows")
        .expect("Workflows service parsed");
    let opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    // Validate must now accept the cross-service ref (parse already
    // resolved + captured target metadata).
    validate::validate(workflows_svc, &opts)
        .expect("validate must accept resolved cross-service ref");

    // Render emits a `cancel` Handle method that targets the
    // cross-service registered name.
    let source = render::render(workflows_svc, &opts);
    assert!(
        source.contains("pub async fn cancel(&self, input: In) -> Result<()>"),
        "cross-service signal must produce a typed Handle method using the target's input type: {source}"
    );
    assert!(
        source.contains("temporal_runtime::signal_proto(&self.inner, \"xs.v1.Notifications.Cancel\", &input).await"),
        "the bridge call must use the cross-service registered name on the wire: {source}"
    );
}

/// R1 — co-annotation support. Cludden's Go plugin allows a single rpc to
/// carry `(temporal.v1.activity)` alongside one of the primary kinds
/// (workflow / signal / update). The Rust plugin now does the same: the
/// activity bucket lives in a separate trait surface that doesn't collide
/// with the client / handler emit, so combinations are safe.
///
/// Combinations involving two primary kinds (workflow + signal,
/// workflow + query, etc.) remain refused because they would share
/// generated symbols.
#[test]
fn co_annotation_workflow_plus_activity_produces_both_entries() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package co_anno.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = { task_queue: "tq" };
            option (temporal.v1.activity) = {};
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let svc = &services[0];
    assert_eq!(svc.workflows.len(), 1, "Run should land in workflows");
    assert_eq!(
        svc.activities.len(),
        1,
        "Run should also land in activities"
    );
    assert_eq!(svc.workflows[0].rpc_method, "Run");
    assert_eq!(svc.activities[0].rpc_method, "Run");
    let opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    validate::validate(svc, &opts).expect("validate must accept activity + one primary kind");
}

#[test]
fn co_annotation_signal_plus_activity_produces_both_entries() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package co_anno.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              signal: [{ ref: "Notify" }]
            };
          }
          rpc Notify(In) returns (google.protobuf.Empty) {
            option (temporal.v1.signal) = {};
            option (temporal.v1.activity) = {};
          }
        }
        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let svc = &services[0];
    assert_eq!(svc.signals.len(), 1);
    assert_eq!(svc.activities.len(), 1);
    let opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    validate::validate(svc, &opts).expect("validate must accept signal + activity");
}

#[test]
fn co_annotation_two_primary_kinds_still_rejected() {
    // workflow + signal on a single rpc would have it appear as both a
    // top-level client method *and* a sibling-attached signal handler —
    // generated symbols would collide. Stay rejected.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package co_anno.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Notify(In) returns (google.protobuf.Empty) {
            option (temporal.v1.workflow) = { task_queue: "tq" };
            option (temporal.v1.signal) = {};
          }
        }
        message In {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("multiple non-activity Temporal annotations")
            && err.contains("workflow")
            && err.contains("signal"),
        "two-primary-kinds combo must still be refused: {err}"
    );
}

#[test]
fn workflow_schema_defaults_apply_at_start() {
    // full_workflow declares id_reuse_policy + 3 timeouts on the proto;
    // the start path must fold those defaults in when the caller leaves
    // the StartOptions field as `None`, so mixed Rust/Go workers driving
    // the same workflow get the same effective options.
    let services = parse_and_validate("full_workflow");
    let source = render::render(&services[0], &Default::default());
    for fragment in [
        "let id_reuse_policy = opts.id_reuse_policy.or(Some(temporal_runtime::WorkflowIdReusePolicy::AllowDuplicateFailedOnly));",
        "let execution_timeout = opts.execution_timeout.or(Some(Duration::from_secs(7200)));",
        "let run_timeout = opts.run_timeout.or(Some(Duration::from_secs(3600)));",
        "let task_timeout = opts.task_timeout.or(Some(Duration::from_secs(60)));",
    ] {
        assert!(
            source.contains(fragment),
            "start path should fold proto default in: {fragment}\n--- source ---\n{source}"
        );
    }
    // Three start paths (Client::run, bootstrap_with_start, reconfigure_with_start)
    // each get their own resolution block — guard against accidental dedup.
    let occurrences = source
        .matches("let id_reuse_policy = opts.id_reuse_policy")
        .count();
    assert_eq!(
        occurrences, 3,
        "expected id_reuse_policy default applied in all 3 start paths, got {occurrences}"
    );
}

#[test]
fn workflow_without_schema_defaults_passes_through() {
    // minimal_workflow declares no id_reuse_policy / timeouts on the proto,
    // so the start path should pass `opts.<field>` through unchanged (no
    // synthesized default that the proto didn't request).
    let services = parse_and_validate("minimal_workflow");
    let source = render::render(&services[0], &Default::default());
    assert!(
        source.contains("let id_reuse_policy = opts.id_reuse_policy;"),
        "no-default fields should still bind locals so the trailing call site stays uniform"
    );
    assert!(
        !source.contains("opts.id_reuse_policy.or(Some("),
        "no proto-level id_reuse_policy declared — must not synthesize a default"
    );
}
