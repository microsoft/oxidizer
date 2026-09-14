// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Validated opaque `User-Agent` values.

use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef, SingleValueField};

/// Defines the `User-Agent` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 10.1.5](https://www.rfc-editor.org/rfc/rfc9110#section-10.1.5).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{UserAgent, UserAgentOwned};
///
/// let mut map = HeaderMap::new();
/// UserAgent::insert(&mut map, UserAgentOwned::try_from_static("client/1")?)?;
/// assert!(UserAgent::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct UserAgent {
    _private: (),
}

/// Owned value for the `User-Agent` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 10.1.5].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::UserAgentOwned::try_from("example-client/1.0")?;
/// assert_eq!(value.as_bytes(), b"example-client/1.0");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `User-Agent: curl/8.5.0` identifies a command-line client, while
/// `User-Agent: example-client/1.0 (integration test)` includes a comment.
///
/// [RFC 9110 section 10.1.5]: https://www.rfc-editor.org/rfc/rfc9110#section-10.1.5
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UserAgentOwned(FieldValue);

/// Borrowed value for the `User-Agent` header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```
/// use http_headers::headers::{UserAgent, UserAgentView};
/// use http_headers::{FieldValue, SingleValueField};
///
/// let field = FieldValue::from_static("curl/8.4.0");
/// let view: UserAgentView<'_> =
///     <UserAgent as SingleValueField>::decode_view(field.as_field_value_ref())?;
/// assert_eq!(view.as_str()?, "curl/8.4.0");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct UserAgentView<'a>(FieldValueRef<'a>);

impl UserAgentOwned {
    /// Validates a static string without using a panicking constructor.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty or invalid field value.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::UserAgentOwned;
    ///
    /// let value = UserAgentOwned::try_from_static("curl/8.4.0")?;
    /// assert_eq!(value.as_bytes(), b"curl/8.4.0");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn try_from_static(value: &'static str) -> Result<Self, DecodeError> {
        Self::try_from(value)
    }

    /// Returns the wire bytes.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::UserAgentOwned::try_from("client/1")?;
    /// assert_eq!(value.as_bytes(), b"client/1");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Returns reusable wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::UserAgentOwned;
    ///
    /// let value = UserAgentOwned::try_from_static("example-client/1.0")?;
    /// let field = value.into_field_value();
    /// assert_eq!(field.as_bytes(), b"example-client/1.0");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::shared::impl_field_value_conversion!(UserAgentOwned, |value| value.0);

impl<'a> UserAgentView<'a> {
    pub(crate) const fn field_value(self) -> FieldValueRef<'a> {
        self.0
    }

    /// Returns the wire bytes.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::UserAgentOwned::try_from("client/1")?;
    /// assert_eq!(value.as_bytes(), b"client/1");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_bytes(self) -> &'a [u8] {
        self.0.as_bytes()
    }

    /// Returns the value as UTF-8.
    ///
    /// # Errors
    ///
    /// Returns an error when the field contains non-UTF-8 bytes.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::UserAgent;
    /// use http_headers::{FieldValue, SingleValueField};
    ///
    /// let field = FieldValue::from_static("Mozilla/5.0 (X11; Linux x86_64)");
    /// let view = <UserAgent as SingleValueField>::decode_view(field.as_field_value_ref())?;
    /// assert_eq!(view.as_str()?, "Mozilla/5.0 (X11; Linux x86_64)");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_str(self) -> Result<&'a str, DecodeError> {
        self.0
            .to_str()
            .map_err(|_invalid| DecodeError::new(&FieldName::UserAgent, DecodeErrorKind::InvalidUtf8))
    }
}

impl SingleValueField for UserAgent {
    type View<'a> = UserAgentView<'a>;
    type Owned = UserAgentOwned;

    fn name() -> &'static FieldName {
        &FieldName::UserAgent
    }

    #[inline]
    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        validate(value)?;
        Ok(UserAgentView(value))
    }

    #[inline]
    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        UserAgentOwned::try_from(value)
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.0
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.0
    }
}

impl TryFrom<&str> for UserAgentOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = FieldValue::from_str(value).map_err(|_invalid| super::invalid_syntax(&FieldName::UserAgent))?;
        Self::try_from(value)
    }
}

impl TryFrom<String> for UserAgentOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = FieldValue::try_from(value).map_err(|_invalid| super::invalid_syntax(&FieldName::UserAgent))?;
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for UserAgentOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        validate(value.as_field_value_ref())?;
        Ok(Self(value))
    }
}

#[inline]
fn validate(value: FieldValueRef<'_>) -> Result<(), DecodeError> {
    if super::has_non_ows(value.as_bytes()) {
        Ok(())
    } else {
        Err(super::invalid_syntax(&FieldName::UserAgent))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![expect(
        clippy::assertions_on_result_states,
        reason = "tests classify parser outcomes without needing successful values"
    )]

    use super::{UserAgent, UserAgentOwned};
    use crate::sink::FieldSink;
    use crate::{DecodeErrorKind, FieldName, FieldValue, SingleValueField, TestSink};

    #[test]
    fn constructors_accessors_and_header_round_trip() {
        let static_value = UserAgentOwned::try_from_static("client/1").expect("valid user agent");
        assert_eq!(static_value.as_bytes(), b"client/1");
        assert_eq!(
            UserAgentOwned::try_from(String::from("client/2"))
                .expect("valid owned string")
                .as_bytes(),
            b"client/2"
        );
        assert_eq!(
            "client/3"
                .parse::<UserAgentOwned>()
                .expect("shared FromStr implementation")
                .as_bytes(),
            b"client/3"
        );
        assert_eq!(static_value.clone().into_field_value(), FieldValue::from_static("client/1"));

        let mut table = TestSink::new();
        UserAgent::insert(&mut table, static_value).expect("table accepts user agent");
        let view = UserAgent::view(&table).expect("valid user agent").expect("present");
        assert_eq!(view.as_bytes(), b"client/1");
        assert_eq!(view.as_str(), Ok("client/1"));
        assert_eq!(
            UserAgent::owned(&table)
                .expect("valid owned user agent")
                .expect("present")
                .as_bytes(),
            b"client/1"
        );
        table.remove_values(&FieldName::UserAgent);
        assert!(UserAgent::view(&table).expect("absence is valid").is_none());
    }

    #[test]
    fn validation_and_utf8_failures_are_reported() {
        for value in ["", " ", "\t", " \t"] {
            assert!(UserAgentOwned::try_from(value).is_err(), "{value:?}");
        }
        assert!(UserAgentOwned::try_from(String::from("\n")).is_err());

        let obs = FieldValue::try_from(vec![0xff]).expect("obs-text is valid field data");
        let view = <UserAgent as SingleValueField>::decode_view(obs.as_field_value_ref()).expect("nonempty obs-text user agent");
        assert_eq!(
            view.as_str().expect_err("obs-text is not UTF-8").kind(),
            DecodeErrorKind::InvalidUtf8
        );
        assert!(<UserAgent as SingleValueField>::decode_owned(FieldValue::from_static(" ")).is_err());

        let valid = FieldValue::from_static("direct/1");
        let view = <UserAgent as SingleValueField>::decode_view(valid.as_field_value_ref()).expect("valid borrowed user agent");
        assert_eq!(view.as_bytes(), b"direct/1");
        let owned = <UserAgent as SingleValueField>::decode_owned(valid.clone()).expect("valid owned user agent");
        assert_eq!(<UserAgent as SingleValueField>::as_field_value(&owned), &valid);
        assert!(UserAgentOwned::try_from("\n").is_err());
        assert!(UserAgentOwned::try_from(String::from("\n")).is_err());
    }
}
