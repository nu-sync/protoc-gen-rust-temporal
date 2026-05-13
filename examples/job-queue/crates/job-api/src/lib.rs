//! Routes split out so they can be exercised in unit tests without binding a
//! socket.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use jobs_proto::jobs::v1::{CancelJobInput, GetStatusInput, JobInput};
use jobs_proto::jobs_v1_job_service_temporal::{JobServiceClient, RunJobStartOptions};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct AppState {
    pub client: Arc<JobServiceClient>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/jobs", post(create_job))
        .route("/jobs/{id}", get(get_status).delete(cancel_job))
        .route("/jobs/{id}/result", get(get_result))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

#[derive(Debug, Deserialize)]
pub struct CreateJobRequest {
    pub name: String,
    pub command: String,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
}

fn default_timeout() -> u32 {
    60
}

#[derive(Debug, Serialize)]
pub struct CreateJobResponse {
    pub workflow_id: String,
}

async fn create_job(
    State(state): State<AppState>,
    Json(req): Json<CreateJobRequest>,
) -> Result<(StatusCode, Json<CreateJobResponse>), AppError> {
    let handle = state
        .client
        .run_job(
            JobInput {
                name: req.name,
                command: req.command,
                timeout_seconds: req.timeout_seconds,
            },
            RunJobStartOptions::default(),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(CreateJobResponse {
            workflow_id: handle.workflow_id().to_string(),
        }),
    ))
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub stage: String,
    pub progress_pct: u32,
}

async fn get_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<StatusResponse>, AppError> {
    let handle = state.client.run_job_handle(id);
    let status = handle.get_status(GetStatusInput {}).await?;
    Ok(Json(StatusResponse {
        stage: status.stage,
        progress_pct: status.progress_pct,
    }))
}

#[derive(Debug, Deserialize)]
pub struct CancelParams {
    #[serde(default)]
    pub reason: Option<String>,
}

async fn cancel_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<CancelParams>,
) -> Result<StatusCode, AppError> {
    let handle = state.client.run_job_handle(id);
    handle
        .cancel_job(CancelJobInput {
            reason: params.reason.unwrap_or_default(),
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
pub struct ResultResponse {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

async fn get_result(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ResultResponse>, AppError> {
    let handle = state.client.run_job_handle(id);
    let r = handle.result().await?;
    Ok(Json(ResultResponse {
        exit_code: r.exit_code,
        stdout: r.stdout,
        stderr: r.stderr,
    }))
}

// ── error mapping ───────────────────────────────────────────────────────

pub struct AppError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let body = serde_json::json!({ "error": format!("{:#}", self.0) });
        (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_job_request_defaults_timeout() {
        let r: CreateJobRequest = serde_json::from_str(r#"{"name":"a","command":"true"}"#).unwrap();
        assert_eq!(r.timeout_seconds, 60);
    }
}
