//! Example consumer crate root.
//!
//! After `buf generate` runs (see `../buf.gen.yaml`), the prost output
//! lives at `src/jobs/v1/mod.rs` and the plugin-generated Temporal client
//! lives at `src/gen/jobs/v1/jobs_temporal.rs`. The two files below are
//! placeholders for that output so the example compiles without a buf
//! invocation in CI:

pub mod temporal_runtime;

// pub mod jobs {
//     pub mod v1 {
//         include!("jobs/v1/mod.rs");           // protoc-gen-prost output
//     }
// }
//
// include!("gen/jobs/v1/jobs_temporal.rs");      // protoc-gen-rust-temporal output
//
// Once the includes above are wired up, the typed surface available to
// callers is:
//
//   use crate::jobs_v1_job_service_temporal::{
//       JobServiceClient, RunJobHandle, RunJobStartOptions,
//   };
//
//   let client = JobServiceClient::new(temporal_runtime_client);
//   let handle = client.run_job(JobInput { name: "demo".into(), ..Default::default() },
//                                RunJobStartOptions::default()).await?;
//   let status = handle.get_status().await?;
//   let _ = handle.cancel_job(CancelJobInput { reason: "user".into() }).await?;
//   let result = handle.result().await?;
