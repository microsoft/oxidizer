// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::super::invalid_syntax;
use super::shared::{encode_fixed_base64, validate_canonical_base64};
use crate::{DecodeError, FieldName, FieldValue, FieldValueRef, SingleValueField};

/// Defines the `Sec-WebSocket-Key` header.
///
/// # Specification
///
/// Defined by [RFC 6455 section 11.3.1](https://www.rfc-editor.org/rfc/rfc6455#section-11.3.1).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{SecWebSocketKey, SecWebSocketKeyOwned};
///
/// let mut map = HeaderMap::new();
/// SecWebSocketKey::insert(
///     &mut map,
///     SecWebSocketKeyOwned::try_from("dGhlIHNhbXBsZSBub25jZQ==")?,
/// )?;
/// assert!(SecWebSocketKey::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct SecWebSocketKey {
    _private: (),
}

/// Owned value for the `Sec-WebSocket-Key` header.
///
/// # Specification
///
/// Defined by [RFC 6455 section 11.3.1].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::SecWebSocketKeyOwned::try_from("dGhlIHNhbXBsZSBub25jZQ==")?;
/// assert_eq!(value.encoded(), b"dGhlIHNhbXBsZSBub25jZQ==");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==` is the RFC example of a
/// base64-encoded 16-byte nonce.
///
/// [RFC 6455 section 11.3.1]: https://www.rfc-editor.org/rfc/rfc6455#section-11.3.1
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SecWebSocketKeyOwned {
    value: FieldValue,
}

/// Borrowed value for the `Sec-WebSocket-Key` header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{SecWebSocketKey, SecWebSocketKeyView};
/// use http_headers::{FieldValueRef, SingleValueField};
///
/// let view: SecWebSocketKeyView<'_> =
///     SecWebSocketKey::decode_view(FieldValueRef::new(b"dGhlIHNhbXBsZSBub25jZQ=="))?;
/// assert_eq!(view.encoded(), b"dGhlIHNhbXBsZSBub25jZQ==");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct SecWebSocketKeyView<'a> {
    value: FieldValueRef<'a>,
}

impl SecWebSocketKeyOwned {
    /// Encodes a 16-byte client nonce.
    ///
    /// The nonce must be generated independently for every connection by a
    /// cryptographically secure random number generator. Predictable or reused
    /// nonces defeat the handshake's protection against intermediary cache
    /// poisoning; this function encodes the nonce but does not generate it.
    ///
    /// # Errors
    ///
    /// The fixed-size nonce always has a representable canonical encoding.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketKeyOwned;
    ///
    /// let value = SecWebSocketKeyOwned::from_nonce(*b"the sample nonce")?;
    /// assert_eq!(value.encoded(), b"dGhlIHNhbXBsZSBub25jZQ==");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[expect(
        clippy::unnecessary_wraps,
        reason = "WebSocket value constructors consistently report validation through DecodeError"
    )]
    pub fn from_nonce(nonce: [u8; 16]) -> Result<Self, DecodeError> {
        Ok(Self {
            value: encode_fixed_base64(&nonce),
        })
    }

    /// Returns the canonical base64 bytes.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketKeyOwned;
    ///
    /// let value = SecWebSocketKeyOwned::try_from("dGhlIHNhbXBsZSBub25jZQ==")?;
    /// assert_eq!(value.encoded(), b"dGhlIHNhbXBsZSBub25jZQ==");
    /// assert!(SecWebSocketKeyOwned::try_from("dGhlIHNhbXBsZSBub25jZQ=").is_err());
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
    /// use http_headers::headers::SecWebSocketKeyOwned;
    ///
    /// let value = SecWebSocketKeyOwned::try_from("dGhlIHNhbXBsZSBub25jZQ==")?;
    /// assert_eq!(
    ///     value.as_field_value().as_bytes(),
    ///     b"dGhlIHNhbXBsZSBub25jZQ=="
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
    /// use http_headers::headers::SecWebSocketKeyOwned;
    ///
    /// let value = SecWebSocketKeyOwned::try_from("dGhlIHNhbXBsZSBub25jZQ==")?;
    /// let field_value = value.into_field_value();
    /// assert_eq!(field_value.as_bytes(), b"dGhlIHNhbXBsZSBub25jZQ==");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::super::shared::impl_field_value_conversion!(SecWebSocketKeyOwned, |value| value.value);

impl<'a> SecWebSocketKeyView<'a> {
    /// Returns the canonical base64 bytes.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketKey;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = SecWebSocketKey::decode_view(FieldValueRef::new(b"dGhlIHNhbXBsZSBub25jZQ=="))?;
    /// assert_eq!(view.encoded(), b"dGhlIHNhbXBsZSBub25jZQ==");
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
    /// use http_headers::headers::SecWebSocketKey;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = SecWebSocketKey::decode_view(FieldValueRef::new(b"dGhlIHNhbXBsZSBub25jZQ=="))?;
    /// assert_eq!(
    ///     view.as_field_value().as_bytes(),
    ///     b"dGhlIHNhbXBsZSBub25jZQ=="
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_field_value(self) -> FieldValueRef<'a> {
        self.value
    }
}

impl SingleValueField for SecWebSocketKey {
    type View<'a> = SecWebSocketKeyView<'a>;
    type Owned = SecWebSocketKeyOwned;

    fn name() -> &'static FieldName {
        &FieldName::SecWebSocketKey
    }

    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        validate_canonical_base64(value.as_bytes(), 16, &FieldName::SecWebSocketKey)?;
        Ok(SecWebSocketKeyView { value })
    }

    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        SecWebSocketKeyOwned::try_from(value)
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.value
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.value
    }
}

super::super::shared::impl_string_conversions!(SecWebSocketKeyOwned, &FieldName::SecWebSocketKey, invalid_syntax, value);

impl TryFrom<FieldValue> for SecWebSocketKeyOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        validate_canonical_base64(value.as_bytes(), 16, &FieldName::SecWebSocketKey)?;
        Ok(Self { value })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{SecWebSocketKey, SecWebSocketKeyOwned};
    use crate::sink::{EncodedValues, FieldSink};
    use crate::{DecodeErrorKind, Field, FieldValue, FieldValueRef, TestSink};

    #[test]
    fn nonce_and_accessors_produce_canonical_base64() {
        let zero = SecWebSocketKeyOwned::from_nonce([0; 16]).expect("nonce encodes");
        assert_eq!(zero.encoded(), b"AAAAAAAAAAAAAAAAAAAAAA==");
        assert_eq!(zero.as_field_value().as_bytes(), zero.encoded());
        assert_eq!(zero.clone().into_field_value().as_bytes(), zero.encoded());
        assert_eq!(
            <SecWebSocketKey as crate::SingleValueField>::as_field_value(&zero).as_bytes(),
            zero.encoded()
        );
        assert_eq!(
            <SecWebSocketKey as crate::SingleValueField>::into_field_value(zero.clone()).as_bytes(),
            zero.encoded()
        );
        assert_eq!(
            <SecWebSocketKey as crate::SingleValueField>::decode_owned(FieldValue::from_static("AAAAAAAAAAAAAAAAAAAAAA==",))
                .expect("direct owned decode"),
            zero
        );
        assert_eq!(
            <SecWebSocketKey as crate::SingleValueField>::decode_view(FieldValueRef::new(b"AAAAAAAAAAAAAAAAAAAAA!==",))
                .expect_err("invalid borrowed nonce")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn conversions_and_header_round_trip_validate_exact_base64() {
        const WIRE: &str = "dGhlIHNhbXBsZSBub25jZQ==";
        for value in [
            SecWebSocketKeyOwned::try_from(WIRE),
            SecWebSocketKeyOwned::try_from(String::from(WIRE)),
            SecWebSocketKeyOwned::try_from(FieldValue::from_static(WIRE)),
        ] {
            assert_eq!(value.expect("canonical nonce").encoded(), WIRE.as_bytes());
        }

        let mut table = TestSink::new();
        assert!(SecWebSocketKey::view(&table).expect("absent header succeeds").is_none());
        SecWebSocketKey::insert(&mut table, SecWebSocketKeyOwned::try_from(WIRE).expect("canonical nonce")).expect("header inserts");
        let view = SecWebSocketKey::view(&table).expect("view decodes").expect("header is present");
        assert_eq!(view.encoded(), WIRE.as_bytes());
        assert_eq!(view.as_field_value().as_bytes(), WIRE.as_bytes());
        assert_eq!(
            SecWebSocketKey::owned(&table)
                .expect("owned decode succeeds")
                .expect("header is present")
                .encoded(),
            WIRE.as_bytes()
        );

        for raw in ["dGhlIHNhbXBsZSBub25jZQ=", "dGhlIHNhbXBsZSBub25jZ!==", "dGhlIHNhbXBsZSBub25jZR=="] {
            assert_eq!(
                SecWebSocketKeyOwned::try_from(raw).expect_err("noncanonical nonce").kind(),
                DecodeErrorKind::InvalidSyntax
            );
        }
        assert_eq!(
            SecWebSocketKeyOwned::try_from(String::from("nonce\n"))
                .expect_err("invalid field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            SecWebSocketKeyOwned::try_from("nonce\n")
                .expect_err("invalid borrowed field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn singleton_decoder_rejects_repeated_keys() {
        let mut table = TestSink::new();
        table
            .set_values(
                SecWebSocketKey::name(),
                EncodedValues::from_vec(vec![
                    FieldValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
                    FieldValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
                ]),
            )
            .expect("table accepts raw values");
        assert_eq!(
            SecWebSocketKey::view(&table).expect_err("singleton rejects duplicates").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
    }
}
