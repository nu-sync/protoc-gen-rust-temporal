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

/// Wire-format marker for `google.protobuf.Empty`. The plugin emits
/// `TypedProtoMessage<ProtoEmpty>` as the `Input` / `Output` associated
/// type on per-rpc `ActivityDefinition` / `WorkflowDefinition` impls
/// whenever the proto declares `google.protobuf.Empty` on either side.
///
/// `()` can't fill that role because the `TemporalSerializable` /
/// `TemporalDeserializable` blanket impls live on
/// `TypedProtoMessage<T: TemporalProtoMessage>` and require
/// `T: prost::Message + Default`. We define `ProtoEmpty` ourselves
/// (instead of leaning on a foreign crate's Empty) so the impls land
/// here without orphan-rule contortions.
#[derive(Clone, Copy, PartialEq, Eq, prost::Message)]
pub struct ProtoEmpty {}

impl TemporalProtoMessage for ProtoEmpty {
    const MESSAGE_TYPE: &'static str = "google.protobuf.Empty";
}

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

    // Regression guards for the wire-format decode contract from
    // `WIRE-FORMAT.md`: any payload that fails to carry exactly the
    // `(binary/protobuf, <expected-type>, …)` triple must be rejected.
    // These tests exercise the `sdk`-feature `TemporalDeserializable`
    // impl directly so a future refactor can't silently drop one of the
    // four required mismatch checks (missing/wrong encoding, missing/
    // wrong messageType).
    #[cfg(feature = "sdk")]
    mod sdk_decode {
        use super::Sample;
        use crate::{ENCODING, TemporalProtoMessage, TypedProtoMessage};
        use std::collections::HashMap;
        use temporalio_common::data_converters::{
            PayloadConversionError, PayloadConverter, SerializationContext,
            SerializationContextData, TemporalDeserializable,
        };
        use temporalio_common::protos::temporal::api::common::v1::Payload;

        // `from_payload` ignores the context, but we still need a real
        // value to pass — using the `None` data variant + `UseWrappers`
        // converter avoids dragging the serde converter graph into a
        // wire-format-only test.
        fn ctx() -> (SerializationContextData, PayloadConverter) {
            (
                SerializationContextData::None,
                PayloadConverter::UseWrappers,
            )
        }
        fn ctx_borrow<'a>(
            data: &'a SerializationContextData,
            conv: &'a PayloadConverter,
        ) -> SerializationContext<'a> {
            SerializationContext {
                data,
                converter: conv,
            }
        }

        fn payload(encoding: Option<&[u8]>, message_type: Option<&[u8]>, data: Vec<u8>) -> Payload {
            let mut metadata = HashMap::new();
            if let Some(e) = encoding {
                metadata.insert("encoding".to_string(), e.to_vec());
            }
            if let Some(m) = message_type {
                metadata.insert("messageType".to_string(), m.to_vec());
            }
            Payload {
                metadata,
                data,
                external_payloads: vec![],
            }
        }

        #[test]
        fn rejects_missing_encoding() {
            let p = payload(None, Some(Sample::MESSAGE_TYPE.as_bytes()), vec![]);
            let err = {
                let (d, c) = ctx();
                <TypedProtoMessage<Sample> as TemporalDeserializable>::from_payload(
                    &ctx_borrow(&d, &c),
                    p,
                )
            }
            .unwrap_err();
            assert!(matches!(err, PayloadConversionError::WrongEncoding));
        }

        #[test]
        fn rejects_wrong_encoding() {
            let p = payload(
                Some(b"json/protobuf"),
                Some(Sample::MESSAGE_TYPE.as_bytes()),
                vec![],
            );
            let err = {
                let (d, c) = ctx();
                <TypedProtoMessage<Sample> as TemporalDeserializable>::from_payload(
                    &ctx_borrow(&d, &c),
                    p,
                )
            }
            .unwrap_err();
            assert!(matches!(err, PayloadConversionError::WrongEncoding));
        }

        #[test]
        fn rejects_missing_message_type() {
            let p = payload(Some(ENCODING.as_bytes()), None, vec![]);
            let err = {
                let (d, c) = ctx();
                <TypedProtoMessage<Sample> as TemporalDeserializable>::from_payload(
                    &ctx_borrow(&d, &c),
                    p,
                )
            }
            .unwrap_err();
            assert!(matches!(err, PayloadConversionError::WrongEncoding));
        }

        #[test]
        fn rejects_wrong_message_type() {
            let p = payload(Some(ENCODING.as_bytes()), Some(b"some.other.Type"), vec![]);
            let err = {
                let (d, c) = ctx();
                <TypedProtoMessage<Sample> as TemporalDeserializable>::from_payload(
                    &ctx_borrow(&d, &c),
                    p,
                )
            }
            .unwrap_err();
            assert!(matches!(err, PayloadConversionError::WrongEncoding));
        }

        #[test]
        fn accepts_matching_triple() {
            let original = Sample { name: "hi".into() };
            let bytes = prost::Message::encode_to_vec(&original);
            let p = payload(
                Some(ENCODING.as_bytes()),
                Some(Sample::MESSAGE_TYPE.as_bytes()),
                bytes,
            );
            let decoded = {
                let (d, c) = ctx();
                <TypedProtoMessage<Sample> as TemporalDeserializable>::from_payload(
                    &ctx_borrow(&d, &c),
                    p,
                )
            }
            .expect("matching triple must decode");
            assert_eq!(decoded.into_inner(), original);
        }

        #[test]
        fn surfaces_prost_decode_errors_as_encoding_error() {
            let p = payload(
                Some(ENCODING.as_bytes()),
                Some(Sample::MESSAGE_TYPE.as_bytes()),
                // Truncated varint — invalid wire bytes for the `name`
                // string tag; prost::Message::decode will error.
                vec![0x0A, 0xFF],
            );
            let err = {
                let (d, c) = ctx();
                <TypedProtoMessage<Sample> as TemporalDeserializable>::from_payload(
                    &ctx_borrow(&d, &c),
                    p,
                )
            }
            .unwrap_err();
            assert!(
                matches!(err, PayloadConversionError::EncodingError(_)),
                "decode failure should surface as EncodingError, got: {err:?}"
            );
        }
    }
}
