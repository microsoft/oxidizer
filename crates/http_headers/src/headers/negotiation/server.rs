// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::shared::{invalid, invalid_syntax};
use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef, SingleValueField};

/// Defines the `Server` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 10.2.4](https://www.rfc-editor.org/rfc/rfc9110#section-10.2.4).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{Server, ServerOwned};
///
/// let mut map = HeaderMap::new();
/// Server::insert(&mut map, ServerOwned::try_from("example/1")?)?;
/// assert!(Server::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct Server {
    _private: (),
}

/// Owned value for the `Server` header.
///
/// The product/comment grammar is deliberately not exposed semantically;
/// callers can inspect the preserved wire bytes.
///
/// # Specification
///
/// Defined by [RFC 9110 section 10.2.4].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::ServerOwned::try_from("example/1")?;
/// assert_eq!(value.as_bytes(), b"example/1");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Server: nginx/1.25.3` identifies one product.
/// `Server: example-server/2.0 (internal)` includes a comment.
///
/// [RFC 9110 section 10.2.4]: https://www.rfc-editor.org/rfc/rfc9110#section-10.2.4
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ServerOwned(FieldValue);

/// Borrowed value for the `Server` header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{Server, ServerView};
/// use http_headers::{FieldValueRef, SingleValueField};
///
/// let value: ServerView<'_> = Server::decode_view(FieldValueRef::new(b"nginx/1.25.3"))?;
/// assert_eq!(value.as_bytes(), b"nginx/1.25.3");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct ServerView<'a>(FieldValueRef<'a>);

impl ServerOwned {
    /// Returns the preserved wire bytes.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ServerOwned;
    ///
    /// let value = ServerOwned::try_from("nginx/1.25.3")?;
    /// assert_eq!(value.as_bytes(), b"nginx/1.25.3");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Returns the original field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ServerOwned;
    ///
    /// let value = ServerOwned::try_from("Apache/2.4.58 (Unix)")?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"Apache/2.4.58 (Unix)");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_field_value(&self) -> &FieldValue {
        &self.0
    }

    /// Returns reusable wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ServerOwned;
    ///
    /// let value = ServerOwned::try_from("nginx/1.25.3")?;
    /// let field_value = value.into_field_value();
    /// assert_eq!(field_value.as_bytes(), b"nginx/1.25.3");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::super::shared::impl_field_value_conversion!(ServerOwned, |value| value.0);

impl<'a> ServerView<'a> {
    /// Returns the preserved wire bytes.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::Server;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let value = Server::decode_view(FieldValueRef::new(b"Apache/2.4.58 (Unix)"))?;
    /// assert_eq!(value.as_bytes(), b"Apache/2.4.58 (Unix)");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_bytes(self) -> &'a [u8] {
        self.0.as_bytes()
    }

    /// Returns the value as UTF-8.
    ///
    /// # Errors
    ///
    /// Returns an error when the field contains non-UTF-8 `obs-text`.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::Server;
    /// use http_headers::{DecodeErrorKind, FieldValueRef, SingleValueField};
    ///
    /// let value = Server::decode_view(FieldValueRef::new(b"nginx/1.25.3"))?;
    /// assert_eq!(value.as_str()?, "nginx/1.25.3");
    ///
    /// let binary = Server::decode_view(FieldValueRef::new(b"\xff"))?;
    /// assert_eq!(
    ///     binary.as_str().unwrap_err().kind(),
    ///     DecodeErrorKind::InvalidUtf8
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_str(self) -> Result<&'a str, DecodeError> {
        self.0
            .to_str()
            .map_err(|_invalid| invalid(&FieldName::Server, DecodeErrorKind::InvalidUtf8))
    }

    /// Returns the original field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::Server;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let value = Server::decode_view(FieldValueRef::new(b"Apache/2.4.58 (Unix)"))?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"Apache/2.4.58 (Unix)");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_field_value(self) -> FieldValueRef<'a> {
        self.0
    }
}

impl SingleValueField for Server {
    type View<'a> = ServerView<'a>;
    type Owned = ServerOwned;

    fn name() -> &'static FieldName {
        &FieldName::Server
    }

    #[inline]
    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        validate_server(value)?;
        Ok(ServerView(value))
    }

    #[inline]
    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        ServerOwned::try_from(value)
    }

    #[inline]
    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.0
    }

    #[inline]
    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.0
    }
}

impl TryFrom<&str> for ServerOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = FieldValue::from_str(value).map_err(|_invalid| invalid_syntax(&FieldName::Server))?;
        Self::try_from(value)
    }
}

impl TryFrom<String> for ServerOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = FieldValue::try_from(value).map_err(|_invalid| invalid_syntax(&FieldName::Server))?;
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for ServerOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        validate_server(value.as_field_value_ref())?;
        Ok(Self(value))
    }
}

#[inline]
fn validate_server(value: FieldValueRef<'_>) -> Result<(), DecodeError> {
    if super::super::has_non_ows(value.as_bytes()) {
        Ok(())
    } else {
        Err(invalid_syntax(&FieldName::Server))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use super::{Server, ServerOwned, validate_server};
    use crate::{DecodeErrorKind, FieldName, FieldValue, FieldValueRef, SingleValueField};

    #[test]
    fn owned_and_borrowed_server_values_preserve_opaque_wire_data() {
        assert_eq!(
            ServerOwned::try_from("borrowed/1").expect("valid borrowed server").as_bytes(),
            b"borrowed/1"
        );
        let owned = ServerOwned::try_from(String::from("example/1 (test)")).expect("nonempty server is valid");
        assert_eq!(owned.as_bytes(), b"example/1 (test)");
        assert_eq!(owned.as_field_value().as_bytes(), b"example/1 (test)");
        assert!(format!("{owned:?}").contains("example/1 (test)"));
        let mut hasher = DefaultHasher::new();
        owned.hash(&mut hasher);
        assert_ne!(hasher.finish(), 0);
        assert_eq!(owned.into_field_value().as_bytes(), b"example/1 (test)");

        let view = <Server as SingleValueField>::decode_view(FieldValueRef::new(b"server/2")).expect("borrowed server is valid");
        assert_eq!(view.as_bytes(), b"server/2");
        assert_eq!(view.as_str(), Ok("server/2"));
        assert_eq!(view.as_field_value().as_bytes(), b"server/2");

        let decoded = <Server as SingleValueField>::decode_owned(FieldValue::from_static("server/3")).expect("owned server");
        assert_eq!(<Server as SingleValueField>::as_field_value(&decoded).as_bytes(), b"server/3");
        assert_eq!(<Server as SingleValueField>::into_field_value(decoded).as_bytes(), b"server/3");
    }

    #[test]
    fn server_rejects_empty_values_and_reports_non_utf8_views() {
        for empty in [b"".as_slice(), b" ", b"\t"] {
            assert_eq!(
                validate_server(FieldValueRef::new(empty))
                    .expect_err("OWS-only values are invalid")
                    .kind(),
                DecodeErrorKind::InvalidSyntax
            );
        }
        assert_eq!(
            ServerOwned::try_from(String::from("line\nbreak"))
                .expect_err("invalid field value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            ServerOwned::try_from("line\nbreak")
                .expect_err("invalid borrowed field value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            <Server as SingleValueField>::decode_owned(FieldValue::from_static(" "))
                .expect_err("OWS-only owned value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            ServerOwned::try_from(FieldValue::from_bytes(b"\xff").expect("obs-text is field-safe"))
                .expect("opaque obs-text is valid")
                .as_bytes(),
            b"\xff"
        );
        let view = <Server as SingleValueField>::decode_view(FieldValueRef::new(b"\xff")).expect("opaque obs-text is valid");
        assert_eq!(
            view.as_str().expect_err("obs-text is not UTF-8").kind(),
            DecodeErrorKind::InvalidUtf8
        );
        assert_eq!(<Server as SingleValueField>::name(), &FieldName::Server);
    }
}
