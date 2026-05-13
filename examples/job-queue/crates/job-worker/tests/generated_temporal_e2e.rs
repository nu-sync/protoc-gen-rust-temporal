use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use jobs_proto::jobs::v1::{CancelJobInput, GetStatusInput, JobInput, JobStatusOutput};
use jobs_proto::jobs_v1_job_service_temporal::{
    JobServiceClient, RunJobHandle, RunJobStartOptions,
};
use jobs_proto::temporal_runtime;
use temporalio_client::{Client, ClientOptions, Connection, ConnectionOptions};
use temporalio_sdk::Worker;
use temporalio_sdk_core::ephemeral_server::{TemporalDevServerConfig, default_cached_download};
use temporalio_sdk_core::{CoreRuntime, RuntimeOptions, Url};
use tokio::task::LocalSet;
use tokio::time::{Instant, sleep, timeout};
use uuid::Uuid;

#[tokio::test(flavor = "current_thread")]
#[ignore = "downloads and runs a Temporal dev server; CI runs it explicitly"]
async fn generated_client_and_worker_execute_against_temporal_dev_server() -> Result<()> {
    LocalSet::new().run_until(run_smoke_test()).await
}

async fn run_smoke_test() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,job_worker=info".into()),
        )
        .try_init();

    let server_config = TemporalDevServerConfig::builder()
        .exe(default_cached_download())
        .ui(false)
        .build();
    let mut server = server_config
        .start_server_with_output(Stdio::null(), Stdio::null())
        .await
        .context("start Temporal dev server")?;
    let temporal_url = format!("http://{}", server.target);

    let sdk_client = connect_sdk_client(&temporal_url).await?;
    let runtime = CoreRuntime::new_assume_tokio(RuntimeOptions::default())
        .context("create Temporal core runtime")?;
    let mut worker = Worker::new(
        &runtime,
        sdk_client.clone(),
        job_worker::build_options("jobs"),
    )
    .map_err(|e| anyhow!(e.to_string()))
    .context("create job worker")?;
    job_worker::register(&mut worker);
    let shutdown_worker = worker.shutdown_handle();
    let worker_task = tokio::task::spawn_local(async move { worker.run().await });

    let smoke_result = run_generated_client_smoke(&temporal_url).await;

    shutdown_worker();
    let worker_result = timeout(Duration::from_secs(10), worker_task)
        .await
        .context("worker did not shut down within 10 seconds")?;
    worker_result.context("worker task panicked")??;
    server
        .shutdown()
        .await
        .context("shut down Temporal dev server")?;

    smoke_result
}

async fn run_generated_client_smoke(temporal_url: &str) -> Result<()> {
    let sdk_client = connect_sdk_client(temporal_url).await?;
    let client = JobServiceClient::new(temporal_runtime::TemporalClient::from_client(sdk_client));

    let job_name = format!("ci-e2e-{}", Uuid::new_v4());
    let workflow_id = format!("job-{job_name}");
    let handle = client
        .run_job(
            JobInput {
                name: job_name,
                command: "cargo test -p job-worker".to_string(),
                timeout_seconds: 60,
            },
            RunJobStartOptions {
                workflow_id: Some(workflow_id),
                ..RunJobStartOptions::default()
            },
        )
        .await
        .context("start workflow through generated client")?;

    let status = wait_for_queryable_status(&handle).await?;
    ensure!(
        matches!(
            status.stage.as_str(),
            "pending" | "preparing" | "executing" | "collecting"
        ),
        "unexpected initial workflow stage: {}",
        status.stage
    );

    handle
        .cancel_job(CancelJobInput {
            reason: "generated-client-e2e".to_string(),
        })
        .await
        .context("send cancel signal through generated handle")?;

    let output = timeout(Duration::from_secs(45), handle.result())
        .await
        .context("workflow did not complete within 45 seconds")?
        .context("workflow completed with Temporal error")?;
    ensure!(
        output.exit_code == 130,
        "expected cancellation exit code 130"
    );
    ensure!(
        output.stderr.contains("generated-client-e2e"),
        "cancel reason missing from workflow output: {}",
        output.stderr
    );

    Ok(())
}

async fn wait_for_queryable_status(handle: &RunJobHandle) -> Result<JobStatusOutput> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last_error = None;

    while Instant::now() < deadline {
        match handle.get_status(GetStatusInput {}).await {
            Ok(status) => return Ok(status),
            Err(err) => {
                last_error = Some(err);
                sleep(Duration::from_millis(250)).await;
            }
        }
    }

    match last_error {
        Some(err) => Err(err).context("workflow did not become queryable within 15 seconds"),
        None => bail!("workflow did not become queryable within 15 seconds"),
    }
}

async fn connect_sdk_client(temporal_url: &str) -> Result<Client> {
    let connection = Connection::connect(
        ConnectionOptions::new(Url::parse(temporal_url).context("parse Temporal URL")?).build(),
    )
    .await
    .context("connect to Temporal frontend")?;
    Client::new(
        connection,
        ClientOptions::new("default".to_string()).build(),
    )
    .context("build Temporal SDK client")
}
