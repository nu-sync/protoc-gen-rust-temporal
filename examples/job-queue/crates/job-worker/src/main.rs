use anyhow::Result;
use temporalio_client::{Client, ClientOptions, Connection, ConnectionOptions};
use temporalio_sdk::Worker;
use temporalio_sdk_core::{CoreRuntime, RuntimeOptions, Url};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,job_worker=debug".into()),
        )
        .init();

    let temporal_url =
        std::env::var("TEMPORAL_URL").unwrap_or_else(|_| "http://localhost:7233".to_string());
    let namespace = std::env::var("TEMPORAL_NAMESPACE").unwrap_or_else(|_| "default".to_string());

    let runtime = CoreRuntime::new_assume_tokio(RuntimeOptions::default())?;
    let connection =
        Connection::connect(ConnectionOptions::new(Url::parse(&temporal_url)?).build()).await?;
    let client = Client::new(connection, ClientOptions::new(namespace).build())?;

    let options = job_worker::build_options("jobs");
    let mut w =
        Worker::new(&runtime, client, options).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    job_worker::register(&mut w);

    tracing::info!(task_queue = "jobs", "job-worker polling");
    w.run().await?;
    Ok(())
}
