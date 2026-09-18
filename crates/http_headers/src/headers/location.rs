// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Validated URI-reference support for the `Location` header.

use std::{fmt, str};

use fluent_uri::Uri;

use crate::{DecodeError, FieldName, FieldValue, FieldValueRef, SingleValueField};

/// Defines the `Location` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 10.2.2](https://www.rfc-editor.org/rfc/rfc9110#section-10.2.2)
/// using the URI-reference grammar from
/// [RFC 3986 section 4](https://www.rfc-editor.org/rfc/rfc3986#section-4).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{Location, LocationOwned};
///
/// let mut map = HeaderMap::new();
/// Location::insert(&mut map, LocationOwned::try_from("/next")?)?;
/// assert!(Location::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct Location {
    _private: (),
}

/// Owned value for the `Location` header.
///
/// # Specification
///
/// The field is defined by [RFC 9110 section 10.2.2] and its URI-reference
/// grammar by [RFC 3986 section 4].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::LocationOwned::try_from("../people?tab=1#profile")?;
/// assert_eq!(value.as_str()?, "../people?tab=1#profile");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Location: https://example.com/people` is absolute,
/// `Location: /accounts/12345` is relative, and `Location: #profile` is a
/// fragment-only reference. An empty field value is also a valid reference.
///
/// [RFC 9110 section 10.2.2]: https://www.rfc-editor.org/rfc/rfc9110#section-10.2.2
/// [RFC 3986 section 4]: https://www.rfc-editor.org/rfc/rfc3986#section-4
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct LocationOwned(FieldValue);

/// Borrowed value for the `Location` header.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```
/// use http_headers::headers::{Location, LocationView};
/// use http_headers::{FieldValue, SingleValueField};
///
/// let field = FieldValue::from_static("/next");
/// let view: LocationView<'_> =
///     <Location as SingleValueField>::decode_view(field.as_field_value_ref())?;
/// assert_eq!(view.as_str()?, "/next");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct LocationView<'a> {
    text: &'a str,
}

impl fmt::Debug for LocationOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocationOwned").field("redacted", &true).finish_non_exhaustive()
    }
}

impl fmt::Debug for LocationView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocationView").field("redacted", &true).finish_non_exhaustive()
    }
}

impl LocationOwned {
    /// Returns the URI-reference bytes.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::LocationOwned;
    ///
    /// let value = LocationOwned::try_from("/next")?;
    /// assert_eq!(value.as_bytes(), b"/next");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Returns the URI-reference as UTF-8.
    /// # Errors
    ///
    /// Returns an error if the stored wire value is unexpectedly non-UTF-8.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::LocationOwned::try_from("/next")?;
    /// assert_eq!(value.as_str()?, "/next");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_str(&self) -> Result<&str, DecodeError> {
        str::from_utf8(self.0.as_bytes()).map_err(|_invalid| super::invalid_syntax(&FieldName::Location))
    }

    /// Returns reusable wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::LocationOwned;
    ///
    /// let value = LocationOwned::try_from("https://example.com/people")?;
    /// let field = value.into_field_value();
    /// assert_eq!(field.as_bytes(), b"https://example.com/people");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::shared::impl_field_value_conversion!(LocationOwned, |value| value.0);

impl<'a> LocationView<'a> {
    /// Returns the URI-reference bytes.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::Location;
    /// use http_headers::{FieldValue, SingleValueField};
    ///
    /// let field = FieldValue::from_static("../people?tab=1#profile");
    /// let view = <Location as SingleValueField>::decode_view(field.as_field_value_ref())?;
    /// assert_eq!(view.as_bytes(), b"../people?tab=1#profile");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_bytes(self) -> &'a [u8] {
        self.text.as_bytes()
    }

    /// Returns the URI-reference as UTF-8.
    ///
    /// # Errors
    ///
    /// This validated view always returns `Ok`; the fallible signature is
    /// retained for API compatibility with the owned form.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::LocationOwned::try_from("/next")?;
    /// assert_eq!(value.as_str()?, "/next");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the fallible signature intentionally matches the owned representation"
    )]
    pub const fn as_str(self) -> Result<&'a str, DecodeError> {
        Ok(self.text)
    }
}

impl SingleValueField for Location {
    type View<'a> = LocationView<'a>;
    type Owned = LocationOwned;

    fn name() -> &'static FieldName {
        &FieldName::Location
    }

    #[inline]
    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        let text = validate(value.as_bytes())?;
        Ok(LocationView { text })
    }

    #[inline]
    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        LocationOwned::try_from(value)
    }

    fn decode_view_with(value: FieldValueRef<'_>, mode: crate::DecodeMode) -> Result<Self::View<'_>, DecodeError> {
        let text = validate_with(value.as_bytes(), mode)?;
        Ok(LocationView { text })
    }

    fn decode_owned_with(mut value: FieldValue, mode: crate::DecodeMode) -> Result<Self::Owned, DecodeError> {
        validate_with(value.as_bytes(), mode)?;
        value.set_sensitive(true);
        Ok(LocationOwned(value))
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.0
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.0
    }
}

impl TryFrom<&str> for LocationOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let mut value = FieldValue::from_str(value).map_err(|_invalid| super::invalid_syntax(&FieldName::Location))?;
        value.set_sensitive(true);
        Self::try_from(value)
    }
}

impl TryFrom<String> for LocationOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let mut value = FieldValue::try_from(value).map_err(|_invalid| super::invalid_syntax(&FieldName::Location))?;
        value.set_sensitive(true);
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for LocationOwned {
    type Error = DecodeError;

    fn try_from(mut value: FieldValue) -> Result<Self, Self::Error> {
        validate(value.as_bytes())?;
        value.set_sensitive(true);
        Ok(Self(value))
    }
}

#[inline]
fn validate(bytes: &[u8]) -> Result<&str, DecodeError> {
    if let Some(text) = is_simple_reference(bytes) {
        return Ok(text);
    }
    let text = str::from_utf8(bytes).map_err(|_invalid| super::invalid_syntax(&FieldName::Location))?;
    validate_general_reference(text)?;
    Ok(text)
}

fn validate_with(bytes: &[u8], mode: crate::DecodeMode) -> Result<&str, DecodeError> {
    if mode == crate::DecodeMode::Strict {
        return validate(bytes);
    }
    let text = str::from_utf8(bytes).map_err(|_invalid| super::invalid_syntax(&FieldName::Location))?;
    if is_simple_reference(bytes).is_some() || validate_general_reference(text).is_ok() {
        return Ok(text);
    }
    if !text.contains('\\') {
        return Err(super::invalid_syntax(&FieldName::Location));
    }
    validate_general_reference(&text.replace('\\', "/"))?;
    Ok(text)
}

/// Validates everything the origin-relative scanner declines to recognize.
///
/// The recognized subset already covers the shapes a redirect normally
/// carries, so keeping the RFC 3986 parse cold and behind a call leaves
/// [`validate`] small enough to fold into its callers without changing
/// validation semantics.
#[cold]
#[inline(never)]
fn validate_general_reference(value: &str) -> Result<(), DecodeError> {
    validate_general_reference_len(value.len())?;
    Uri::parse(value)
        .map(|_parsed| ())
        .map_err(|_invalid| super::invalid_syntax(&FieldName::Location))
}

#[inline]
fn validate_general_reference_len(length: usize) -> Result<(), DecodeError> {
    if i32::try_from(length).is_ok() {
        Ok(())
    } else {
        Err(super::invalid_syntax(&FieldName::Location))
    }
}

/// Accepts the references that need no RFC 3986 parse.
///
/// The scanner recognizes a `path-absolute` with an optional query and one
/// optional fragment written only with `unreserved`, `sub-delims`, `:`, `@`,
/// `/`, and `?`, and the same shape behind a `scheme "://" host [":" port]`
/// prefix. Anything else, including every percent escape, userinfo, and
/// IP-literal, returns `false` and is handed to the full parser unchanged.
fn is_simple_reference(bytes: &[u8]) -> Option<&str> {
    http_headers_simd::as_simple_uri_reference(bytes)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![expect(
        clippy::assertions_on_result_states,
        reason = "tests classify parser outcomes without needing successful values"
    )]

    use super::{Location, LocationOwned, validate, validate_general_reference, validate_general_reference_len, validate_with};
    use crate::sink::FieldSink;
    use crate::{DecodeErrorKind, DecodeMode, FieldName, FieldValue, SingleValueField, TestSink};

    #[test]
    fn constructors_accessors_debug_and_round_trip() {
        for wire in ["", "/next?tab=1#profile", "../next", "https://example.com/a"] {
            let owned = LocationOwned::try_from(wire).expect("valid URI reference");
            assert_eq!(owned.as_bytes(), wire.as_bytes());
            assert_eq!(owned.as_str(), Ok(wire));
            assert!(format!("{owned:?}").contains("redacted"));
            assert!(owned.clone().into_field_value().is_sensitive());

            let mut table = TestSink::new();
            Location::insert(&mut table, owned).expect("table accepts location");
            let view = Location::view(&table).expect("valid location").expect("present");
            assert_eq!(view.as_bytes(), wire.as_bytes());
            assert_eq!(view.as_str(), Ok(wire));
            assert!(format!("{view:?}").contains("redacted"));
            assert!(
                Location::owned(&table)
                    .expect("valid owned location")
                    .expect("present")
                    .into_field_value()
                    .is_sensitive()
            );
            table.remove_values(&FieldName::Location);
            assert!(Location::view(&table).expect("absence is valid").is_none());
        }

        assert_eq!(
            LocationOwned::try_from(String::from("/owned")).expect("valid owned URI").as_str(),
            Ok("/owned")
        );
        assert_eq!(
            "/parsed".parse::<LocationOwned>().expect("shared FromStr implementation").as_str(),
            Ok("/parsed")
        );
    }

    /// The fast path replaces the RFC 3986 parse outright, so anything it
    /// accepts must be something the general parser would also accept.
    #[test]
    fn the_recognized_subset_agrees_with_the_general_parser() {
        let alphabet = b"aZ0:@/?#%[].+-_~!$&'()*,;=\\ \t";
        for first in alphabet {
            for second in alphabet {
                for third in alphabet {
                    for fourth in alphabet {
                        let bytes = [*first, *second, *third, *fourth];
                        for prefix in [b"".as_slice(), b"https://h.example".as_slice()] {
                            let mut candidate = prefix.to_vec();
                            candidate.extend_from_slice(&bytes);
                            let Some(text) = super::is_simple_reference(&candidate) else {
                                continue;
                            };
                            assert!(
                                validate_general_reference(text).is_ok(),
                                "recognized {text:?} that the general parser rejects"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn strict_and_relaxed_validation_cover_fallbacks() {
        assert!(validate(b"/simple/path?query#fragment").is_ok());
        assert!(validate(b"https://example.com/docs?x=1#top").is_ok());
        assert!(validate(b"https://example.com:8443/docs").is_ok());
        assert!(validate(b"https://user@example.com/docs").is_ok());
        assert!(validate(b"https://example.com/caf%C3%A9").is_ok());
        assert!(validate(b"https://example.com/a b").is_err());
        assert!(validate_general_reference("mailto:user@example.com").is_ok());
        assert!(validate(b"%zz").is_err());
        assert!(validate(b"has space").is_err());
        assert!(validate(&[0xff]).is_err());

        assert!(validate_with(b"/a\\b", DecodeMode::Strict).is_err());
        assert_eq!(validate_with(b"/simple", DecodeMode::Relaxed), Ok("/simple"));
        assert_eq!(validate_with(b"/a\\b", DecodeMode::Relaxed), Ok("/a\\b"));
        assert!(validate_with(b"bad\\%zz", DecodeMode::Relaxed).is_err());
        assert!(validate_with(b"%zz", DecodeMode::Relaxed).is_err());
        assert!(validate_with(&[0xff], DecodeMode::Relaxed).is_err());

        let relaxed = FieldValue::from_static("/a\\b");
        assert!(<Location as SingleValueField>::decode_view(relaxed.as_field_value_ref()).is_err());
        assert!(<Location as SingleValueField>::decode_view_with(relaxed.as_field_value_ref(), DecodeMode::Relaxed).is_ok());
        let owned = <Location as SingleValueField>::decode_owned_with(relaxed, DecodeMode::Relaxed).expect("relaxed owned location");
        assert!(owned.into_field_value().is_sensitive());

        let strict = FieldValue::from_static("/strict");
        let strict_view = <Location as SingleValueField>::decode_view(strict.as_field_value_ref()).expect("valid strict location");
        assert_eq!(strict_view.as_str(), Ok("/strict"));
        let strict_owned = <Location as SingleValueField>::decode_owned(strict.clone()).expect("valid strict owned location");
        assert_eq!(<Location as SingleValueField>::as_field_value(&strict_owned), &strict);

        let invalid = FieldValue::try_from(vec![0xff]).expect("obs-text field value");
        assert_eq!(
            LocationOwned::try_from(invalid.clone())
                .expect_err("location requires UTF-8")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert!(LocationOwned::try_from("\n").is_err());
        assert!(LocationOwned::try_from(String::from("\n")).is_err());
        assert!(validate_general_reference_len(i32::MAX as usize).is_ok());
        assert!(validate_general_reference_len(i32::MAX as usize + 1).is_err());
        let malformed = LocationOwned(invalid);
        assert_eq!(
            malformed.as_str().expect_err("malformed private storage is not UTF-8").kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }
}
