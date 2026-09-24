// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Opaque repeated `Set-Cookie` field-line storage.

use std::str::FromStr;
use std::{fmt, slice, vec};

use super::shared::FieldLinesIter;
use crate::sink::{FieldSink, InsertError, InsertErrorKind};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, Field, FieldName, FieldValue, FieldValueRef};

/// Defines the `Set-Cookie` header.
///
/// # Specification
///
/// Defined by [RFC 6265 section 4.1](https://www.rfc-editor.org/rfc/rfc6265#section-4.1).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{SetCookie, SetCookieOwned};
///
/// let mut map = HeaderMap::new();
/// let mut cookies = SetCookieOwned::new();
/// cookies.push_str("session=abc123; Path=/; HttpOnly; Secure")?;
/// SetCookie::insert(&mut map, cookies)?;
/// assert!(SetCookie::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct SetCookie {
    _private: (),
}

/// Owned value for the `Set-Cookie` header.
///
/// # Specification
///
/// Defined by [RFC 6265 section 4.1].
///
/// # Examples
///
/// ```rust
/// let mut cookies = http_headers::headers::SetCookieOwned::new();
/// cookies.push_str("session=abc123; Path=/; HttpOnly; Secure")?;
/// assert_eq!(cookies.len(), 1);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// [RFC 6265 section 4.1]: https://www.rfc-editor.org/rfc/rfc6265#section-4.1
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct SetCookieOwned {
    values: FieldLinesIter,
}

/// Borrowed value for the `Set-Cookie` header.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), http_headers::DecodeError> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{SetCookie, SetCookieView};
///
/// let mut map = HeaderMap::new();
/// map.append(
///     http::header::SET_COOKIE,
///     http::HeaderValue::from_static("id=a3fWa; Max-Age=2592000; Secure; HttpOnly"),
/// );
/// let cookies: SetCookieView<'_> = SetCookie::view(&map)?.expect("present");
/// assert_eq!(cookies.len(), 1);
/// # Ok::<(), http_headers::DecodeError>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
pub struct SetCookieView<'a> {
    values: FieldLines<'a>,
}

impl fmt::Debug for SetCookieOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetCookieOwned").field("value_count", &self.values.len()).finish()
    }
}

impl fmt::Debug for SetCookieView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetCookieView").field("value_count", &self.values.len()).finish()
    }
}

impl SetCookieOwned {
    /// Creates an empty collection.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert!(http_headers::headers::SetCookieOwned::new().is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            values: FieldLinesIter::empty(),
        }
    }

    /// Adds one nonempty HTTP field value.
    ///
    /// This method does not parse RFC 6265 cookie grammar. Callers that include
    /// untrusted data must encode it before constructing the field value;
    /// delimiters such as `;` are accepted as opaque cookie attributes.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is empty.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let mut cookies = http_headers::headers::SetCookieOwned::new();
    /// cookies.push(http_headers::FieldValue::from_static("a=1"))?;
    /// assert_eq!(cookies.len(), 1);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn push(&mut self, mut value: FieldValue) -> Result<(), DecodeError> {
        validate(value.as_field_value_ref())?;
        value.set_sensitive(true);
        self.values.push(value);
        Ok(())
    }

    /// Adds one nonempty cookie string as an opaque field value.
    ///
    /// This method does not parse RFC 6265 cookie grammar. Callers that include
    /// untrusted data must encode it before interpolation; `;` and `=` are
    /// accepted because they are meaningful cookie delimiters.
    ///
    /// # Errors
    ///
    /// Returns an error when the string is not a valid nonempty field value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let mut cookies = http_headers::headers::SetCookieOwned::new();
    /// cookies.push_str("a=1; HttpOnly")?;
    /// assert_eq!(cookies.len(), 1);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn push_str(&mut self, value: &str) -> Result<(), DecodeError> {
        let value = match FieldValue::from_str(value) {
            Ok(value) => value,
            Err(_invalid) => return Err(super::invalid_syntax(&FieldName::SetCookie)),
        };
        self.push(value)
    }

    /// Iterates the stored field values.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let mut cookies = http_headers::headers::SetCookieOwned::new();
    /// cookies.push_str("a=1")?;
    /// assert!(cookies.iter().any(|cookie| cookie == "a=1"));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn iter(&self) -> slice::Iter<'_, FieldValue> {
        self.values.iter()
    }

    /// Mutably iterates the stored field values.
    ///
    /// Each value must remain nonempty for later insertion to succeed.
    /// Insertion restores the sensitive marker on every value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldSensitivity;
    ///
    /// let mut cookies = http_headers::headers::SetCookieOwned::new();
    /// cookies.push_str("a=1")?;
    /// cookies
    ///     .iter_mut()
    ///     .for_each(|cookie| cookie.set_sensitivity(FieldSensitivity::Sensitive));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn iter_mut(&mut self) -> slice::IterMut<'_, FieldValue> {
        self.values.iter_mut()
    }

    /// Returns the number of cookie field values.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let mut cookies = http_headers::headers::SetCookieOwned::new();
    /// cookies.push_str("a=1")?;
    /// assert_eq!(cookies.len(), 1);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns whether no cookie field values are stored.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert!(http_headers::headers::SetCookieOwned::new().is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub(crate) fn into_sensitive_encoded_values(self) -> Result<crate::sink::EncodedValues, InsertError> {
        fn restore_sensitivity(mut value: FieldValue) -> FieldValue {
            value.set_sensitive(true);
            value
        }

        if self.values.iter().any(FieldValue::is_empty) {
            return Err(InsertError::new(InsertErrorKind::InvalidValue));
        }

        Ok(match self.values {
            FieldLinesIter::Empty => crate::sink::EncodedValues::new(),
            FieldLinesIter::One(value) => crate::sink::EncodedValues::single(restore_sensitivity(value)),
            FieldLinesIter::Many(values) => crate::sink::EncodedValues::from_vec(values.into_iter().map(restore_sensitivity).collect()),
        })
    }
}

impl FromStr for SetCookieOwned {
    type Err = DecodeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut cookies = Self::new();
        cookies.push_str(value)?;
        Ok(cookies)
    }
}

impl Default for SetCookieOwned {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> SetCookieView<'a> {
    /// Iterates the borrowed field values.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), http_headers::DecodeError> {
    /// use http::HeaderMap;
    /// use http_headers::Field;
    /// use http_headers::headers::SetCookie;
    /// let mut map = HeaderMap::new();
    /// map.append(
    ///     http::header::SET_COOKIE,
    ///     http::HeaderValue::from_static("a=1"),
    /// );
    /// assert_eq!(
    ///     SetCookie::view(&map)?.map(|cookies| cookies.iter().count()),
    ///     Some(1)
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = FieldValueRef<'a>> + '_ {
        self.values.repeated().map(|value| value.with_sensitive(true))
    }

    /// Returns the number of cookie field values.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), http_headers::DecodeError> {
    /// use http::HeaderMap;
    /// use http_headers::Field;
    /// use http_headers::headers::SetCookie;
    /// let mut map = HeaderMap::new();
    /// map.append(
    ///     http::header::SET_COOKIE,
    ///     http::HeaderValue::from_static("a=1"),
    /// );
    /// assert_eq!(SetCookie::view(&map)?.map(|cookies| cookies.len()), Some(1));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns whether no cookie field values are present.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), http_headers::DecodeError> {
    /// use http::HeaderMap;
    /// use http_headers::Field;
    /// use http_headers::headers::SetCookie;
    /// let mut map = HeaderMap::new();
    /// map.append(
    ///     http::header::SET_COOKIE,
    ///     http::HeaderValue::from_static("a=1"),
    /// );
    /// assert_eq!(
    ///     SetCookie::view(&map)?.map(|cookies| cookies.is_empty()),
    ///     Some(false)
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    #[must_use]
    #[expect(
        clippy::unused_self,
        reason = "a SetCookie view is nonempty by construction and this query keeps collection semantics"
    )]
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl<'a> IntoIterator for &'a SetCookieOwned {
    type Item = &'a FieldValue;
    type IntoIter = slice::Iter<'a, FieldValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

impl<'a> IntoIterator for &'a mut SetCookieOwned {
    type Item = &'a mut FieldValue;
    type IntoIter = slice::IterMut<'a, FieldValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.iter_mut()
    }
}

impl IntoIterator for SetCookieOwned {
    type Item = FieldValue;
    type IntoIter = vec::IntoIter<FieldValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.into_vec().into_iter()
    }
}

impl Field for SetCookie {
    type View<'a> = SetCookieView<'a>;
    type Owned = SetCookieOwned;

    fn name() -> &'static FieldName {
        &FieldName::SetCookie
    }

    #[expect(
        clippy::inline_always,
        reason = "generic header forwarding should monomorphize into each source call site"
    )]
    #[inline(always)]
    fn view_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        decode_view_values(source.lines(Self::name()))
    }

    #[expect(
        clippy::inline_always,
        reason = "generic header forwarding should monomorphize into each source call site"
    )]
    #[inline(always)]
    fn owned_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        decode_owned_values(source.lines(Self::name()))
    }

    #[expect(
        clippy::inline_always,
        reason = "generic header forwarding should monomorphize into each sink call site"
    )]
    #[inline(always)]
    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), value.into_sensitive_encoded_values()?)
    }
}

fn decode_view_values(values: Option<FieldLines<'_>>) -> Result<Option<SetCookieView<'_>>, DecodeError> {
    let Some(values) = values else {
        return Ok(None);
    };
    values.validate_custom_source()?;
    let mut iter = values.repeated();
    if let Some(value) = iter.next() {
        validate(value)?;
    }
    iter.try_for_each(validate)?;
    Ok(Some(SetCookieView { values }))
}

fn decode_owned_values(values: Option<FieldLines<'_>>) -> Result<Option<SetCookieOwned>, DecodeError> {
    let Some(values) = values else {
        return Ok(None);
    };
    let mut decoded = SetCookieOwned {
        values: FieldLinesIter::empty(),
    };
    for (value, mut owned) in values.repeated_owned()? {
        validate(value)?;
        if !owned.is_sensitive() {
            owned.set_sensitive(true);
        }
        decoded.values.push(owned);
    }
    Ok(Some(decoded))
}

fn validate(value: FieldValueRef<'_>) -> Result<(), DecodeError> {
    if value.as_bytes().is_empty() {
        Err(super::invalid_syntax(&FieldName::SetCookie))
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![expect(
        clippy::assertions_on_result_states,
        reason = "tests classify parser outcomes without needing successful values"
    )]

    use super::{SetCookie, SetCookieOwned};
    use crate::sink::{EncodedValues, FieldSink, InsertError, InsertErrorKind};
    use crate::source::{FieldLines, FieldSource};
    use crate::{DecodeErrorKind, FieldName, FieldValue, TestSink};

    struct Source(Vec<FieldValue>);

    impl FieldSource for Source {
        fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
            (name == &FieldName::SetCookie)
                .then(|| FieldLines::from_slice(name, &self.0))
                .flatten()
        }
    }

    #[test]
    fn owned_collection_covers_storage_and_iterator_forms() {
        let mut cookies = SetCookieOwned::default();
        assert!(cookies.is_empty());
        assert_eq!(cookies.len(), 0);
        cookies.push(FieldValue::from_static("a=1")).expect("nonempty cookie");
        cookies.push_str("b=2").expect("valid cookie string");
        assert_eq!(cookies.len(), 2);
        assert!(cookies.iter().all(FieldValue::is_sensitive));
        cookies.iter_mut().for_each(|value| value.set_sensitive(true));
        assert_eq!((&cookies).into_iter().count(), 2);
        assert_eq!((&mut cookies).into_iter().count(), 2);
        assert!(format!("{cookies:?}").contains("value_count"));
        assert_eq!(cookies.clone().into_iter().count(), 2);

        let parsed = "c=3".parse::<SetCookieOwned>().expect("one cookie parses");
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            "".parse::<SetCookieOwned>().expect_err("empty cookie must fail").kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert!(cookies.push_str("\n").is_err());
    }

    #[test]
    fn header_decoding_and_encoding_cover_absent_empty_and_repeated() {
        let source = Source(vec![FieldValue::from_static("a=1"), FieldValue::from_static("b=2")]);
        let view = SetCookie::view(&source).expect("valid repeated cookies").expect("present");
        assert_eq!(view.len(), 2);
        assert!(!view.is_empty());
        assert_eq!(view.iter().count(), 2);
        assert!(format!("{view:?}").contains("value_count"));

        let owned = SetCookie::owned(&source).expect("valid owned cookies").expect("present");
        assert!(owned.iter().all(FieldValue::is_sensitive));

        let mut first = FieldValue::from_static("a=1");
        first.set_sensitive(true);
        let mut second = FieldValue::from_static("b=2");
        second.set_sensitive(true);
        assert!(
            SetCookie::owned(&Source(vec![first, second]))
                .expect("sensitive cookies remain valid")
                .expect("present")
                .iter()
                .all(FieldValue::is_sensitive)
        );

        let mut table = TestSink::new();
        SetCookie::insert(&mut table, owned).expect("table accepts cookies");
        assert_eq!(table.lines(&FieldName::SetCookie).expect("cookies stored").repeated().count(), 2);
        table.remove_values(&FieldName::SetCookie);
        assert!(SetCookie::view(&table).expect("absence is valid").is_none());
        assert!(SetCookie::owned(&table).expect("absence is valid").is_none());

        let invalid = Source(vec![FieldValue::from_static("a=1"), FieldValue::from_static("")]);
        assert!(SetCookie::view(&invalid).is_err());
        assert!(SetCookie::owned(&invalid).is_err());

        let empty = Source(vec![FieldValue::from_static("")]);
        assert!(SetCookie::view(&empty).is_err());
        assert!(SetCookie::owned(&empty).is_err());

        let _ = EncodedValues::new();
    }

    #[test]
    fn emission_rejects_values_invalidated_through_mutable_iteration() {
        let mut cookies = SetCookieOwned::new();
        cookies.push_str("a=1").expect("valid cookie");
        *cookies.iter_mut().next().expect("one cookie") = FieldValue::from_static("");

        let mut sink = TestSink::new();
        assert_eq!(
            SetCookie::insert(&mut sink, cookies),
            Err(InsertError::new(InsertErrorKind::InvalidValue))
        );
        assert!(sink.lines(&FieldName::SetCookie).is_none());

        let mut cookies = SetCookieOwned::new();
        cookies.push_str("a=1").expect("valid cookie");
        *(&mut cookies).into_iter().next().expect("one cookie") = FieldValue::from_static("");
        assert_eq!(
            crate::sink::FieldSinkExt::append_set_cookie(&mut sink, cookies).err(),
            Some(InsertError::new(InsertErrorKind::InvalidValue))
        );
        assert!(sink.lines(&FieldName::SetCookie).is_none());
    }
}
