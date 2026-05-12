//! Example consumer crate. Wired together so `cargo check --workspace`
//! catches regressions in the plugin's rendered output, against the
//! documented `temporal_runtime` facade.
//!
//! Three layers:
//! 1. `temporal_runtime` — consumer-supplied bridge (stubbed with
//!    `todo!()`); see `temporal_runtime.rs`.
//! 2. `jobs::v1` — prost types built by `build.rs`.
//! 3. The plugin's emitted module, included at the crate root.
//!
//! Public usage example (does not compile-check the example itself —
//! that's what the file structure above does — but reads well in docs):
//!
//! ```ignore
//! use job_queue_integration_example::jobs_v1_job_service_temporal::{
//!     JobServiceClient, RunJobStartOptions,
//! };
//! use job_queue_integration_example::jobs::v1::JobInput;
//!
//! # async fn demo(client: temporal_runtime::TemporalClient) -> anyhow::Result<()> {
//! let client = JobServiceClient::new(client);
//! let handle = client.run_job(
//!     JobInput { name: "demo".into(), ..Default::default() },
//!     RunJobStartOptions::default(),
//! ).await?;
//! let status = handle.get_status().await?;
//! handle.cancel_job(/* CancelJobInput { .. } */).await?;
//! let result = handle.result().await?;
//! # Ok(()) }
//! ```

pub mod temporal_runtime;

pub mod jobs {
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/jobs.v1.rs"));
    }
}

include!("gen/jobs/v1/jobs_temporal.rs");
