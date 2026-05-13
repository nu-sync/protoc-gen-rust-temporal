use std::sync::Arc;

use job_api::{AppState, router};
use jobs_proto::jobs_v1_job_service_temporal::JobServiceClient;
use jobs_proto::temporal_runtime;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,job_api=debug".into()),
        )
        .init();

    let temporal_url =
        std::env::var("TEMPORAL_URL").unwrap_or_else(|_| "http://localhost:7233".to_string());
    let namespace = std::env::var("TEMPORAL_NAMESPACE").unwrap_or_else(|_| "default".to_string());
    let bind = std::env::var("BIND").unwrap_or_else(|_| "0.0.0.0:3030".to_string());

    let client = temporal_runtime::connect(&temporal_url, &namespace).await?;
    let service = Arc::new(JobServiceClient::new(client));
    let state = AppState { client: service };

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, %temporal_url, namespace = %namespace, "job-api listening");
    axum::serve(listener, router(state)).await?;
    Ok(())
}
