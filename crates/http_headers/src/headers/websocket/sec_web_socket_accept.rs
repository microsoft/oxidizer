// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use sha1::{Digest as _, Sha1};

use super::super::invalid_syntax;
use super::sec_web_socket_key::SecWebSocketKeyOwned;
use super::shared::{encode_fixed_base64, validate_canonical_base64};
use crate::{DecodeError, FieldName, FieldValue, FieldValueRef, SingleValueField};

const WEBSOCKET_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Defines the `Sec-WebSocket-Accept` header.
///
/// # Specification
///
/// Defined by [RFC 6455 section 11.3.3](https://www.rfc-editor.org/rfc/rfc6455#section-11.3.3).
///
/// # Examples
///
/// ```rust
/// use http_headers::headers::SecWebSocketAccept;
/// use http_headers::{FieldValueRef, SingleValueField};
///
/// let view =
///     SecWebSocketAccept::decode_view(FieldValueRef::new(b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo="))?;
/// assert_eq!(view.encoded(), b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
#[derive(Debug)]
pub struct SecWebSocketAccept {
    _private: (),
}

/// Owned value for the `Sec-WebSocket-Accept` header.
///
/// # Specification
///
/// Defined by [RFC 6455 section 11.3.3].
///
/// # Examples
///
/// ```rust
/// use http_headers::headers::SecWebSocketAcceptOwned;
///
/// let value = SecWebSocketAcceptOwned::try_from("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")?;
/// assert_eq!(value.encoded(), b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=` is the response
/// corresponding to the RFC example key.
///
/// [RFC 6455 section 11.3.3]: https://www.rfc-editor.org/rfc/rfc6455#section-11.3.3
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SecWebSocketAcceptOwned {
    value: FieldValue,
}

/// Borrowed value for the `Sec-WebSocket-Accept` header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{SecWebSocketAccept, SecWebSocketAcceptView};
/// use http_headers::{FieldValueRef, SingleValueField};
///
/// let view: SecWebSocketAcceptView<'_> =
///     SecWebSocketAccept::decode_view(FieldValueRef::new(b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo="))?;
/// assert_eq!(view.encoded(), b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct SecWebSocketAcceptView<'a> {
    value: FieldValueRef<'a>,
}

impl SecWebSocketAcceptOwned {
    /// Encodes a 20-byte SHA-1 handshake digest.
    ///
    /// The digest is the binary result of hashing the client key's wire bytes
    /// followed by the WebSocket GUID.
    ///
    /// # Errors
    ///
    /// The fixed-size digest always has a representable canonical encoding.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketAcceptOwned;
    ///
    /// let value = SecWebSocketAcceptOwned::from_digest([0; 20])?;
    /// assert_eq!(value.encoded(), b"AAAAAAAAAAAAAAAAAAAAAAAAAAA=");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[expect(
        clippy::unnecessary_wraps,
        reason = "WebSocket value constructors consistently report validation through DecodeError"
    )]
    pub fn from_digest(digest: [u8; 20]) -> Result<Self, DecodeError> {
        Ok(Self {
            value: encode_fixed_base64(&digest),
        })
    }

    /// Computes the server handshake value for a validated client key.
    ///
    /// # Errors
    ///
    /// Returns an error if the resulting field value cannot be represented.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketAcceptOwned;
    ///
    /// let key = "dGhlIHNhbXBsZSBub25jZQ==".try_into()?;
    /// let value = SecWebSocketAcceptOwned::from_key(&key)?;
    /// assert_eq!(value.encoded(), b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn from_key(key: &SecWebSocketKeyOwned) -> Result<Self, DecodeError> {
        let mut digest = Sha1::new();
        digest.update(key.encoded());
        digest.update(WEBSOCKET_GUID);
        Self::from_digest(digest.finalize().into())
    }

    /// Returns the canonical base64 bytes.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketAcceptOwned;
    ///
    /// let value = SecWebSocketAcceptOwned::try_from("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")?;
    /// assert_eq!(value.encoded(), b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn encoded(&self) -> &[u8] {
        self.value.as_bytes()
    }

    /// Returns the stored field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketAcceptOwned;
    ///
    /// let value = SecWebSocketAcceptOwned::try_from("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")?;
    /// assert_eq!(
    ///     value.as_field_value().as_bytes(),
    ///     b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_field_value(&self) -> &FieldValue {
        &self.value
    }

    /// Returns reusable wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketAcceptOwned;
    ///
    /// let value = SecWebSocketAcceptOwned::try_from("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")?;
    /// let field_value = value.into_field_value();
    /// assert_eq!(field_value.as_bytes(), b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::super::shared::impl_field_value_conversion!(SecWebSocketAcceptOwned, |value| value.value);

impl<'a> SecWebSocketAcceptView<'a> {
    /// Returns the canonical base64 bytes.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketAccept;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view =
    ///     SecWebSocketAccept::decode_view(FieldValueRef::new(b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo="))?;
    /// assert_eq!(view.encoded(), b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn encoded(self) -> &'a [u8] {
        self.value.as_bytes()
    }

    /// Returns the original field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketAccept;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view =
    ///     SecWebSocketAccept::decode_view(FieldValueRef::new(b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo="))?;
    /// assert_eq!(
    ///     view.as_field_value().as_bytes(),
    ///     b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_field_value(self) -> FieldValueRef<'a> {
        self.value
    }
}

impl SingleValueField for SecWebSocketAccept {
    type View<'a> = SecWebSocketAcceptView<'a>;
    type Owned = SecWebSocketAcceptOwned;

    fn name() -> &'static FieldName {
        &FieldName::SecWebSocketAccept
    }

    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        validate_canonical_base64(value.as_bytes(), 20, &FieldName::SecWebSocketAccept)?;
        Ok(SecWebSocketAcceptView { value })
    }

    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        SecWebSocketAcceptOwned::try_from(value)
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.value
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.value
    }
}

impl TryFrom<&str> for SecWebSocketAcceptOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = FieldValue::from_str(value).map_err(|_invalid| invalid_syntax(&FieldName::SecWebSocketAccept))?;
        Self::try_from(value)
    }
}

impl TryFrom<String> for SecWebSocketAcceptOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = FieldValue::try_from(value).map_err(|_invalid| invalid_syntax(&FieldName::SecWebSocketAccept))?;
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for SecWebSocketAcceptOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        validate_canonical_base64(value.as_bytes(), 20, &FieldName::SecWebSocketAccept)?;
        Ok(Self { value })
    }
}

impl TryFrom<&SecWebSocketKeyOwned> for SecWebSocketAcceptOwned {
    type Error = DecodeError;

    fn try_from(key: &SecWebSocketKeyOwned) -> Result<Self, Self::Error> {
        Self::from_key(key)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{SecWebSocketAccept, SecWebSocketAcceptOwned};
    use crate::sink::{EncodedValues, FieldSink};
    use crate::{DecodeErrorKind, Field, FieldValue, FieldValueRef, TestSink};

    #[test]
    fn digest_key_and_accessors_produce_canonical_handshake_value() {
        let zero = SecWebSocketAcceptOwned::from_digest([0; 20]).expect("digest encodes as base64");
        assert_eq!(zero.encoded(), b"AAAAAAAAAAAAAAAAAAAAAAAAAAA=");
        assert_eq!(zero.as_field_value().as_bytes(), zero.encoded());
        assert_eq!(zero.clone().into_field_value().as_bytes(), zero.encoded());

        let key = super::SecWebSocketKeyOwned::try_from("dGhlIHNhbXBsZSBub25jZQ==").expect("RFC key");
        let accept = SecWebSocketAcceptOwned::from_key(&key).expect("key hashes");
        assert_eq!(accept.encoded(), b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        assert_eq!(
            <SecWebSocketAccept as crate::SingleValueField>::as_field_value(&accept).as_bytes(),
            accept.encoded()
        );
        assert_eq!(
            <SecWebSocketAccept as crate::SingleValueField>::into_field_value(accept.clone()).as_bytes(),
            accept.encoded()
        );
        assert_eq!(
            <SecWebSocketAccept as crate::SingleValueField>::decode_owned(FieldValue::from_static("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),)
                .expect("direct owned decode"),
            accept
        );
        assert_eq!(
            <SecWebSocketAccept as crate::SingleValueField>::decode_view(FieldValueRef::new(b"s3pPLMBiTxaQ9kYGzzhZRbK+xO!=",))
                .expect_err("invalid borrowed digest")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(SecWebSocketAcceptOwned::try_from(&key).expect("TryFrom hashes"), accept);
    }

    #[test]
    fn conversions_and_header_round_trip_validate_exact_base64() {
        const WIRE: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";
        for value in [
            SecWebSocketAcceptOwned::try_from(WIRE),
            SecWebSocketAcceptOwned::try_from(String::from(WIRE)),
            SecWebSocketAcceptOwned::try_from(FieldValue::from_static(WIRE)),
        ] {
            assert_eq!(value.expect("canonical digest").encoded(), WIRE.as_bytes());
        }

        let mut table = TestSink::new();
        assert!(SecWebSocketAccept::view(&table).expect("absent header succeeds").is_none());
        SecWebSocketAccept::insert(&mut table, SecWebSocketAcceptOwned::try_from(WIRE).expect("canonical digest")).expect("header inserts");
        let view = SecWebSocketAccept::view(&table).expect("view decodes").expect("header is present");
        assert_eq!(view.encoded(), WIRE.as_bytes());
        assert_eq!(view.as_field_value().as_bytes(), WIRE.as_bytes());
        assert_eq!(
            SecWebSocketAccept::owned(&table)
                .expect("owned decode succeeds")
                .expect("header is present")
                .encoded(),
            WIRE.as_bytes()
        );

        for raw in [
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo",
            "s3pPLMBiTxaQ9kYGzzhZRbK+xO!=",
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOp=",
        ] {
            assert_eq!(
                SecWebSocketAcceptOwned::try_from(raw).expect_err("noncanonical digest").kind(),
                DecodeErrorKind::InvalidSyntax
            );
        }
        assert_eq!(
            SecWebSocketAcceptOwned::try_from(String::from("digest\n"))
                .expect_err("invalid field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            SecWebSocketAcceptOwned::try_from("digest\n")
                .expect_err("invalid borrowed field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn singleton_decoder_rejects_repeated_accept_values() {
        let mut table = TestSink::new();
        table
            .set_values(
                SecWebSocketAccept::name(),
                EncodedValues::from_vec(vec![
                    FieldValue::from_static("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
                    FieldValue::from_static("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
                ]),
            )
            .expect("table accepts raw values");
        assert_eq!(
            SecWebSocketAccept::view(&table).expect_err("singleton rejects duplicates").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
    }
}
