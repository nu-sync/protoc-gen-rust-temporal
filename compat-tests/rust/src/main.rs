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
    use super::jobs_v1::{JobInput, JobOutput};
    use serde::Deserialize;

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
}
