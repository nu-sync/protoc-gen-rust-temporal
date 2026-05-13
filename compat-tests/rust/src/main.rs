//! Rust arm of the cross-language wire-format audit.
//!
//! Walks `../fixtures/*.input.json`, populates the typed prost message,
//! wraps it in `TypedProtoMessage<T>`, and writes the resulting Payload
//! triple to `<fixture>.rust.payload.json`. Compare against the Go arm's
//! output via plain `diff` — bytes-identical means the audit passes.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use base64::Engine;
use prost::Message;
use serde::{Deserialize, Serialize};

pub mod jobs_v1 {
    include!(concat!(env!("OUT_DIR"), "/jobs.v1.rs"));
}

use temporal_proto_runtime::{ENCODING, TemporalProtoMessage};

impl TemporalProtoMessage for jobs_v1::JobInput {
    const MESSAGE_TYPE: &'static str = "jobs.v1.JobInput";
}
impl TemporalProtoMessage for jobs_v1::JobOutput {
    const MESSAGE_TYPE: &'static str = "jobs.v1.JobOutput";
}
impl TemporalProtoMessage for jobs_v1::JobBatch {
    const MESSAGE_TYPE: &'static str = "jobs.v1.JobBatch";
}
impl TemporalProtoMessage for jobs_v1::JobList {
    const MESSAGE_TYPE: &'static str = "jobs.v1.JobList";
}
impl TemporalProtoMessage for jobs_v1::JobChoice {
    const MESSAGE_TYPE: &'static str = "jobs.v1.JobChoice";
}
impl TemporalProtoMessage for jobs_v1::JobEnum {
    const MESSAGE_TYPE: &'static str = "jobs.v1.JobEnum";
}
impl TemporalProtoMessage for jobs_v1::JobMap {
    const MESSAGE_TYPE: &'static str = "jobs.v1.JobMap";
}

#[derive(Deserialize)]
struct Fixture {
    message_type: String,
    fields: serde_json::Value,
}

#[derive(Serialize)]
struct WirePayload<'a> {
    metadata: std::collections::BTreeMap<&'a str, String>,
    data: String,
}

fn main() -> Result<()> {
    let arg = std::env::args().nth(1).unwrap_or_default();
    if arg != "generate" {
        bail!("usage: compat-tests-rust generate");
    }

    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures");
    for entry in
        fs::read_dir(&fixtures_dir).with_context(|| format!("read {}", fixtures_dir.display()))?
    {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        if !name.ends_with(".input.json") {
            continue;
        }
        let out_path = path.with_file_name(name.replace(".input.json", ".rust.payload.json"));
        process_fixture(&path, &out_path)?;
        println!("wrote {}", out_path.display());
    }
    Ok(())
}

fn process_fixture(in_path: &PathBuf, out_path: &PathBuf) -> Result<()> {
    let raw = fs::read_to_string(in_path)?;
    let fx: Fixture = serde_json::from_str(&raw)?;
    let data = encode_fixture(&fx)?;
    let payload = WirePayload {
        metadata: [
            ("encoding", ENCODING.to_string()),
            ("messageType", fx.message_type.clone()),
        ]
        .into_iter()
        .collect(),
        data: base64::engine::general_purpose::STANDARD.encode(&data),
    };
    let mut out = serde_json::to_string_pretty(&payload)?;
    out.push('\n');
    fs::write(out_path, out)?;
    Ok(())
}

fn encode_fixture(fx: &Fixture) -> Result<Vec<u8>> {
    match fx.message_type.as_str() {
        "jobs.v1.JobInput" => {
            let msg: jobs_v1::JobInput = decode_fields(&fx.fields)?;
            Ok(msg.encode_to_vec())
        }
        "jobs.v1.JobOutput" => {
            let msg: jobs_v1::JobOutput = decode_fields(&fx.fields)?;
            Ok(msg.encode_to_vec())
        }
        "jobs.v1.JobBatch" => {
            let msg: jobs_v1::JobBatch = decode_fields(&fx.fields)?;
            Ok(msg.encode_to_vec())
        }
        "jobs.v1.JobList" => {
            let msg: jobs_v1::JobList = decode_fields(&fx.fields)?;
            Ok(msg.encode_to_vec())
        }
        "jobs.v1.JobChoice" => {
            let msg: jobs_v1::JobChoice = decode_fields(&fx.fields)?;
            Ok(msg.encode_to_vec())
        }
        "jobs.v1.JobEnum" => {
            let msg: jobs_v1::JobEnum = decode_fields(&fx.fields)?;
            Ok(msg.encode_to_vec())
        }
        "jobs.v1.JobMap" => {
            let msg: jobs_v1::JobMap = decode_fields(&fx.fields)?;
            Ok(msg.encode_to_vec())
        }
        "google.protobuf.Empty" => Ok(Vec::new()),
        other => bail!(
            "unknown fixture message_type {other:?}. Add an arm in compat-tests/rust/src/main.rs."
        ),
    }
}

fn decode_fields<T: for<'de> Deserialize<'de>>(value: &serde_json::Value) -> Result<T> {
    serde_json::from_value(value.clone()).context("decode fields into prost message")
}

// jobs_v1 types need serde to round-trip the JSON fixtures. The standard
// prost-build invocation doesn't add serde derives, so we re-implement
// Deserialize for the two fixture types by hand. They're small.
mod _serde_impls {
    use super::jobs_v1::{
        JobBatch, JobChoice, JobEnum, JobInput, JobList, JobMap, JobOutput, job_choice,
    };
    use std::collections::HashMap;

    use serde::Deserialize;
    use serde::de::Error as _;

    #[derive(Deserialize)]
    struct JobInputJson {
        #[serde(default)]
        name: String,
    }
    #[derive(Deserialize)]
    struct JobOutputJson {
        #[serde(default)]
        id: String,
    }
    #[derive(Deserialize)]
    struct JobBatchJson {
        #[serde(default)]
        batch_id: String,
        #[serde(default)]
        input: Option<JobInput>,
        #[serde(default)]
        priority: i32,
    }
    #[derive(Deserialize)]
    struct JobListJson {
        #[serde(default)]
        items: Vec<JobInput>,
    }
    #[derive(Deserialize)]
    struct JobMapJson {
        #[serde(default)]
        inputs: HashMap<String, JobInput>,
    }

    impl<'de> serde::Deserialize<'de> for JobInput {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let j = JobInputJson::deserialize(d)?;
            Ok(JobInput { name: j.name })
        }
    }
    impl<'de> serde::Deserialize<'de> for JobOutput {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let j = JobOutputJson::deserialize(d)?;
            Ok(JobOutput { id: j.id })
        }
    }
    impl<'de> serde::Deserialize<'de> for JobBatch {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let j = JobBatchJson::deserialize(d)?;
            Ok(JobBatch {
                batch_id: j.batch_id,
                input: j.input,
                priority: j.priority,
            })
        }
    }
    impl<'de> serde::Deserialize<'de> for JobList {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let j = JobListJson::deserialize(d)?;
            Ok(JobList { items: j.items })
        }
    }
    impl<'de> serde::Deserialize<'de> for JobChoice {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let v = serde_json::Value::deserialize(d)?;
            let name = v.get("name");
            let input = v.get("input");
            let target = match (name, input) {
                (Some(name), None) => {
                    let name = name
                        .as_str()
                        .ok_or_else(|| D::Error::custom("JobChoice.name must be a string"))?;
                    Some(job_choice::Target::Name(name.to_string()))
                }
                (None, Some(input)) => {
                    let input: JobInput =
                        serde_json::from_value(input.clone()).map_err(D::Error::custom)?;
                    Some(job_choice::Target::Input(input))
                }
                (None, None) => None,
                (Some(_), Some(_)) => {
                    return Err(D::Error::custom(
                        "JobChoice fixture must set exactly one oneof arm",
                    ));
                }
            };
            Ok(JobChoice { target })
        }
    }
    impl<'de> serde::Deserialize<'de> for JobEnum {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let v = serde_json::Value::deserialize(d)?;
            let kind = match v.get("kind") {
                None | Some(serde_json::Value::Null) => 0,
                Some(serde_json::Value::String(s)) => match s.as_str() {
                    "JOB_KIND_UNSPECIFIED" => 0,
                    "JOB_KIND_BATCH" => 1,
                    other => {
                        return Err(D::Error::custom(format!(
                            "unknown JobKind fixture value {other:?}"
                        )));
                    }
                },
                Some(serde_json::Value::Number(n)) => n
                    .as_i64()
                    .ok_or_else(|| D::Error::custom("JobKind number must fit in i64"))?
                    as i32,
                Some(other) => {
                    return Err(D::Error::custom(format!(
                        "JobEnum.kind must be a string or number, got {other}"
                    )));
                }
            };
            Ok(JobEnum { kind })
        }
    }
    impl<'de> serde::Deserialize<'de> for JobMap {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let j = JobMapJson::deserialize(d)?;
            Ok(JobMap { inputs: j.inputs })
        }
    }
}
