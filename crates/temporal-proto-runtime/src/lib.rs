//! Wire-format runtime for [`protoc-gen-rust-temporal`]-generated clients.
//!
//! Generated code pairs each prost message type with a
//! [`TemporalProtoMessage`] impl carrying the fully-qualified proto name, and
//! routes payloads through [`TypedProtoMessage<T>`] so Temporal's data
//! converter encodes them as the `(encoding, messageType, data)` triple
//! documented in `WIRE-FORMAT.md`.
//!
//! # Feature flags
//!
//! * **default**: trait + wrapper + `ENCODING` constant only, no SDK dep.
//! * **`sdk`**: pulls in `temporalio-common = "0.4"` and ships
//!   `TemporalSerializable` + `TemporalDeserializable` impls for
//!   [`TypedProtoMessage<T>`]. Consumer crates should enable this so the
//!   Rust orphan rule doesn't force them to redefine the wrapper locally
//!   just to implement those traits against `temporalio-common`.
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

#[cfg(feature = "sdk")]
mod sdk_impls {
    use super::{ENCODING, TemporalProtoMessage, TypedProtoMessage};
    use std::collections::HashMap;
    use temporalio_common::data_converters::{
        PayloadConversionError, SerializationContext, TemporalDeserializable, TemporalSerializable,
    };
    use temporalio_common::protos::temporal::api::common::v1::Payload;

    impl<T: TemporalProtoMessage> TemporalSerializable for TypedProtoMessage<T> {
        fn to_payload(
            &self,
            _: &SerializationContext<'_>,
        ) -> Result<Payload, PayloadConversionError> {
            let mut metadata = HashMap::new();
            metadata.insert("encoding".to_string(), ENCODING.as_bytes().to_vec());
            metadata.insert(
                "messageType".to_string(),
                T::MESSAGE_TYPE.as_bytes().to_vec(),
            );
            Ok(Payload {
                metadata,
                data: prost::Message::encode_to_vec(&self.0),
                external_payloads: vec![],
            })
        }
    }

    impl<T: TemporalProtoMessage> TemporalDeserializable for TypedProtoMessage<T> {
        fn from_payload(
            _: &SerializationContext<'_>,
            p: Payload,
        ) -> Result<Self, PayloadConversionError> {
            let encoding = p.metadata.get("encoding").map(Vec::as_slice);
            if encoding != Some(ENCODING.as_bytes()) {
                return Err(PayloadConversionError::WrongEncoding);
            }
            let msg_type = p.metadata.get("messageType").map(Vec::as_slice);
            if msg_type != Some(T::MESSAGE_TYPE.as_bytes()) {
                return Err(PayloadConversionError::WrongEncoding);
            }
            T::decode(p.data.as_slice())
                .map(TypedProtoMessage)
                .map_err(|e| PayloadConversionError::EncodingError(Box::new(e)))
        }
    }
}

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
