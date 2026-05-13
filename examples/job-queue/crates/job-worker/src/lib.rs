//! `RunJob` workflow + stub activities.
//!
//! The activities are deliberately stubbed (sleeps + canned data): real
//! subprocess execution is out of scope for v1 per SPEC.md. The demo proves
//! the *contract surface* of the plugin-generated client, not job semantics.

// Required by temporalio-macros' #[workflow_methods] / #[activities] expansions
// — they call `.boxed_local()` / `.boxed()` from `futures_util::FutureExt`
// without re-exporting the trait. See docs/sdk-shape.md §6 in the sibling repo.
#[allow(unused_imports)]
use futures_util::FutureExt as _;

pub mod state;

use std::time::Duration;

use jobs_proto::TypedProtoMessage;
use jobs_proto::jobs::v1::{
    CancelJobInput, GetStatusInput, JobInput, JobOutput, JobStatusOutput, PrepareWorkspaceInput,
};
use jobs_proto::jobs_v1_job_service_temporal as temporal_contract;
// The inner attrs (`init`, `run`, `signal`, `query`, `activity`) look unused
// lexically because the outer `#[workflow_methods]` / `#[activities]` macros
// consume them during expansion. The imports are still required so name
// resolution at attribute lookup time finds them.
#[allow(unused_imports)]
use temporalio_macros::{
    activities, activity, init, query, run, signal, workflow, workflow_methods,
};
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use temporalio_sdk::{
    ActivityOptions, SyncWorkflowContext, Worker, WorkerOptions, WorkflowContext,
    WorkflowContextView, WorkflowResult,
};

pub use state::Stage;

/// Build `WorkerOptions` for the demo. Kept inside the lib crate because
/// `#[workflow_methods]` makes the workflow type's visibility restricted, so
/// `main.rs` can't reach `RunJob` directly.
pub fn build_options(task_queue: &str) -> WorkerOptions {
    WorkerOptions::new(task_queue).build()
}

pub fn register(worker: &mut Worker) -> &mut Worker {
    temporal_contract::register_run_job_workflow::<RunJob>(worker);
    temporal_contract::register_job_service_activities(worker, JobActivities)
}

// ── activities ────────────────────────────────────────────────────────────

pub struct JobActivities;

#[activities]
impl JobActivities {
    /// "Prepare" stage — stubbed 1s sleep.
    #[activity(name = temporal_contract::PREPARE_WORKSPACE_ACTIVITY_NAME)]
    pub async fn prepare_workspace(
        _ctx: ActivityContext,
        input: TypedProtoMessage<PrepareWorkspaceInput>,
    ) -> Result<(), ActivityError> {
        prepare_workspace(input.into_inner().name)
            .await
            .map_err(ActivityError::from)
    }

    /// "Execute" stage — stubbed sleep capped to the caller-supplied timeout,
    /// then returns canned output.
    #[activity(name = temporal_contract::EXECUTE_COMMAND_ACTIVITY_NAME)]
    pub async fn execute_command(
        _ctx: ActivityContext,
        input: TypedProtoMessage<JobInput>,
    ) -> Result<TypedProtoMessage<JobOutput>, ActivityError> {
        execute_command(input.into_inner())
            .await
            .map(TypedProtoMessage)
            .map_err(ActivityError::from)
    }

    /// "Collect" stage — stubbed 1s sleep.
    #[activity(name = temporal_contract::COLLECT_OUTPUT_ACTIVITY_NAME)]
    pub async fn collect_output(_ctx: ActivityContext) -> Result<(), ActivityError> {
        collect_output().await.map_err(ActivityError::from)
    }
}

impl temporal_contract::JobServiceActivities for JobActivities {
    fn prepare_workspace(
        &self,
        _ctx: ActivityContext,
        input: PrepareWorkspaceInput,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        prepare_workspace(input.name)
    }

    fn execute_command(
        &self,
        _ctx: ActivityContext,
        input: JobInput,
    ) -> impl std::future::Future<Output = anyhow::Result<JobOutput>> + Send {
        execute_command(input)
    }

    fn collect_output(
        &self,
        _ctx: ActivityContext,
        _input: (),
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        collect_output()
    }
}

async fn prepare_workspace(name: String) -> anyhow::Result<()> {
    tracing::debug!(name = %name, "prepare_workspace");
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

async fn execute_command(input: JobInput) -> anyhow::Result<JobOutput> {
    let cap = input.timeout_seconds.clamp(1, 60) as u64;
    let dur = Duration::from_secs(3).min(Duration::from_secs(cap));
    tracing::debug!(command = %input.command, "execute_command");
    tokio::time::sleep(dur).await;
    Ok(JobOutput {
        exit_code: 0,
        stdout: format!("[stub] ran `{}` (name={})", input.command, input.name),
        stderr: String::new(),
    })
}

async fn collect_output() -> anyhow::Result<()> {
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

// ── workflow ──────────────────────────────────────────────────────────────
//
// Per docs/sdk-shape.md:
//   - #[workflow] goes on the struct (not the impl block).
//   - The struct must NOT be `pub` — `pub(crate)` is the highest visibility
//     that compiles with the macro-generated internal `Run` type.
//   - #[run(name = "...")] carries the registration name; the name on
//     #[workflow] is silently ignored.
//   - Input flows through #[init], not #[run].
//   - Return type is WorkflowResult<T> (= Result<T, WorkflowTermination>).
//   - WorkflowContextView takes NO generic; WorkflowContext<W> does.

#[workflow]
pub(crate) struct RunJob {
    input: JobInput,
    stage: Stage,
    progress_pct: u32,
    cancelled: bool,
    cancel_reason: Option<String>,
}

#[allow(dead_code)]
#[workflow_methods]
impl RunJob {
    #[init]
    fn new(_ctx: &WorkflowContextView, input: TypedProtoMessage<JobInput>) -> Self {
        Self {
            input: input.into_inner(),
            stage: Stage::Pending,
            progress_pct: 0,
            cancelled: false,
            cancel_reason: None,
        }
    }

    #[run(name = temporal_contract::RUN_JOB_WORKFLOW_NAME)]
    async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<TypedProtoMessage<JobOutput>> {
        // Stage: Preparing
        ctx.state_mut(|s| {
            s.stage = Stage::Preparing;
            s.progress_pct = 10;
        });
        let name = ctx.state(|s| s.input.name.clone());
        ctx.start_activity(
            JobActivities::prepare_workspace,
            TypedProtoMessage(PrepareWorkspaceInput { name }),
            ActivityOptions::start_to_close_timeout(Duration::from_secs(30)),
        )
        .await?;

        if ctx.state(|s| s.cancelled) {
            return Ok(cancelled_output(ctx));
        }

        // Stage: Executing
        ctx.state_mut(|s| {
            s.stage = Stage::Executing;
            s.progress_pct = 50;
        });
        let input = TypedProtoMessage(ctx.state(|s| s.input.clone()));
        let out: TypedProtoMessage<JobOutput> = ctx
            .start_activity(
                JobActivities::execute_command,
                input,
                ActivityOptions::start_to_close_timeout(Duration::from_secs(120)),
            )
            .await?;

        if ctx.state(|s| s.cancelled) {
            return Ok(cancelled_output(ctx));
        }

        // Stage: Collecting
        ctx.state_mut(|s| {
            s.stage = Stage::Collecting;
            s.progress_pct = 90;
        });
        ctx.start_activity(
            JobActivities::collect_output,
            (),
            ActivityOptions::start_to_close_timeout(Duration::from_secs(30)),
        )
        .await?;

        ctx.state_mut(|s| {
            s.stage = Stage::Done;
            s.progress_pct = 100;
        });
        Ok(out)
    }

    #[signal(name = temporal_contract::CANCEL_JOB_SIGNAL_NAME)]
    fn cancel_job(
        &mut self,
        _ctx: &mut SyncWorkflowContext<Self>,
        input: TypedProtoMessage<CancelJobInput>,
    ) {
        let i = input.into_inner();
        self.cancelled = true;
        self.cancel_reason = Some(i.reason);
    }

    #[query(name = temporal_contract::GET_STATUS_QUERY_NAME)]
    fn get_status(
        &self,
        _ctx: &WorkflowContextView,
        _input: TypedProtoMessage<GetStatusInput>,
    ) -> TypedProtoMessage<JobStatusOutput> {
        TypedProtoMessage(JobStatusOutput {
            stage: self.stage.as_wire().to_string(),
            progress_pct: self.progress_pct,
        })
    }
}

impl temporal_contract::RunJobDefinition for RunJob {
    type Input = JobInput;
    type Output = JobOutput;
}

fn cancelled_output(ctx: &mut WorkflowContext<RunJob>) -> TypedProtoMessage<JobOutput> {
    let reason = ctx.state(|s| s.cancel_reason.clone().unwrap_or_default());
    ctx.state_mut(|s| {
        s.stage = Stage::Cancelled;
    });
    TypedProtoMessage(JobOutput {
        exit_code: 130,
        stdout: String::new(),
        stderr: format!("cancelled: {reason}"),
    })
}
