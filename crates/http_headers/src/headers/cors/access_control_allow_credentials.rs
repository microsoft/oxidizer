// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::marker::PhantomData;

use super::shared::{invalid_syntax, trimmed_range};
use crate::sink::{EncodedValues, FieldSink, InsertError};
use crate::source::FieldSource;
use crate::{DecodeError, Field, FieldName, FieldValue, FieldValueRef};

/// Defines the `Access-Control-Allow-Credentials` header.
///
/// # Specification
///
/// Defined by the Fetch standard's
/// [CORS protocol and credentials section](https://fetch.spec.whatwg.org/#http-access-control-allow-credentials).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{
///     AccessControlAllowCredentials, AccessControlAllowCredentialsOwned,
/// };
///
/// let mut map = HeaderMap::new();
/// AccessControlAllowCredentials::insert(&mut map, AccessControlAllowCredentialsOwned::allow())?;
/// assert!(AccessControlAllowCredentials::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct AccessControlAllowCredentials {
    _private: (),
}

/// Owned value for the `Access-Control-Allow-Credentials` header.
///
/// The grammar admits a single field value, so the type carries no storage and
/// always encodes the canonical `true`; whitespace framing a decoded value is
/// not preserved.
///
/// # Specification
///
/// Defined by the Fetch standard's [CORS protocol and credentials section].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::AccessControlAllowCredentialsOwned::allow();
/// assert_eq!(value.into_field_value(), "true");
/// ```
///
/// `Access-Control-Allow-Credentials: true` is the only valid value.
///
/// [CORS protocol and credentials section]: https://fetch.spec.whatwg.org/#http-access-control-allow-credentials
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct AccessControlAllowCredentialsOwned;

/// The only field value `Access-Control-Allow-Credentials` accepts.
const ALLOW_CREDENTIALS_TRUE: &str = "true";

/// The canonical field value, handed out in place of decoded storage.
static ALLOW_CREDENTIALS_VALUE: FieldValue = FieldValue::from_static(ALLOW_CREDENTIALS_TRUE);

/// Borrowed value for the `Access-Control-Allow-Credentials` header.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{AccessControlAllowCredentials, AccessControlAllowCredentialsView};
/// use http_headers::source::{FieldLines, FieldSource};
/// use http_headers::{Field, FieldName};
///
/// struct Source;
///
/// impl FieldSource for Source {
///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
///         (name == &FieldName::AccessControlAllowCredentials)
///             .then(|| FieldLines::single(name, b"true"))
///     }
/// }
///
/// let view: AccessControlAllowCredentialsView<'_> =
///     AccessControlAllowCredentials::view(&Source)?.expect("present");
/// assert_eq!(view.as_str(), "true");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct AccessControlAllowCredentialsView<'a> {
    values: PhantomData<FieldValueRef<'a>>,
}

impl AccessControlAllowCredentialsOwned {
    /// Constructs the only valid value, case-sensitive `true`.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AccessControlAllowCredentialsOwned;
    ///
    /// let value = AccessControlAllowCredentialsOwned::allow();
    /// assert_eq!(value.to_string(), "true");
    /// ```
    pub const fn allow() -> Self {
        Self
    }

    /// Returns reusable wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AccessControlAllowCredentialsOwned;
    ///
    /// let value = AccessControlAllowCredentialsOwned::allow();
    /// let field_value = value.into_field_value();
    /// assert_eq!(field_value, "true");
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::super::shared::impl_field_value_conversion!(AccessControlAllowCredentialsOwned, |_value| ALLOW_CREDENTIALS_VALUE.clone());

impl fmt::Display for AccessControlAllowCredentialsOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(ALLOW_CREDENTIALS_TRUE)
    }
}

impl<'a> AccessControlAllowCredentialsView<'a> {
    /// Returns the semantic value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AccessControlAllowCredentials;
    /// use http_headers::source::{FieldLines, FieldSource};
    /// use http_headers::{Field, FieldName};
    ///
    /// struct Source;
    ///
    /// impl FieldSource for Source {
    ///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
    ///         (name == &FieldName::AccessControlAllowCredentials)
    ///             .then(|| FieldLines::single(name, b"true"))
    ///     }
    /// }
    ///
    /// let view = AccessControlAllowCredentials::view(&Source)?.expect("present");
    /// assert_eq!(view.as_str(), "true");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[expect(
        clippy::unused_self,
        reason = "the validated zero-sized view exposes the same accessor shape as value-carrying views"
    )]
    pub const fn as_str(self) -> &'a str {
        ALLOW_CREDENTIALS_TRUE
    }

    /// Returns the canonical field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AccessControlAllowCredentials;
    /// use http_headers::source::{FieldLines, FieldSource};
    /// use http_headers::{Field, FieldName};
    ///
    /// struct Source;
    ///
    /// impl FieldSource for Source {
    ///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
    ///         (name == &FieldName::AccessControlAllowCredentials)
    ///             .then(|| FieldLines::single(name, b"true"))
    ///     }
    /// }
    ///
    /// let view = AccessControlAllowCredentials::view(&Source)?.expect("present");
    /// assert_eq!(view.as_field_value(), "true");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[expect(
        clippy::unused_self,
        reason = "the validated zero-sized view exposes the same accessor shape as value-carrying views"
    )]
    pub fn as_field_value(self) -> FieldValueRef<'a> {
        ALLOW_CREDENTIALS_VALUE.as_field_value_ref()
    }
}

impl Field for AccessControlAllowCredentials {
    type View<'a> = AccessControlAllowCredentialsView<'a>;
    type Owned = AccessControlAllowCredentialsOwned;

    fn name() -> &'static FieldName {
        &FieldName::AccessControlAllowCredentials
    }

    #[inline]
    fn view_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_custom_source()?;
        if is_credentials_true(lines.exactly_one()?.as_bytes()) {
            Ok(Some(AccessControlAllowCredentialsView { values: PhantomData }))
        } else {
            Err(invalid_syntax(&FieldName::AccessControlAllowCredentials))
        }
    }

    fn owned_with<S>(source: &S, mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        Self::view_with(source, mode).map(|view| view.map(|_| AccessControlAllowCredentialsOwned))
    }

    fn insert<S>(sink: &mut S, _value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), EncodedValues::single(ALLOW_CREDENTIALS_VALUE.clone()))
    }
}

impl TryFrom<&str> for AccessControlAllowCredentialsOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = FieldValue::from_str(value).map_err(|_invalid| invalid_syntax(&FieldName::AccessControlAllowCredentials))?;
        Self::try_from(value)
    }
}

impl TryFrom<String> for AccessControlAllowCredentialsOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = FieldValue::try_from(value).map_err(|_invalid| invalid_syntax(&FieldName::AccessControlAllowCredentials))?;
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for AccessControlAllowCredentialsOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        if is_credentials_true(value.as_bytes()) {
            Ok(Self)
        } else {
            Err(invalid_syntax(&FieldName::AccessControlAllowCredentials))
        }
    }
}
#[inline]
fn is_credentials_true(bytes: &[u8]) -> bool {
    bytes == b"true" || is_trimmed_credentials_true(bytes)
}

#[cold]
fn is_trimmed_credentials_true(bytes: &[u8]) -> bool {
    bytes.get(trimmed_range(bytes)) == Some(b"true".as_slice())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{AccessControlAllowCredentials, AccessControlAllowCredentialsOwned, is_credentials_true};
    use crate::headers::cors::test_support::TestMap;
    use crate::{DecodeErrorKind, FieldName, FieldValue};

    #[test]
    fn canonical_value_construction_display_and_conversion_are_storage_free() {
        let value = AccessControlAllowCredentialsOwned::allow();
        assert_eq!(value, AccessControlAllowCredentialsOwned);
        assert_eq!(value.to_string(), "true");
        assert_eq!(value.into_field_value(), "true");

        assert_eq!(
            AccessControlAllowCredentialsOwned::try_from(" true ").expect("whitespace framed true"),
            value
        );
        assert_eq!(
            AccessControlAllowCredentialsOwned::try_from(String::from("\ttrue\t")).expect("owned true string"),
            value
        );
        assert_eq!(
            AccessControlAllowCredentialsOwned::try_from(FieldValue::from_static("true")).expect("true field"),
            value
        );

        assert!(is_credentials_true(b"true"));
        assert!(is_credentials_true(b" \ttrue\t "));
        assert!(!is_credentials_true(b"TRUE"));
        assert!(!is_credentials_true(b"false"));
    }

    #[test]
    fn header_decode_insert_absence_and_errors_cover_all_paths() {
        let source = TestMap::new(&FieldName::AccessControlAllowCredentials, vec![FieldValue::from_static(" true ")]);
        let view = AccessControlAllowCredentials::view(&source)
            .expect("valid credentials view")
            .expect("present");
        assert_eq!(view.as_str(), "true");
        assert_eq!(view.as_field_value(), "true");
        assert_eq!(
            AccessControlAllowCredentials::owned(&source)
                .expect("valid credentials owned")
                .expect("present"),
            AccessControlAllowCredentialsOwned
        );

        let mut sink = TestMap::new(&FieldName::Accept, Vec::new());
        AccessControlAllowCredentials::insert(&mut sink, AccessControlAllowCredentialsOwned::allow()).expect("insert credentials");
        assert_eq!(sink.name, &FieldName::AccessControlAllowCredentials);
        assert_eq!(sink.values, [FieldValue::from_static("true")]);

        let absent = TestMap::new(&FieldName::Accept, Vec::new());
        assert!(AccessControlAllowCredentials::view(&absent).expect("absent").is_none());
        assert!(AccessControlAllowCredentials::owned(&absent).expect("absent").is_none());

        for wire in ["TRUE", "false", "true value"] {
            let error = AccessControlAllowCredentialsOwned::try_from(wire).expect_err("only lowercase true is accepted");
            assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
        }
        let error = AccessControlAllowCredentialsOwned::try_from(String::from("bad\nvalue")).expect_err("invalid field bytes");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
        assert_eq!(
            AccessControlAllowCredentialsOwned::try_from("bad\nvalue")
                .expect_err("invalid borrowed field bytes")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let invalid = TestMap::new(&FieldName::AccessControlAllowCredentials, vec![FieldValue::from_static("false")]);
        assert_eq!(
            AccessControlAllowCredentials::view(&invalid)
                .expect_err("invalid credentials header")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let duplicate = TestMap::new(
            &FieldName::AccessControlAllowCredentials,
            vec![FieldValue::from_static("true"), FieldValue::from_static("true")],
        );
        assert_eq!(
            AccessControlAllowCredentials::view(&duplicate)
                .expect_err("singleton header")
                .kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
    }
}
