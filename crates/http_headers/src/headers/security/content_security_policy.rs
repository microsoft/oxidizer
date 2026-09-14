// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{fmt, str};

use super::super::shared::FieldLinesIter;
use crate::sink::{FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValue, FieldValueRef, validate};

/// Defines the `Content-Security-Policy` header.
///
/// # Specification
///
/// Defined by [Content Security Policy Level 3 section 7.1](https://www.w3.org/TR/CSP3/#csp-header).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{ContentSecurityPolicy, ContentSecurityPolicyOwned};
///
/// let mut map = HeaderMap::new();
/// ContentSecurityPolicy::insert(
///     &mut map,
///     ContentSecurityPolicyOwned::new("default-src 'self'")?,
/// )?;
/// assert!(ContentSecurityPolicy::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct ContentSecurityPolicy {
    _private: (),
}

/// Owned value for the `Content-Security-Policy` header.
///
/// # Specification
///
/// Defined by [Content Security Policy Level 3 section 7.1].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::ContentSecurityPolicyOwned::new("default-src 'self'")?;
/// assert_eq!(value.policies().count(), 1);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Content-Security-Policy: default-src 'self'` sets a default policy.
/// `Content-Security-Policy: default-src 'self'; script-src 'nonce-abc123'`
/// adds a script-specific source list. Multiple field lines are independent
/// policies and are preserved separately.
///
/// [Content Security Policy Level 3 section 7.1]: https://www.w3.org/TR/CSP3/#csp-header
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ContentSecurityPolicyOwned {
    values: FieldLinesIter,
}

/// Borrowed value for the `Content-Security-Policy` header.
/// # Examples
///
/// ```
/// use http_headers::headers::{ContentSecurityPolicy, ContentSecurityPolicyView};
/// use http_headers::source::{FieldLines, FieldSource};
/// use http_headers::{Field, FieldName};
///
/// struct Source;
/// impl FieldSource for Source {
///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
///         (name == &FieldName::ContentSecurityPolicy)
///             .then(|| FieldLines::single(name, b"default-src 'self'; img-src *"))
///     }
/// }
///
/// let view: ContentSecurityPolicyView<'_> =
///     ContentSecurityPolicy::view(&Source)?.expect("header is present");
/// let policies = view.policy_strs().collect::<Result<Vec<_>, _>>()?;
/// assert_eq!(policies, ["default-src 'self'; img-src *"]);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct ContentSecurityPolicyView<'a> {
    values: FieldLines<'a>,
}

impl fmt::Debug for ContentSecurityPolicyOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContentSecurityPolicyOwned")
            .field("value_count", &self.values.len())
            .finish()
    }
}

impl fmt::Debug for ContentSecurityPolicyView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContentSecurityPolicyView")
            .field("value_count", &self.values.len())
            .finish()
    }
}

impl ContentSecurityPolicyOwned {
    /// Constructs one opaque policy field value.
    ///
    /// # Errors
    ///
    /// Returns an error when `policy` is not a safe HTTP field value.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentSecurityPolicyOwned;
    ///
    /// let value = ContentSecurityPolicyOwned::new("default-src 'self'")?;
    /// let policies = value.policy_strs().collect::<Result<Vec<_>, _>>()?;
    /// assert_eq!(policies, ["default-src 'self'"]);
    ///
    /// assert!(ContentSecurityPolicyOwned::new("default-src\nscript-src").is_err());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn new(policy: impl AsRef<str>) -> Result<Self, DecodeError> {
        let policy = policy.as_ref();
        let value = FieldValue::from_str(policy).map_err(|_invalid| super::super::invalid_syntax(&FieldName::ContentSecurityPolicy))?;
        Ok(Self::from_field_value(value))
    }

    /// Constructs one opaque policy from arbitrary safe field-value bytes.
    ///
    /// # Errors
    ///
    /// Returns an error for controls, DEL, CR, or LF.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentSecurityPolicyOwned;
    ///
    /// let value = ContentSecurityPolicyOwned::from_bytes(b"default-src 'self'; img-src *")?;
    /// assert_eq!(
    ///     value.policies().next(),
    ///     Some(b"default-src 'self'; img-src *".as_slice()),
    /// );
    ///
    /// assert!(ContentSecurityPolicyOwned::from_bytes(b"default-src\x7f").is_err());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn from_bytes(policy: impl AsRef<[u8]>) -> Result<Self, DecodeError> {
        let policy = policy.as_ref();
        if !validate::field_value(policy) {
            return Err(super::super::invalid_syntax(&FieldName::ContentSecurityPolicy));
        }
        let value = validated_field_value(policy);
        Ok(Self::from_field_value(value))
    }

    /// Adds another independently enforced policy field line.
    ///
    /// # Errors
    ///
    /// Returns an error when `policy` is not a safe HTTP field value.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentSecurityPolicyOwned;
    ///
    /// let value =
    ///     ContentSecurityPolicyOwned::new("default-src 'self'")?.with_policy("script-src 'none'")?;
    /// let policies = value.policy_strs().collect::<Result<Vec<_>, _>>()?;
    /// assert_eq!(policies, ["default-src 'self'", "script-src 'none'"]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn with_policy(mut self, policy: &str) -> Result<Self, DecodeError> {
        let value = FieldValue::from_str(policy).map_err(|_invalid| super::super::invalid_syntax(&FieldName::ContentSecurityPolicy))?;
        self.values.push(value);
        Ok(self)
    }

    /// Iterates raw policy bytes in field-line order.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentSecurityPolicyOwned;
    ///
    /// let value = ContentSecurityPolicyOwned::new("default-src 'self'")?.with_policy("img-src *")?;
    /// let policies = value.policies().collect::<Vec<_>>();
    /// assert_eq!(
    ///     policies,
    ///     [b"default-src 'self'".as_slice(), b"img-src *".as_slice()],
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn policies(&self) -> impl Iterator<Item = &[u8]> {
        self.values.iter().map(FieldValue::as_bytes)
    }

    /// Iterates policy text as UTF-8.
    ///
    /// # Errors
    ///
    /// An item contains [`DecodeErrorKind::InvalidUtf8`] when that field line
    /// is not UTF-8. Iteration resumes with the following field line.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentSecurityPolicyOwned;
    ///
    /// let value = ContentSecurityPolicyOwned::from_bytes(b"default-src 'self'")?;
    /// let policies = value.policy_strs().collect::<Result<Vec<_>, _>>()?;
    /// assert_eq!(policies, ["default-src 'self'"]);
    ///
    /// let opaque = ContentSecurityPolicyOwned::from_bytes(b"\xff")?;
    /// assert!(opaque.policy_strs().next().expect("one policy").is_err());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn policy_strs(&self) -> impl Iterator<Item = Result<&str, DecodeError>> {
        self.policies().map(policy_str)
    }

    #[cfg(feature = "serde")]
    pub(crate) fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'_>> + '_ {
        self.values.iter().map(FieldValue::as_field_value_ref)
    }

    fn from_field_value(value: FieldValue) -> Self {
        Self {
            values: FieldLinesIter::one(value),
        }
    }
}

impl ContentSecurityPolicyView<'_> {
    pub(crate) fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'_>> + '_ {
        self.values.repeated()
    }

    /// Iterates raw policy bytes in field-line order.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentSecurityPolicy;
    /// use http_headers::source::{FieldLines, FieldSource};
    /// use http_headers::{Field, FieldName};
    ///
    /// struct Source;
    /// impl FieldSource for Source {
    ///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
    ///         (name == &FieldName::ContentSecurityPolicy)
    ///             .then(|| FieldLines::single(name, b"default-src 'self'"))
    ///     }
    /// }
    ///
    /// let view = ContentSecurityPolicy::view(&Source)?.expect("header is present");
    /// assert_eq!(
    ///     view.policies().collect::<Vec<_>>(),
    ///     [b"default-src 'self'".as_slice()],
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn policies(&self) -> impl Iterator<Item = &[u8]> {
        self.values.repeated().map(FieldValueRef::as_bytes)
    }

    /// Iterates policy text as UTF-8.
    ///
    /// # Errors
    ///
    /// An item contains [`DecodeErrorKind::InvalidUtf8`] when that field line
    /// is not UTF-8. Iteration resumes with the following field line.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentSecurityPolicy;
    /// use http_headers::source::{FieldLines, FieldSource};
    /// use http_headers::{Field, FieldName};
    ///
    /// struct Source;
    /// impl FieldSource for Source {
    ///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
    ///         (name == &FieldName::ContentSecurityPolicy)
    ///             .then(|| FieldLines::single(name, b"default-src 'self'; img-src *"))
    ///     }
    /// }
    ///
    /// let view = ContentSecurityPolicy::view(&Source)?.expect("header is present");
    /// let policies = view.policy_strs().collect::<Result<Vec<_>, _>>()?;
    /// assert_eq!(policies, ["default-src 'self'; img-src *"]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn policy_strs(&self) -> impl Iterator<Item = Result<&str, DecodeError>> {
        self.policies().map(policy_str)
    }
}

impl Field for ContentSecurityPolicy {
    type View<'a> = ContentSecurityPolicyView<'a>;
    type Owned = ContentSecurityPolicyOwned;

    fn name() -> &'static FieldName {
        &FieldName::ContentSecurityPolicy
    }

    fn view_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(values) = source.lines(Self::name()) else {
            return Ok(None);
        };
        values.validate_custom_source()?;
        Ok(Some(ContentSecurityPolicyView { values }))
    }

    fn owned_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        let mut copied = FieldLinesIter::empty();
        for (_, owned) in lines.repeated_owned()? {
            copied.push(owned);
        }
        Ok(Some(ContentSecurityPolicyOwned { values: copied }))
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), value.values.into_encoded())
    }
}

impl TryFrom<&str> for ContentSecurityPolicyOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<String> for ContentSecurityPolicyOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = FieldValue::try_from(value).map_err(|_invalid| super::super::invalid_syntax(&FieldName::ContentSecurityPolicy))?;
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for ContentSecurityPolicyOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        Ok(Self::from_field_value(value))
    }
}

fn policy_str(bytes: &[u8]) -> Result<&str, DecodeError> {
    str::from_utf8(bytes).map_err(|_invalid| DecodeError::new(&FieldName::ContentSecurityPolicy, DecodeErrorKind::InvalidUtf8))
}

fn validated_field_value(bytes: &[u8]) -> FieldValue {
    FieldValue::from_bytes(bytes).expect("field-value validation accepted these exact bytes")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{ContentSecurityPolicy, ContentSecurityPolicyOwned};
    use crate::sink::{EncodedValues, FieldSink};
    use crate::source::FieldSource;
    use crate::{DecodeErrorKind, Field, FieldValue, TestSink};

    #[test]
    fn constructors_preserve_policy_lines_and_reject_unsafe_bytes() {
        let policy = ContentSecurityPolicyOwned::new("default-src 'self'")
            .expect("valid policy")
            .with_policy("script-src 'none'")
            .expect("valid second policy");
        assert_eq!(
            policy.policies().collect::<Vec<_>>(),
            [b"default-src 'self'".as_slice(), b"script-src 'none'".as_slice()]
        );
        assert_eq!(
            policy.policy_strs().collect::<Result<Vec<_>, _>>(),
            Ok(vec!["default-src 'self'", "script-src 'none'"])
        );
        assert_eq!(format!("{policy:?}"), "ContentSecurityPolicyOwned { value_count: 2 }");

        let bytes = ContentSecurityPolicyOwned::from_bytes(b"sandbox").expect("safe bytes are accepted");
        assert_eq!(bytes.policies().next(), Some(b"sandbox".as_slice()));
        assert_eq!(
            ContentSecurityPolicyOwned::new("default-src\r\nscript-src")
                .expect_err("line breaks are unsafe")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            ContentSecurityPolicyOwned::from_bytes(b"default-src\x7f")
                .expect_err("DEL is unsafe")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            ContentSecurityPolicyOwned::new("default-src 'self'")
                .expect("valid policy")
                .with_policy("script-src\n")
                .expect_err("unsafe appended policy")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn conversions_and_utf8_errors_are_reported_per_policy() {
        let borrowed = ContentSecurityPolicyOwned::try_from("default-src *").expect("borrowed string converts");
        let owned = ContentSecurityPolicyOwned::try_from(String::from("sandbox")).expect("owned string converts");
        let field = FieldValue::from_static("upgrade-insecure-requests");
        let from_field = ContentSecurityPolicyOwned::try_from(field).expect("field converts");
        assert_eq!(borrowed.policy_strs().next(), Some(Ok("default-src *")));
        assert_eq!(owned.policy_strs().next(), Some(Ok("sandbox")));
        assert_eq!(from_field.policy_strs().next(), Some(Ok("upgrade-insecure-requests")));
        assert_eq!(
            ContentSecurityPolicyOwned::try_from(String::from("sandbox\n"))
                .expect_err("invalid field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let opaque = FieldValue::from_bytes([0xff]).expect("obs-text is a safe field value");
        let policy = ContentSecurityPolicyOwned::try_from(opaque).expect("opaque bytes are allowed");
        assert_eq!(
            policy
                .policy_strs()
                .next()
                .expect("one policy")
                .expect_err("policy is not UTF-8")
                .kind(),
            DecodeErrorKind::InvalidUtf8
        );
    }

    #[test]
    fn header_round_trip_covers_views_owned_values_and_absence() {
        let mut table = TestSink::new();
        assert!(ContentSecurityPolicy::view(&table).expect("absent view succeeds").is_none());
        assert!(
            ContentSecurityPolicy::owned(&table)
                .expect("absent owned decode succeeds")
                .is_none()
        );

        table
            .set_values(
                ContentSecurityPolicy::name(),
                EncodedValues::from_vec(vec![
                    FieldValue::from_static("default-src 'self'"),
                    FieldValue::from_static("script-src 'none'"),
                ]),
            )
            .expect("table accepts policies");
        let view = ContentSecurityPolicy::view(&table)
            .expect("view decodes")
            .expect("header is present");
        assert_eq!(
            view.policies().collect::<Vec<_>>(),
            [b"default-src 'self'".as_slice(), b"script-src 'none'".as_slice()]
        );
        assert_eq!(
            view.policy_strs().collect::<Result<Vec<_>, _>>(),
            Ok(vec!["default-src 'self'", "script-src 'none'",])
        );
        assert_eq!(view.field_values().count(), 2);
        assert_eq!(format!("{view:?}"), "ContentSecurityPolicyView { value_count: 2 }");

        let owned = ContentSecurityPolicy::owned(&table)
            .expect("owned value decodes")
            .expect("header is present");
        let mut output = TestSink::new();
        ContentSecurityPolicy::insert(&mut output, owned).expect("owned value inserts");
        assert_eq!(
            output
                .lines(ContentSecurityPolicy::name())
                .expect("inserted values")
                .repeated()
                .map(crate::FieldValueRef::as_bytes)
                .collect::<Vec<_>>(),
            [b"default-src 'self'".as_slice(), b"script-src 'none'".as_slice()]
        );
    }
}
