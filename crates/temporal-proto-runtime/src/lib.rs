//! Wire-format runtime for [`protoc-gen-rust-temporal`]-generated clients.
//!
//! Generated code pairs each prost message type with a
//! [`TemporalProtoMessage`] impl carrying the fully-qualified proto name, and
//! routes payloads through [`TypedProtoMessage<T>`] so Temporal's data
//! converter encodes them as the `(encoding, messageType, data)` triple
//! documented in `WIRE-FORMAT.md`.
//!
//! Phase 0 ships only the trait + wrapper + encoding constant. The
//! `TemporalSerializable` / `TemporalDeserializable` impls against the
//! Temporal Rust SDK land in Phase 2, gated behind a `sdk` cargo feature —
//! the SDK pulls in `temporalio-sdk-core` which requires Rust ≥1.88, while
//! this crate's MSRV is 1.85 so the trait + wrapper remain usable in
//! lower-MSRV consumers (e.g. for offline encoding tests).
//!
//! [`protoc-gen-rust-temporal`]: https://github.com/nu-sync/protoc-gen-rust-temporal

/// Marker trait implemented for every prost message that participates in a
/// Temporal payload. The generated code emits one impl per workflow / signal
/// / query / update input or output type.
pub trait TemporalProtoMessage: prost::Message + Default + 'static {
    /// Fully-qualified proto name, e.g. `"jobs.v1.JobInput"`. Written into
    /// `metadata.messageType` on the wire.
    const MESSAGE_TYPE: &'static str;
}

/// Newtype wrapper that pairs a proto value with its message-type metadata.
/// Workflows, activities, and the generated client take/return this wrapper
/// at the wire boundary so the SDK's `TemporalSerializable` /
/// `TemporalDeserializable` dispatch picks up the `binary/protobuf`
/// encoding rather than the serde-JSON blanket impl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedProtoMessage<T: TemporalProtoMessage>(pub T);

impl<T: TemporalProtoMessage> TypedProtoMessage<T> {
    pub fn into_inner(self) -> T {
        self.0
    }

    pub fn as_inner(&self) -> &T {
        &self.0
    }
}

impl<T: TemporalProtoMessage> From<T> for TypedProtoMessage<T> {
    fn from(t: T) -> Self {
        TypedProtoMessage(t)
    }
}

/// The encoding string written into / required from `metadata.encoding`.
pub const ENCODING: &str = "binary/protobuf";

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Eq, prost::Message)]
    struct Sample {
        #[prost(string, tag = "1")]
        name: String,
    }

    impl TemporalProtoMessage for Sample {
        const MESSAGE_TYPE: &'static str = "test.v1.Sample";
    }

    #[test]
    fn wrapper_round_trips_via_prost_encode() {
        let original = TypedProtoMessage::from(Sample {
            name: "hello".into(),
        });
        let bytes = prost::Message::encode_to_vec(original.as_inner());
        let decoded: Sample = prost::Message::decode(bytes.as_slice()).unwrap();
        assert_eq!(decoded, original.into_inner());
    }

    #[test]
    fn encoding_constant_is_stable() {
        assert_eq!(ENCODING, "binary/protobuf");
    }

    #[test]
    fn message_type_constant_is_fully_qualified() {
        assert_eq!(Sample::MESSAGE_TYPE, "test.v1.Sample");
    }
}
