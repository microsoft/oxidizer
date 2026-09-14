// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::ops::Range;

use super::shared::{CorsMethodView, common_method, invalid_syntax, invalid_token, method_ref, trimmed_range, untrimmed_range};
use crate::sink::{EncodedValues, FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValue, FieldValueRef};

/// Defines the `Access-Control-Request-Method` header.
///
/// # Specification
///
/// Defined by the Fetch standard's
/// [CORS-preflight fetch section](https://fetch.spec.whatwg.org/#http-access-control-request-method).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{AccessControlRequestMethod, AccessControlRequestMethodOwned};
///
/// let mut map = HeaderMap::new();
/// let method = AccessControlRequestMethodOwned::from_method("PATCH")?;
/// AccessControlRequestMethod::insert(&mut map, method)?;
/// assert!(AccessControlRequestMethod::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct AccessControlRequestMethod {
    _private: (),
}

/// Owned value for the `Access-Control-Request-Method` header.
///
/// # Specification
///
/// Defined by the Fetch standard's [CORS-preflight fetch section].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::AccessControlRequestMethodOwned::try_from("PATCH")?;
/// assert_eq!(value.method()?.as_str(), "PATCH");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Access-Control-Request-Method: PATCH` requests permission to use `PATCH`;
/// extension method tokens are also preserved.
///
/// [CORS-preflight fetch section]: https://fetch.spec.whatwg.org/#http-access-control-request-method
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AccessControlRequestMethodOwned {
    method: RequestMethodStore,
}

/// Storage for an owned request method.
///
/// Registered methods are known constants, so they are held by name and the
/// wire storage is kept only for extension tokens.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum RequestMethodStore {
    Registered(&'static str),
    Extension(FieldValue),
}

impl RequestMethodStore {
    #[expect(clippy::inline_always, reason = "measured: fusing the store into the caller saves 29 Ir")]
    #[inline(always)]
    fn of(values: &FieldLines<'_>) -> Result<Self, DecodeError> {
        let mut repeated = values.repeated();
        let value = repeated.next().expect("FieldLines always contains at least one field line");
        if repeated.next().is_some() {
            return Err(DecodeError::new(values.name(), DecodeErrorKind::UnexpectedMultipleValues));
        }
        if let Some(method) = registered_method(value.as_bytes()) {
            return Ok(Self::Registered(method));
        }
        extension_method(value.as_bytes())?;
        let owned = values.exactly_one_owned()?;
        Ok(Self::Extension(owned))
    }

    fn into_field_value(self) -> FieldValue {
        match self {
            Self::Registered(method) => FieldValue::from_static(method),
            Self::Extension(value) => value,
        }
    }

    fn as_method(&self) -> Result<CorsMethodView<'_>, DecodeError> {
        match self {
            Self::Registered(method) => Ok(CorsMethodView(method)),
            Self::Extension(value) => extension_method(value.as_bytes()),
        }
    }
}

/// Recognizes a registered method for owned storage.
///
/// Kept apart from [`common_method`] so that inlining it into the owned
/// decoding path cannot change how the borrowed path is compiled.
#[expect(clippy::inline_always, reason = "measured: keeping the owned matcher separate saves 29 Ir")]
#[inline(always)]
fn registered_method(token: &[u8]) -> Option<&'static str> {
    match token {
        b"GET" => Some("GET"),
        b"PUT" => Some("PUT"),
        b"HEAD" => Some("HEAD"),
        b"POST" => Some("POST"),
        b"PATCH" => Some("PATCH"),
        b"TRACE" => Some("TRACE"),
        b"DELETE" => Some("DELETE"),
        b"CONNECT" => Some("CONNECT"),
        b"OPTIONS" => Some("OPTIONS"),
        _ => None,
    }
}

/// Borrowed value for the `Access-Control-Request-Method` header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{AccessControlRequestMethod, AccessControlRequestMethodView};
/// use http_headers::source::{FieldLines, FieldSource};
/// use http_headers::{Field, FieldName};
///
/// struct Source;
///
/// impl FieldSource for Source {
///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
///         (name == &FieldName::AccessControlRequestMethod)
///             .then(|| FieldLines::single(name, b"POST"))
///     }
/// }
///
/// let view: AccessControlRequestMethodView<'_> =
///     AccessControlRequestMethod::view(&Source)?.expect("request method");
/// assert_eq!(view.method().as_str(), "POST");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct AccessControlRequestMethodView<'a> {
    value: FieldValueRef<'a>,
    method: CorsMethodView<'a>,
}

impl AccessControlRequestMethodOwned {
    /// Constructs a request method, including extension methods.
    ///
    /// # Errors
    ///
    /// Returns an error if `method` is not an HTTP method token.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AccessControlRequestMethodOwned;
    ///
    /// let value = AccessControlRequestMethodOwned::from_method("PATCH")?;
    /// assert_eq!(value.method()?.as_str(), "PATCH");
    /// assert!(AccessControlRequestMethodOwned::from_method("bad method").is_err());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn from_method(method: impl AsRef<str>) -> Result<Self, DecodeError> {
        Self::try_from(method.as_ref())
    }

    /// Returns the method without allocating.
    ///
    /// # Errors
    ///
    /// Returns an error if stored metadata does not match the field value.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AccessControlRequestMethodOwned;
    ///
    /// let registered = AccessControlRequestMethodOwned::from_method("POST")?;
    /// assert_eq!(registered.method()?.as_str(), "POST");
    ///
    /// let extension = AccessControlRequestMethodOwned::try_from(" CUSTOM ")?;
    /// assert_eq!(extension.method()?.as_str(), "CUSTOM");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn method(&self) -> Result<CorsMethodView<'_>, DecodeError> {
        self.method.as_method()
    }

    /// Returns reusable wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AccessControlRequestMethodOwned;
    ///
    /// let registered = AccessControlRequestMethodOwned::from_method("DELETE")?;
    /// assert_eq!(registered.into_field_value(), "DELETE");
    ///
    /// let extension = AccessControlRequestMethodOwned::try_from(" CUSTOM ")?;
    /// assert_eq!(extension.into_field_value(), " CUSTOM ");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }

    #[cfg(feature = "serde")]
    pub(crate) fn field_value(&self) -> FieldValueRef<'_> {
        match &self.method {
            RequestMethodStore::Registered(method) => FieldValueRef::new(method.as_bytes()),
            RequestMethodStore::Extension(value) => value.as_field_value_ref(),
        }
    }
}

super::super::shared::impl_field_value_conversion!(AccessControlRequestMethodOwned, |value| value.method.into_field_value());

impl fmt::Display for AccessControlRequestMethodOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.method().map_err(|_invalid| fmt::Error)?.as_str())
    }
}

impl<'a> AccessControlRequestMethodView<'a> {
    /// Returns the method without allocating.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AccessControlRequestMethod;
    /// use http_headers::source::{FieldLines, FieldSource};
    /// use http_headers::{Field, FieldName};
    ///
    /// struct Source;
    ///
    /// impl FieldSource for Source {
    ///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
    ///         (name == &FieldName::AccessControlRequestMethod)
    ///             .then(|| FieldLines::single(name, b"PATCH"))
    ///     }
    /// }
    ///
    /// let view = AccessControlRequestMethod::view(&Source)?.expect("request method");
    /// assert_eq!(view.method().as_str(), "PATCH");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn method(self) -> CorsMethodView<'a> {
        self.method
    }

    /// Returns the original field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AccessControlRequestMethod;
    /// use http_headers::source::{FieldLines, FieldSource};
    /// use http_headers::{Field, FieldName};
    ///
    /// struct Source;
    ///
    /// impl FieldSource for Source {
    ///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
    ///         (name == &FieldName::AccessControlRequestMethod)
    ///             .then(|| FieldLines::single(name, b" CUSTOM "))
    ///     }
    /// }
    ///
    /// let view = AccessControlRequestMethod::view(&Source)?.expect("request method");
    /// assert_eq!(view.method().as_str(), "CUSTOM");
    /// assert_eq!(view.as_field_value(), " CUSTOM ");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_field_value(self) -> FieldValueRef<'a> {
        self.value
    }
}

/// Validates one `Access-Control-Request-Method` field value.
///
/// Registered methods are recognized whole, so the common case never reaches
/// the token scan behind it.
#[inline]
fn request_method_of(bytes: &[u8]) -> Result<CorsMethodView<'_>, DecodeError> {
    match common_method(bytes) {
        Some(method) => Ok(CorsMethodView(method)),
        None => extension_method(bytes),
    }
}

/// Validates a field value that no registered method matched.
fn extension_method(bytes: &[u8]) -> Result<CorsMethodView<'_>, DecodeError> {
    let token = &bytes[request_method_range(bytes)];
    method_ref(token).ok_or_else(|| invalid_token(&FieldName::AccessControlRequestMethod))
}

/// Locates the method token inside a `Access-Control-Request-Method` value.
fn request_method_range(bytes: &[u8]) -> Range<usize> {
    untrimmed_range(bytes).unwrap_or_else(|| trimmed_range(bytes))
}

/// Reads the sole field line, with the cardinality check folded into the caller.
///
/// [`FieldLines::exactly_one`] carries only an inlining hint, which the
/// optimizer declines on this path, leaving a call in front of what is
/// otherwise a token match. Forcing the fusion here mirrors what
/// [`RequestMethodStore::of`] already does for the owned form, so the two arms
/// reach the same shape as well as the same answer.
#[expect(
    clippy::inline_always,
    reason = "fusing the cardinality check into the caller keeps the single-line view a straight run"
)]
#[inline(always)]
fn sole_line<'a>(values: &FieldLines<'a>) -> Result<FieldValueRef<'a>, DecodeError> {
    let mut repeated = values.repeated();
    let value = repeated.next().expect("FieldLines always contains at least one field line");
    if repeated.next().is_some() {
        return Err(DecodeError::new(values.name(), DecodeErrorKind::UnexpectedMultipleValues));
    }
    Ok(value)
}

impl Field for AccessControlRequestMethod {
    type View<'a> = AccessControlRequestMethodView<'a>;
    type Owned = AccessControlRequestMethodOwned;

    fn name() -> &'static FieldName {
        &FieldName::AccessControlRequestMethod
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
        let value = sole_line(&lines)?;
        let method = request_method_of(value.as_bytes())?;
        Ok(Some(AccessControlRequestMethodView { value, method }))
    }

    /// Validates without building a view, since the owned form keeps only the
    /// field value.
    #[inline]
    fn owned_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_custom_source()?;
        let method = RequestMethodStore::of(&lines)?;
        Ok(Some(AccessControlRequestMethodOwned { method }))
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), EncodedValues::single(value.method.into_field_value()))
    }
}

#[cfg(feature = "http")]
impl TryFrom<http::Method> for AccessControlRequestMethodOwned {
    type Error = DecodeError;

    fn try_from(method: http::Method) -> Result<Self, Self::Error> {
        Self::from_method(method)
    }
}

impl TryFrom<&str> for AccessControlRequestMethodOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = FieldValue::from_str(value).map_err(|_invalid| invalid_syntax(&FieldName::AccessControlRequestMethod))?;
        Self::try_from(value)
    }
}

impl TryFrom<String> for AccessControlRequestMethodOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = FieldValue::try_from(value).map_err(|_invalid| invalid_syntax(&FieldName::AccessControlRequestMethod))?;
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for AccessControlRequestMethodOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        if let Some(method) = registered_method(value.as_bytes()) {
            return Ok(Self {
                method: RequestMethodStore::Registered(method),
            });
        }
        extension_method(value.as_bytes())?;
        Ok(Self {
            method: RequestMethodStore::Extension(value),
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{AccessControlRequestMethod, AccessControlRequestMethodOwned, RequestMethodStore, registered_method};
    use crate::headers::cors::test_support::TestMap;
    use crate::{DecodeErrorKind, FieldName, FieldValue};

    #[test]
    fn registered_and_extension_constructors_cover_storage_and_accessors() {
        for method in ["GET", "PUT", "HEAD", "POST", "PATCH", "TRACE", "DELETE", "CONNECT", "OPTIONS"] {
            assert_eq!(registered_method(method.as_bytes()), Some(method), "{method}");
            let owned = AccessControlRequestMethodOwned::from_method(method).expect("registered method");
            assert_eq!(owned.method().expect("method view").as_str(), method);
            assert_eq!(owned.to_string(), method);
            assert_eq!(owned.into_field_value(), method);
        }
        assert_eq!(registered_method(b"CUSTOM"), None);

        let extension = AccessControlRequestMethodOwned::try_from(" CUSTOM ").expect("trimmed extension");
        assert_eq!(extension.method().expect("extension").as_str(), "CUSTOM");
        assert_eq!(extension.into_field_value(), " CUSTOM ");
        assert_eq!(
            AccessControlRequestMethodOwned::try_from(String::from("CUSTOM"))
                .expect("owned extension")
                .method()
                .expect("method")
                .as_str(),
            "CUSTOM"
        );
        assert_eq!(
            AccessControlRequestMethodOwned::try_from(FieldValue::from_static("CUSTOM"))
                .expect("field extension")
                .method()
                .expect("method")
                .as_str(),
            "CUSTOM"
        );

        #[cfg(feature = "http")]
        assert_eq!(
            AccessControlRequestMethodOwned::try_from(http::Method::PATCH)
                .expect("HTTP method")
                .method()
                .expect("method")
                .as_str(),
            "PATCH"
        );
    }

    #[test]
    fn header_decode_insert_absence_duplicates_and_invalid_tokens_are_reported() {
        let registered = TestMap::new(&FieldName::AccessControlRequestMethod, vec![FieldValue::from_static("PATCH")]);
        let view = AccessControlRequestMethod::view(&registered)
            .expect("registered view")
            .expect("present");
        assert_eq!(view.method().as_str(), "PATCH");
        assert_eq!(view.as_field_value(), "PATCH");
        let owned = AccessControlRequestMethod::owned(&registered)
            .expect("registered owned")
            .expect("present");
        assert_eq!(owned.method().expect("method").as_str(), "PATCH");

        let extension = TestMap::new(&FieldName::AccessControlRequestMethod, vec![FieldValue::from_static(" CUSTOM ")]);
        assert_eq!(
            AccessControlRequestMethod::view(&extension)
                .expect("extension view")
                .expect("present")
                .method()
                .as_str(),
            "CUSTOM"
        );
        let extension_owned = AccessControlRequestMethod::owned(&extension)
            .expect("extension owned")
            .expect("present");
        assert_eq!(extension_owned.method().expect("extension").as_str(), "CUSTOM");

        let mut sink = TestMap::new(&FieldName::Accept, Vec::new());
        AccessControlRequestMethod::insert(&mut sink, extension_owned).expect("insert request method");
        assert_eq!(sink.name, &FieldName::AccessControlRequestMethod);
        assert_eq!(sink.values, extension.values);

        let absent = TestMap::new(&FieldName::Accept, Vec::new());
        assert!(AccessControlRequestMethod::view(&absent).expect("absent").is_none());
        assert!(AccessControlRequestMethod::owned(&absent).expect("absent").is_none());

        let duplicate = TestMap::new(
            &FieldName::AccessControlRequestMethod,
            vec![FieldValue::from_static("GET"), FieldValue::from_static("POST")],
        );
        assert_eq!(
            AccessControlRequestMethod::owned(&duplicate).expect_err("singleton header").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        assert_eq!(
            AccessControlRequestMethod::view(&duplicate).expect_err("singleton header").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );

        for (wire, kind) in [
            ("", DecodeErrorKind::InvalidToken),
            (" ", DecodeErrorKind::InvalidToken),
            ("bad method", DecodeErrorKind::InvalidToken),
            ("bad,method", DecodeErrorKind::InvalidToken),
        ] {
            let error = AccessControlRequestMethodOwned::try_from(wire).expect_err("invalid request method");
            assert_eq!(error.kind(), kind, "{wire:?}");
        }
        let error = AccessControlRequestMethodOwned::try_from(String::from("bad\nvalue")).expect_err("invalid field bytes");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
        assert_eq!(
            AccessControlRequestMethodOwned::try_from("bad\nvalue")
                .expect_err("invalid borrowed field bytes")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn corrupted_extension_storage_is_revalidated_by_accessors() {
        let value = AccessControlRequestMethodOwned {
            method: RequestMethodStore::Extension(FieldValue::from_static("bad method")),
        };
        assert_eq!(value.method().expect_err("corrupt method").kind(), DecodeErrorKind::InvalidToken);
    }
}
