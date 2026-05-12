use std::collections::HashSet;
use std::io::{self, Read, Write};

use anyhow::{Context, Result};
use prost::Message;
use prost_reflect::DescriptorPool;
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse};

const VERSION: &str = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"),);

fn main() -> Result<()> {
    // protoc invokes the plugin without args. Outside of protoc, support
    // `--version` / `-V` and `--help` / `-h` so installations are
    // diagnosable from a shell. Everything else falls through to the
    // stdin/stdout protoc contract.
    if let Some(flag) = std::env::args().nth(1) {
        match flag.as_str() {
            "--version" | "-V" => {
                println!("{VERSION}");
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "{VERSION}\n\nUsage: invoked by `protoc` via stdin/stdout.\n\
                     See https://github.com/nu-sync/protoc-gen-rust-temporal for buf.gen.yaml examples."
                );
                return Ok(());
            }
            _ => {}
        }
    }

    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input).context("read stdin")?;

    let response = match build_response(&input) {
        Ok(files) => CodeGeneratorResponse {
            file: files,
            error: None,
            supported_features: Some(
                prost_types::compiler::code_generator_response::Feature::Proto3Optional as u64,
            ),
        },
        Err(e) => CodeGeneratorResponse {
            error: Some(format!("{e:#}")),
            ..Default::default()
        },
    };

    let mut buf = Vec::new();
    response
        .encode(&mut buf)
        .context("encode CodeGeneratorResponse")?;
    io::stdout().write_all(&buf).context("write stdout")?;
    Ok(())
}

fn build_response(raw: &[u8]) -> Result<Vec<prost_types::compiler::code_generator_response::File>> {
    // Decode with prost-types just to get file_to_generate; extensions on
    // MethodOptions are dropped here, but we don't read them from this form.
    let req = CodeGeneratorRequest::decode(raw).context("decode CodeGeneratorRequest")?;
    let parameter = req.parameter().to_string();
    let files_to_generate: HashSet<String> = req.file_to_generate.into_iter().collect();
    let options = protoc_gen_rust_temporal::options::parse_options(&parameter)
        .context("parse plugin options")?;

    // Re-extract raw bytes of each FileDescriptorProto directly from the
    // original CodeGeneratorRequest wire payload. prost-types decode loses
    // extension data on MethodOptions; prost-reflect via
    // decode_file_descriptor_set preserves them as unknown-field bytes.
    let proto_file_blobs = extract_proto_file_blobs(raw)?;
    let mut fds_bytes = Vec::new();
    for blob in &proto_file_blobs {
        // FileDescriptorSet.file = 1, wire type 2 (length-delimited).
        encode_tagged(&mut fds_bytes, 1, blob);
    }

    let mut pool = DescriptorPool::new();
    pool.decode_file_descriptor_set(&*fds_bytes)
        .context("decode_file_descriptor_set (extensions preserved)")?;

    // Surface the target file set in any error coming out of the pipeline.
    // buf's per-target invocation pattern (buf v2 sends one
    // CodeGeneratorRequest per target proto in a module) means a single
    // `buf generate` may call the plugin many times, and stderr is
    // interleaved — without the file name in the message, you can't tell
    // which invocation failed without re-running with --debug.
    protoc_gen_rust_temporal::run_with_pool(&pool, &files_to_generate, &options).with_context(
        || {
            let mut targets: Vec<&str> = files_to_generate.iter().map(String::as_str).collect();
            targets.sort();
            format!("generating from [{}]", targets.join(", "))
        },
    )
}

/// Walk the `CodeGeneratorRequest` wire bytes and pull out each
/// `proto_file = 15` length-delimited blob without decoding it. This
/// preserves all extension data inside each `FileDescriptorProto`.
fn extract_proto_file_blobs(mut raw: &[u8]) -> Result<Vec<Vec<u8>>> {
    use prost::bytes::Buf;
    use prost::encoding::{WireType, decode_key, decode_varint};

    let mut out = Vec::new();
    while raw.has_remaining() {
        let (tag, wire_type) = decode_key(&mut raw).context("decode key")?;
        match (tag, wire_type) {
            (15, WireType::LengthDelimited) => {
                let len = decode_varint(&mut raw).context("decode proto_file len")? as usize;
                if raw.remaining() < len {
                    anyhow::bail!("truncated proto_file blob");
                }
                out.push(raw[..len].to_vec());
                raw = &raw[len..];
            }
            (_, WireType::Varint) => {
                let _ = decode_varint(&mut raw).context("skip varint")?;
            }
            (_, WireType::SixtyFourBit) => {
                if raw.remaining() < 8 {
                    anyhow::bail!("truncated 64-bit");
                }
                raw = &raw[8..];
            }
            (_, WireType::LengthDelimited) => {
                let len = decode_varint(&mut raw).context("skip ld len")? as usize;
                if raw.remaining() < len {
                    anyhow::bail!("truncated ld");
                }
                raw = &raw[len..];
            }
            (_, WireType::ThirtyTwoBit) => {
                if raw.remaining() < 4 {
                    anyhow::bail!("truncated 32-bit");
                }
                raw = &raw[4..];
            }
            (_, WireType::StartGroup | WireType::EndGroup) => {
                anyhow::bail!("unexpected group wire type");
            }
        }
    }
    Ok(out)
}

fn encode_tagged(out: &mut Vec<u8>, field: u32, payload: &[u8]) {
    use prost::encoding::{WireType, encode_key, encode_varint};
    encode_key(field, WireType::LengthDelimited, out);
    encode_varint(payload.len() as u64, out);
    out.extend_from_slice(payload);
}
