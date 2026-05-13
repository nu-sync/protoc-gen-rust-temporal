use clap::{Args, Parser, Subcommand};
use jobs_proto::jobs::v1::{CancelJobInput, GetStatusInput, JobInput};
use jobs_proto::jobs_v1_job_service_temporal::{JobServiceClient, RunJobStartOptions};
use jobs_proto::temporal_runtime;

/// CLI driver for the JobService workflow contract.
///
/// Talks directly to Temporal — does NOT go through job-api. This is the whole
/// point of the demo: two unrelated programs (axum API + this CLI) drive the
/// same workflow through the same compile-time types.
#[derive(Debug, Parser)]
#[command(name = "jobctl", version)]
struct Cli {
    /// Temporal frontend URL.
    #[arg(long, env = "TEMPORAL_URL", default_value = "http://localhost:7233")]
    temporal_url: String,

    /// Temporal namespace.
    #[arg(long, env = "TEMPORAL_NAMESPACE", default_value = "default")]
    namespace: String,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Submit a new job, print the workflow id.
    Submit(SubmitArgs),
    /// Print the current stage and progress of a workflow.
    Status { workflow_id: String },
    /// Send a cancel signal to a workflow.
    Cancel(CancelArgs),
    /// Wait for a workflow to finish and print the result as JSON.
    Wait { workflow_id: String },
}

#[derive(Debug, Args)]
struct SubmitArgs {
    #[arg(long)]
    name: String,
    #[arg(long)]
    command: String,
    #[arg(long, default_value_t = 60)]
    timeout: u32,
}

#[derive(Debug, Args)]
struct CancelArgs {
    workflow_id: String,
    #[arg(long, default_value = "manual")]
    reason: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,jobctl=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let svc = build_client(&cli.temporal_url, &cli.namespace).await?;

    match cli.cmd {
        Cmd::Submit(args) => {
            let handle = svc
                .run_job(
                    JobInput {
                        name: args.name,
                        command: args.command,
                        timeout_seconds: args.timeout,
                    },
                    RunJobStartOptions::default(),
                )
                .await?;
            println!("workflow_id={}", handle.workflow_id());
        }
        Cmd::Status { workflow_id } => {
            let handle = svc.run_job_handle(workflow_id);
            let s = handle.get_status(GetStatusInput {}).await?;
            println!("stage={} progress={}%", s.stage, s.progress_pct);
        }
        Cmd::Cancel(args) => {
            let handle = svc.run_job_handle(args.workflow_id.clone());
            handle
                .cancel_job(CancelJobInput {
                    reason: args.reason,
                })
                .await?;
            println!("cancelled: {}", args.workflow_id);
        }
        Cmd::Wait { workflow_id } => {
            let handle = svc.run_job_handle(workflow_id);
            let out = handle.result().await?;
            let json = serde_json::json!({
                "exit_code": out.exit_code,
                "stdout": out.stdout,
                "stderr": out.stderr,
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
    }
    Ok(())
}

async fn build_client(url: &str, namespace: &str) -> anyhow::Result<JobServiceClient> {
    let client = temporal_runtime::connect(url, namespace).await?;
    Ok(JobServiceClient::new(client))
}
