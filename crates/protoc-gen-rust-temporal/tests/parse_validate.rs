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
fn workflow_update_ref_with_cli_is_rejected_at_parse() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package bad.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Touch", cli: {} }]
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
    let err = parse::parse(&pool, &files_to_generate)
        .expect_err("cli on update ref must be rejected at parse")
        .to_string();
    assert!(
        err.contains("cli") && err.contains("update[ref=Touch]"),
        "parse error must surface cli + update ref name, got: {err}"
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
fn workflow_cli_name_is_rejected() {
    // Honouring cli.ignore while silently dropping cli.name would surprise
    // users who expect `cli: { name: "foo" }` to change the subcommand name.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              cli: { name: "custom" }
            };
          }
        }
        message In {} message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("cli.name") && err.contains("does not yet honour"),
        "expected cli.name diagnostic, got: {err}"
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
        source.contains("#[derive(temporal_runtime::clap::Parser)]"),
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
        "pub async fn reconfigure(&self, input: ReconfigureInput, wait_policy: temporal_runtime::WaitPolicy)",
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
              task_queue:           "tq"
              search_attributes:    "string.foo = \"bar\""
              wait_for_cancellation: true
              versioning_behavior:   VERSIONING_BEHAVIOR_PINNED
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
        "wait_for_cancellation",
        "versioning_behavior",
    ] {
        assert!(
            err.contains(field),
            "diagnostic should list {field}, got: {err}"
        );
    }
}

#[test]
fn update_with_id_template_is_rejected() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "google/protobuf/empty.proto";
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Patch" }]
            };
          }
          rpc Patch(In) returns (Out) {
            option (temporal.v1.update) = { id: "patch-{{ .Field }}" };
          }
        }
        message In { string field = 1; }
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("temporal.v1.update") && err.contains("id"),
        "expected update-id diagnostic, got: {err}"
    );
}

#[test]
fn update_with_deprecated_wait_policy_is_rejected() {
    // The deprecated `wait_policy` field on UpdateOptions still appears on
    // legacy protos ported from cludden's Go plugin. Silently ignoring it
    // would let a user lose their default WaitPolicy on the Rust client.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Patch" }]
            };
          }
          rpc Patch(In) returns (Out) {
            option (temporal.v1.update) = { wait_policy: WAIT_POLICY_ACCEPTED };
          }
        }
        message In {}
        message Out {}
        "#,
    );
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("wait_policy"),
        "expected deprecated-wait_policy diagnostic, got: {err}"
    );
}

#[test]
fn workflow_update_ref_with_conflict_policy_is_rejected() {
    // The bridge's update-with-start path hardcodes UseExisting; a per-
    // update conflict_policy override on WorkflowOptions.update[] would
    // be silently dropped. Refuse the proto rather than ship the wrong
    // policy.
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Run(In) returns (Out) {
            option (temporal.v1.workflow) = {
              task_queue: "tq"
              update: [{ ref: "Patch" workflow_id_conflict_policy: WORKFLOW_ID_CONFLICT_POLICY_FAIL }]
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
    let err = parse::parse(&pool, &files_to_generate)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("workflow_id_conflict_policy") && err.contains("Patch"),
        "expected nested-update conflict_policy diagnostic, got: {err}"
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

#[test]
fn activity_with_timeouts_is_rejected() {
    let (pool, files_to_generate, _tmp) = compile_fixture_inline(
        r#"
        syntax = "proto3";
        package guard.v1;
        import "temporal/v1/temporal.proto";

        service Svc {
          rpc Work(In) returns (Out) {
            option (temporal.v1.activity) = {
              task_queue: "workers"
              start_to_close_timeout: { seconds: 30 }
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
    assert!(
        err.contains("task_queue") && err.contains("start_to_close_timeout"),
        "expected activity diagnostic listing both unsupported fields, got: {err}"
    );
}

/// Table-driven coverage of every `reject_unsupported_*` branch in
/// `parse.rs`. When you add a new rejection rule, drop a row here naming
/// the field and an isolating proto snippet. The roadmap (R1) requires that
/// every unsupported-field diagnostic fire under test so silent drops can
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
            label: "WorkflowOptions.parent_close_policy",
            snippet: r#"
              service Svc {
                rpc Run(In) returns (Out) {
                  option (temporal.v1.workflow) = {
                    task_queue: "tq"
                    parent_close_policy: PARENT_CLOSE_POLICY_ABANDON
                  };
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "parent_close_policy",
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
            label: "UpdateOptions.wait_for_stage",
            snippet: r#"
              service Svc {
                rpc Run(In) returns (Out) {
                  option (temporal.v1.workflow) = {
                    task_queue: "tq"
                    update: [{ ref: "Patch" }]
                  };
                }
                rpc Patch(In) returns (Out) {
                  option (temporal.v1.update) = { wait_for_stage: WAIT_POLICY_COMPLETED };
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "wait_for_stage",
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
            label: "WorkflowOptions.Signal[].cli",
            snippet: r#"
              import "google/protobuf/empty.proto";
              service Svc {
                rpc Run(In) returns (Out) {
                  option (temporal.v1.workflow) = {
                    task_queue: "tq"
                    signal: [{ ref: "Cancel", cli: { name: "cancel" } }]
                  };
                }
                rpc Cancel(In) returns (google.protobuf.Empty) {
                  option (temporal.v1.signal) = {};
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "cli",
        },
        Case {
            label: "ActivityOptions.schedule_to_close_timeout",
            snippet: r#"
              service Svc {
                rpc Work(In) returns (Out) {
                  option (temporal.v1.activity) = {
                    schedule_to_close_timeout: { seconds: 60 }
                  };
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "schedule_to_close_timeout",
        },
        Case {
            label: "ActivityOptions.schedule_to_start_timeout",
            snippet: r#"
              service Svc {
                rpc Work(In) returns (Out) {
                  option (temporal.v1.activity) = {
                    schedule_to_start_timeout: { seconds: 30 }
                  };
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "schedule_to_start_timeout",
        },
        Case {
            label: "ActivityOptions.heartbeat_timeout",
            snippet: r#"
              service Svc {
                rpc Work(In) returns (Out) {
                  option (temporal.v1.activity) = {
                    heartbeat_timeout: { seconds: 5 }
                  };
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "heartbeat_timeout",
        },
        Case {
            label: "ActivityOptions.wait_for_cancellation",
            snippet: r#"
              service Svc {
                rpc Work(In) returns (Out) {
                  option (temporal.v1.activity) = {
                    wait_for_cancellation: true
                  };
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "wait_for_cancellation",
        },
        Case {
            label: "ActivityOptions.retry_policy",
            snippet: r#"
              service Svc {
                rpc Work(In) returns (Out) {
                  option (temporal.v1.activity) = {
                    retry_policy: { max_attempts: 5 }
                  };
                }
              }
              message In {} message Out {}
            "#,
            expect_field: "retry_policy",
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

/// Cross-service refs — Go's plugin resolves `ref: "other.v1.OtherService.Cancel"`
/// against any sibling service in the descriptor pool. The Rust plugin does
/// not yet (R1). Users porting from Go must see an explicit "cross-service
/// refs are not yet supported" diagnostic, not the generic "no sibling rpc
/// carries…" one, or they'll spend time hunting for a missing same-service
/// signal that the Go side never had.
#[test]
fn cross_service_ref_is_rejected_with_clear_diagnostic() {
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

        message In {} message Out {}
        "#,
    );
    let services = parse::parse(&pool, &files_to_generate).expect("parse");
    let workflows_svc = services
        .iter()
        .find(|s| s.service == "Workflows")
        .expect("Workflows service parsed");
    let opts = protoc_gen_rust_temporal::options::RenderOptions::default();
    let err = validate::validate(workflows_svc, &opts)
        .expect_err("validate should fail")
        .to_string();
    assert!(
        err.contains("cross-service refs are not yet supported"),
        "diagnostic should call out cross-service refs as unsupported, got: {err}"
    );
    assert!(
        err.contains("xs.v1.Notifications.Cancel"),
        "diagnostic should quote the offending ref so users can search it, got: {err}"
    );
}

/// Co-annotations on a single rpc — Go's plugin supports several combinations
/// (workflow+activity, signal+activity, update+activity); the Rust emit does
/// not, and R1 in ROADMAP.md tracks adding support. Until then the parser
/// must refuse them so users cannot accidentally ship a service with half
/// its Temporal contract silently dropped.
#[test]
fn co_annotations_are_rejected_with_clear_diagnostic() {
    struct Case {
        label: &'static str,
        snippet: &'static str,
        expect_combo: &'static str,
    }

    // Each case attaches two `temporal.v1.*` extensions to the same rpc. The
    // returned diagnostic must name both kinds so users see which combination
    // tripped the limitation.
    let cases: &[Case] = &[
        Case {
            label: "workflow + activity",
            snippet: r#"
              service Svc {
                rpc Run(In) returns (Out) {
                  option (temporal.v1.workflow) = { task_queue: "tq" };
                  option (temporal.v1.activity) = {};
                }
              }
              message In {} message Out {}
            "#,
            expect_combo: "workflow + activity",
        },
        Case {
            label: "signal + activity",
            snippet: r#"
              import "google/protobuf/empty.proto";
              service Svc {
                rpc Notify(In) returns (google.protobuf.Empty) {
                  option (temporal.v1.signal) = {};
                  option (temporal.v1.activity) = {};
                }
              }
              message In {}
            "#,
            expect_combo: "activity + signal",
        },
        Case {
            label: "update + activity",
            snippet: r#"
              service Svc {
                rpc Patch(In) returns (Out) {
                  option (temporal.v1.update) = {};
                  option (temporal.v1.activity) = {};
                }
              }
              message In {} message Out {}
            "#,
            expect_combo: "activity + update",
        },
    ];

    for case in cases {
        let source = format!(
            "syntax = \"proto3\";\npackage co_anno.v1;\nimport \"temporal/v1/temporal.proto\";\n{}",
            case.snippet,
        );
        let (pool, files_to_generate, _tmp) = compile_fixture_inline(&source);
        let err = match parse::parse(&pool, &files_to_generate) {
            Ok(_) => panic!("{}: expected parse to fail, but it succeeded", case.label),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains(case.expect_combo),
            "{}: diagnostic must name the combination `{}`, got: {err}",
            case.label,
            case.expect_combo,
        );
        assert!(
            err.contains("co-annotations are not yet supported"),
            "{}: diagnostic must mark co-annotations as not-yet-supported (so users see the roadmap path), got: {err}",
            case.label,
        );
    }
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
