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

// Default build: stub temporal_runtime.rs with `todo!()` bodies — keeps the
// workspace SDK-free in CI. With `--features bridge`, swap to the real
// bridge crate; the plugin's generated emit calls `crate::temporal_runtime::*`
// either way, so this single re-export is the only knob.
#[cfg(not(feature = "bridge"))]
pub mod temporal_runtime;
#[cfg(feature = "bridge")]
pub use temporal_proto_runtime_bridge as temporal_runtime;

pub mod jobs {
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/jobs.v1.rs"));
    }
}

include!("gen/jobs/v1/jobs_temporal.rs");
