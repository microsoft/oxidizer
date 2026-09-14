// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Write as _;
use std::str;
use std::time::Duration;

use compact_str::CompactString;

use super::super::ExtensionValue;
use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef, SingleValueField, validate};

/// Defines the `Strict-Transport-Security` header.
///
/// # Specification
///
/// Defined by [RFC 6797 section 6.1](https://www.rfc-editor.org/rfc/rfc6797#section-6.1).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{StrictTransportSecurity, StrictTransportSecurityOwned};
///
/// let mut map = HeaderMap::new();
/// StrictTransportSecurity::insert(
///     &mut map,
///     StrictTransportSecurityOwned::new(std::time::Duration::from_secs(60))?,
/// )?;
/// assert!(StrictTransportSecurity::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct StrictTransportSecurity {
    _private: (),
}

/// Owned value for the `Strict-Transport-Security` header.
///
/// # Specification
///
/// Defined by [RFC 6797 section 6.1]. The optional `preload` directive is a
/// nonstandard convention documented by the [HSTS preload service].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::StrictTransportSecurityOwned::builder(
///     std::time::Duration::from_secs(31_536_000),
/// )
/// .include_subdomains()
/// .build()?;
/// assert_eq!(value.max_age(), std::time::Duration::from_secs(31_536_000));
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Strict-Transport-Security: max-age=31536000` sets the lifetime.
/// `Strict-Transport-Security: max-age=31536000; includeSubDomains; preload`
/// also covers subdomains and requests preload-list inclusion.
///
/// [RFC 6797 section 6.1]: https://www.rfc-editor.org/rfc/rfc6797#section-6.1
/// [HSTS preload service]: https://hstspreload.org/
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StrictTransportSecurityOwned {
    value: FieldValue,
    summary: HstsSummary,
}

/// Borrowed value for the `Strict-Transport-Security` header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```
/// use http_headers::headers::{StrictTransportSecurity, StrictTransportSecurityView};
/// use http_headers::{FieldValueRef, SingleValueField};
///
/// let view: StrictTransportSecurityView<'_> = StrictTransportSecurity::decode_view(
///     FieldValueRef::new(b"max-age=31536000; includeSubDomains"),
/// )?;
/// assert_eq!(view.max_age().as_secs(), 31_536_000);
/// assert!(view.include_subdomains());
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct StrictTransportSecurityView<'a> {
    value: FieldValueRef<'a>,
    summary: HstsSummary,
}

/// Builder for a canonical `Strict-Transport-Security` field value.
#[derive(Clone, Debug)]
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// use http_headers::headers::{StrictTransportSecurityBuilder, StrictTransportSecurityOwned};
///
/// let builder: StrictTransportSecurityBuilder =
///     StrictTransportSecurityOwned::builder(Duration::from_secs(31_536_000))
///         .include_subdomains()
///         .preload();
/// let value = builder.build()?;
/// assert_eq!(
///     value.as_field_value().as_bytes(),
///     b"max-age=31536000; includeSubDomains; preload",
/// );
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct StrictTransportSecurityBuilder {
    max_age: Duration,
    include_subdomains: bool,
    preload: bool,
    extensions: Vec<BuilderExtension>,
}

#[derive(Clone, Debug)]
struct BuilderExtension {
    name: CompactString,
    value: Option<CompactString>,
}

/// One borrowed HSTS directive.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// use http_headers::headers::{HstsDirectiveView, StrictTransportSecurityOwned};
///
/// let value = StrictTransportSecurityOwned::builder(Duration::from_secs(60))
///     .include_subdomains()
///     .build()?;
/// let mut directives = value.directives();
/// let first: HstsDirectiveView<'_> = directives.next().transpose()?.expect("max-age is present");
/// assert_eq!(first.name(), "max-age");
/// assert_eq!(first.value(), Some(&b"60"[..]));
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct HstsDirectiveView<'a> {
    raw: &'a [u8],
    name: &'a str,
    value: Option<&'a [u8]>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct HstsSummary {
    max_age: Duration,
    include_subdomains: bool,
    preload: bool,
}

impl StrictTransportSecurityOwned {
    /// Constructs a header containing only `max-age`.
    ///
    /// # Errors
    ///
    /// Returns an error if field-value construction fails.
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::new(Duration::from_secs(31_536_000))?;
    /// assert_eq!(value.max_age(), Duration::from_secs(31_536_000));
    /// assert!(!value.include_subdomains());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn new(max_age: Duration) -> Result<Self, DecodeError> {
        Self::builder(max_age).build()
    }

    /// Creates an HSTS builder with the required `max-age`.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::builder(Duration::from_secs(600))
    ///     .preload()
    ///     .build()?;
    /// assert!(value.preload());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn builder(max_age: Duration) -> StrictTransportSecurityBuilder {
        StrictTransportSecurityBuilder {
            max_age: Duration::from_secs(max_age.as_secs()),
            include_subdomains: false,
            preload: false,
            extensions: Vec::new(),
        }
    }

    /// Returns the `max-age` duration.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::try_from("max-age=31536000")?;
    /// assert_eq!(value.max_age(), Duration::from_secs(31_536_000));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn max_age(&self) -> Duration {
        self.summary.max_age
    }

    /// Returns whether `includeSubDomains` is present.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::try_from("max-age=60; includeSubDomains")?;
    /// assert!(value.include_subdomains());
    ///
    /// let narrow = StrictTransportSecurityOwned::try_from("max-age=60")?;
    /// assert!(!narrow.include_subdomains());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn include_subdomains(&self) -> bool {
        self.summary.include_subdomains
    }

    /// Returns whether the nonstandard `preload` directive is present.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::try_from("max-age=60; preload")?;
    /// assert!(value.preload());
    ///
    /// let plain = StrictTransportSecurityOwned::try_from("max-age=60")?;
    /// assert!(!plain.preload());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn preload(&self) -> bool {
        self.summary.preload
    }

    /// Iterates all directives, including extensions, in wire order.
    ///
    /// # Errors
    ///
    /// An item is [`Err`] when a stored directive has invalid syntax or a
    /// non-UTF-8 name. Iteration resumes with the following directive.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::try_from("max-age=60; includeSubDomains")?;
    /// let names = value
    ///     .directives()
    ///     .map(|directive| Ok(directive?.name()))
    ///     .collect::<Result<Vec<_>, http_headers::DecodeError>>()?;
    /// assert_eq!(names, ["max-age", "includeSubDomains"]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn directives(&self) -> impl Iterator<Item = Result<HstsDirectiveView<'_>, DecodeError>> {
        HstsItems::new(self.value.as_bytes()).map(parse_hsts_directive)
    }

    /// Returns the stored field value.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::try_from("max-age=60; preload")?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"max-age=60; preload");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_field_value(&self) -> &FieldValue {
        &self.value
    }

    /// Returns reusable wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::try_from("max-age=60")?;
    /// let field_value = value.into_field_value();
    /// assert_eq!(field_value.as_bytes(), b"max-age=60");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::super::shared::impl_field_value_conversion!(StrictTransportSecurityOwned, |value| value.value);

impl<'a> StrictTransportSecurityView<'a> {
    /// Returns the `max-age` duration.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurity;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = StrictTransportSecurity::decode_view(FieldValueRef::new(b"max-age=31536000"))?;
    /// assert_eq!(view.max_age().as_secs(), 31_536_000);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn max_age(self) -> Duration {
        self.summary.max_age
    }

    /// Returns whether `includeSubDomains` is present.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurity;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view =
    ///     StrictTransportSecurity::decode_view(FieldValueRef::new(b"max-age=60; includeSubDomains"))?;
    /// assert!(view.include_subdomains());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn include_subdomains(self) -> bool {
        self.summary.include_subdomains
    }

    /// Returns whether the nonstandard `preload` directive is present.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurity;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = StrictTransportSecurity::decode_view(FieldValueRef::new(b"max-age=60; preload"))?;
    /// assert!(view.preload());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn preload(self) -> bool {
        self.summary.preload
    }

    /// Iterates all directives, including extensions, in wire order.
    ///
    /// # Errors
    ///
    /// An item is [`Err`] when a stored directive has invalid syntax or a
    /// non-UTF-8 name. Iteration resumes with the following directive.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurity;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = StrictTransportSecurity::decode_view(FieldValueRef::new(b"max-age=60; preload"))?;
    /// let names = view
    ///     .directives()
    ///     .map(|directive| Ok(directive?.name()))
    ///     .collect::<Result<Vec<_>, http_headers::DecodeError>>()?;
    /// assert_eq!(names, ["max-age", "preload"]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn directives(self) -> impl Iterator<Item = Result<HstsDirectiveView<'a>, DecodeError>> {
        HstsItems::new(self.value.as_bytes()).map(parse_hsts_directive)
    }

    /// Returns the original field value.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurity;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = StrictTransportSecurity::decode_view(FieldValueRef::new(b"max-age=60"))?;
    /// assert_eq!(view.as_field_value().as_bytes(), b"max-age=60");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_field_value(self) -> FieldValueRef<'a> {
        self.value
    }
}

impl StrictTransportSecurityBuilder {
    /// Adds `includeSubDomains`.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::builder(Duration::from_secs(60))
    ///     .include_subdomains()
    ///     .build()?;
    /// assert_eq!(
    ///     value.as_field_value().as_bytes(),
    ///     b"max-age=60; includeSubDomains"
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn include_subdomains(mut self) -> Self {
        self.include_subdomains = true;
        self
    }

    /// Adds the nonstandard `preload` directive.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::builder(Duration::from_secs(60))
    ///     .preload()
    ///     .build()?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"max-age=60; preload");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn preload(mut self) -> Self {
        self.preload = true;
        self
    }

    /// Adds an extension directive for validation by [`Self::build`].
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use http_headers::headers::{ExtensionValue, StrictTransportSecurityOwned};
    ///
    /// let value = StrictTransportSecurityOwned::builder(Duration::from_secs(60))
    ///     .extension("report", ExtensionValue::Value("audit"))
    ///     .build()?;
    /// assert_eq!(
    ///     value.as_field_value().as_bytes(),
    ///     b"max-age=60; report=audit"
    /// );
    ///
    /// // Reserved directive names are rejected.
    /// assert!(
    ///     StrictTransportSecurityOwned::builder(Duration::from_secs(60))
    ///         .extension("preload", ExtensionValue::Flag)
    ///         .build()
    ///         .is_err(),
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[must_use]
    pub fn extension(mut self, name: impl AsRef<str>, value: ExtensionValue<'_>) -> Self {
        let name = name.as_ref();
        let value = match value {
            ExtensionValue::Flag => None,
            ExtensionValue::Value(value) => Some(CompactString::from(value)),
        };
        self.extensions.push(BuilderExtension {
            name: CompactString::from(name),
            value,
        });
        self
    }

    /// Adds a flag-style extension directive.
    #[must_use]
    pub fn extension_flag(self, name: impl AsRef<str>) -> Self {
        self.extension(name, ExtensionValue::Flag)
    }

    /// Adds an extension directive carrying a token or quoted-string value.
    #[must_use]
    pub fn extension_value(self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        self.extension(name, ExtensionValue::Value(value.as_ref()))
    }

    /// Builds the header.
    ///
    /// # Errors
    ///
    /// Returns an error if an extension is malformed, uses a reserved name, or
    /// field-value construction fails.
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::builder(Duration::from_secs(31_536_000))
    ///     .include_subdomains()
    ///     .build()?;
    /// assert_eq!(value.max_age(), Duration::from_secs(31_536_000));
    /// assert!(value.include_subdomains());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn build(self) -> Result<StrictTransportSecurityOwned, DecodeError> {
        if self.extensions.iter().any(|extension| {
            !validate::token(extension.name.as_bytes())
                || known_hsts_name(extension.name.as_bytes())
                || extension
                    .value
                    .as_ref()
                    .is_some_and(|value| !valid_token_or_quoted(value.as_bytes()))
        }) {
            return Err(super::super::invalid_syntax(&FieldName::StrictTransportSecurity));
        }
        let capacity = "max-age=".len()
            + decimal_len(self.max_age.as_secs())
            + usize::from(self.include_subdomains) * "; includeSubDomains".len()
            + usize::from(self.preload) * "; preload".len()
            + self
                .extensions
                .iter()
                .map(|extension| 2 + extension.name.len() + extension.value.as_ref().map_or(0, |value| 1 + value.len()))
                .sum::<usize>();
        let mut wire = String::with_capacity(capacity);
        write!(wire, "max-age={}", self.max_age.as_secs()).expect("writing to a String is infallible");
        if self.include_subdomains {
            wire.push_str("; includeSubDomains");
        }
        if self.preload {
            wire.push_str("; preload");
        }
        for extension in self.extensions {
            wire.push_str("; ");
            wire.push_str(&extension.name);
            if let Some(value) = extension.value {
                wire.push('=');
                wire.push_str(&value);
            }
        }
        StrictTransportSecurityOwned::try_from(wire)
    }
}

const fn decimal_len(mut value: u64) -> usize {
    let mut length = 1;
    while value >= 10 {
        value /= 10;
        length += 1;
    }
    length
}

impl<'a> HstsDirectiveView<'a> {
    /// Returns the directive name.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::try_from("max-age=60; includeSubDomains")?;
    /// let directive = value
    ///     .directives()
    ///     .next()
    ///     .transpose()?
    ///     .expect("max-age is present");
    /// assert_eq!(directive.name(), "max-age");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn name(self) -> &'a str {
        self.name
    }

    /// Returns the raw token or quoted-string value.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::try_from("max-age=60; preload")?;
    /// let mut directives = value.directives();
    /// let max_age = directives.next().transpose()?.expect("max-age is present");
    /// assert_eq!(max_age.value(), Some(&b"60"[..]));
    ///
    /// // Valueless directives report `None`.
    /// let preload = directives.next().transpose()?.expect("preload is present");
    /// assert_eq!(preload.value(), None);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn value(self) -> Option<&'a [u8]> {
        self.value
    }

    /// Returns the optional value as UTF-8.
    ///
    /// # Errors
    ///
    /// Returns an error when a quoted value contains non-UTF-8 `obs-text`.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::try_from("max-age=60")?;
    /// let directive = value
    ///     .directives()
    ///     .next()
    ///     .transpose()?
    ///     .expect("max-age is present");
    /// assert_eq!(directive.value_str()?, Some("60"));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn value_str(self) -> Result<Option<&'a str>, DecodeError> {
        self.value
            .map(str::from_utf8)
            .transpose()
            .map_err(|_invalid| DecodeError::new(&FieldName::StrictTransportSecurity, DecodeErrorKind::InvalidUtf8))
    }

    /// Returns the complete directive bytes after surrounding OWS trimming.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::StrictTransportSecurityOwned;
    ///
    /// let value = StrictTransportSecurityOwned::try_from("max-age=60; includeSubDomains")?;
    /// let directive = value
    ///     .directives()
    ///     .nth(1)
    ///     .transpose()?
    ///     .expect("second directive");
    /// assert_eq!(directive.as_bytes(), b"includeSubDomains");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_bytes(self) -> &'a [u8] {
        self.raw
    }
}

impl SingleValueField for StrictTransportSecurity {
    type View<'a> = StrictTransportSecurityView<'a>;
    type Owned = StrictTransportSecurityOwned;

    fn name() -> &'static FieldName {
        &FieldName::StrictTransportSecurity
    }

    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        let summary = parse_hsts(value.as_bytes())?;
        Ok(StrictTransportSecurityView { value, summary })
    }

    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        StrictTransportSecurityOwned::try_from(value)
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.value
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.value
    }
}

impl TryFrom<&str> for StrictTransportSecurityOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = FieldValue::from_str(value).map_err(|_invalid| super::super::invalid_syntax(&FieldName::StrictTransportSecurity))?;
        Self::try_from(value)
    }
}

impl TryFrom<String> for StrictTransportSecurityOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = FieldValue::try_from(value).map_err(|_invalid| super::super::invalid_syntax(&FieldName::StrictTransportSecurity))?;
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for StrictTransportSecurityOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        let summary = parse_hsts(value.as_bytes())?;
        Ok(Self { value, summary })
    }
}

fn parse_hsts(bytes: &[u8]) -> Result<HstsSummary, DecodeError> {
    if bytes == b"max-age=31536000; includeSubDomains" {
        return Ok(HstsSummary {
            max_age: Duration::from_hours(8_760),
            include_subdomains: true,
            preload: false,
        });
    }
    if bytes == b"max-age=63072000; includeSubDomains; preload" {
        return Ok(HstsSummary {
            max_age: Duration::from_hours(17_520),
            include_subdomains: true,
            preload: true,
        });
    }
    if let Some(summary) = parse_canonical_hsts(bytes) {
        return Ok(summary);
    }
    parse_hsts_directives(bytes)
}

/// Parses the canonical serialization, returning `None` for anything else so
/// that [`parse_hsts_directives`] can apply the full grammar.
fn parse_canonical_hsts(bytes: &[u8]) -> Option<HstsSummary> {
    let digits = bytes.strip_prefix(b"max-age=")?;
    let mut max_age = 0_u64;
    let mut index = 0_usize;
    while let Some(digit) = digits.get(index).map(|byte| byte.wrapping_sub(b'0'))
        && digit <= 9
    {
        max_age = max_age.checked_mul(10)?.checked_add(u64::from(digit))?;
        index += 1;
    }
    if index == 0 {
        return None;
    }

    let mut rest = &digits[index..];
    let mut include_subdomains = false;
    let mut preload = false;
    loop {
        rest = trim_start_ows(rest);
        if rest.is_empty() {
            break;
        }
        rest = trim_start_ows(rest.strip_prefix(b";")?);
        if let Some(tail) = rest.strip_prefix(b"includeSubDomains") {
            if include_subdomains {
                return None;
            }
            include_subdomains = true;
            rest = tail;
        } else {
            let tail = rest.strip_prefix(b"preload")?;
            if preload {
                return None;
            }
            preload = true;
            rest = tail;
        }
    }

    Some(HstsSummary {
        max_age: Duration::from_secs(max_age),
        include_subdomains,
        preload,
    })
}

fn trim_start_ows(mut bytes: &[u8]) -> &[u8] {
    while let Some((first, rest)) = bytes.split_first() {
        if !matches!(first, b' ' | b'\t') {
            break;
        }
        bytes = rest;
    }
    bytes
}

fn parse_hsts_directives(bytes: &[u8]) -> Result<HstsSummary, DecodeError> {
    let mut max_age = None;
    let mut include_subdomains = false;
    let mut preload = false;
    for item in HstsItems::new(bytes) {
        let (name, value) = split_hsts_directive(item)?;
        if validate::eq_ignore_ascii_case(name, b"max-age") {
            if max_age.is_some() {
                return Err(super::super::invalid_syntax(&FieldName::StrictTransportSecurity));
            }
            let value = value.ok_or_else(|| super::super::invalid_syntax(&FieldName::StrictTransportSecurity))?;
            max_age = Some(parse_delta_seconds(value)?);
        } else if validate::eq_ignore_ascii_case(name, b"includesubdomains") {
            if include_subdomains || value.is_some() {
                return Err(super::super::invalid_syntax(&FieldName::StrictTransportSecurity));
            }
            include_subdomains = true;
        } else if validate::eq_ignore_ascii_case(name, b"preload") {
            if preload || value.is_some() {
                return Err(super::super::invalid_syntax(&FieldName::StrictTransportSecurity));
            }
            preload = true;
        }
    }
    Ok(HstsSummary {
        max_age: Duration::from_secs(max_age.ok_or_else(|| super::super::invalid_syntax(&FieldName::StrictTransportSecurity))?),
        include_subdomains,
        preload,
    })
}

fn split_hsts_directive(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>), DecodeError> {
    let bytes = super::super::trim_ows(bytes);
    let equals = bytes.iter().position(|byte| *byte == b'=');
    let (name, value) = equals.map_or((bytes, None), |equals| (&bytes[..equals], Some(&bytes[equals + 1..])));
    if !validate::token(name) || value.is_some_and(|value| !valid_token_or_quoted(value)) {
        return Err(super::super::invalid_syntax(&FieldName::StrictTransportSecurity));
    }
    Ok((name, value))
}

fn parse_hsts_directive(bytes: &[u8]) -> Result<HstsDirectiveView<'_>, DecodeError> {
    let bytes = super::super::trim_ows(bytes);
    let (name, value) = split_hsts_directive(bytes)?;
    let name = str::from_utf8(name).expect("HTTP token validation guarantees ASCII");
    Ok(HstsDirectiveView { raw: bytes, name, value })
}

fn parse_delta_seconds(bytes: &[u8]) -> Result<u64, DecodeError> {
    if bytes.is_empty() {
        return Err(DecodeError::new(
            &FieldName::StrictTransportSecurity,
            DecodeErrorKind::InvalidNumber,
        ));
    }
    if bytes.first() != Some(&b'"') {
        return validate::decimal_u64(bytes)
            .ok_or_else(|| DecodeError::new(&FieldName::StrictTransportSecurity, DecodeErrorKind::InvalidNumber));
    }
    let inner = bytes
        .strip_prefix(b"\"")
        .and_then(|value| value.strip_suffix(b"\""))
        .ok_or_else(|| DecodeError::new(&FieldName::StrictTransportSecurity, DecodeErrorKind::InvalidNumber))?;
    let mut value = 0_u64;
    let mut digits = 0_usize;
    let mut index = 0;
    while index < inner.len() {
        let byte = if inner[index] == b'\\' {
            index += 1;
            *inner
                .get(index)
                .ok_or_else(|| DecodeError::new(&FieldName::StrictTransportSecurity, DecodeErrorKind::InvalidNumber))?
        } else {
            inner[index]
        };
        if !byte.is_ascii_digit() {
            return Err(DecodeError::new(
                &FieldName::StrictTransportSecurity,
                DecodeErrorKind::InvalidNumber,
            ));
        }
        value = value
            .checked_mul(10)
            .and_then(|current| current.checked_add(u64::from(byte - b'0')))
            .ok_or_else(|| DecodeError::new(&FieldName::StrictTransportSecurity, DecodeErrorKind::InvalidNumber))?;
        digits += 1;
        index += 1;
    }
    if digits == 0 {
        return Err(DecodeError::new(
            &FieldName::StrictTransportSecurity,
            DecodeErrorKind::InvalidNumber,
        ));
    }
    Ok(value)
}

fn known_hsts_name(name: &[u8]) -> bool {
    validate::eq_ignore_ascii_case(name, b"max-age")
        || validate::eq_ignore_ascii_case(name, b"includesubdomains")
        || validate::eq_ignore_ascii_case(name, b"preload")
}

fn valid_token_or_quoted(bytes: &[u8]) -> bool {
    validate::token(bytes) || valid_quoted_string(bytes)
}

fn valid_quoted_string(bytes: &[u8]) -> bool {
    if bytes.len() < 2 || bytes.first() != Some(&b'"') || bytes.last() != Some(&b'"') {
        return false;
    }
    let mut escaped = false;
    for byte in &bytes[1..bytes.len() - 1] {
        if escaped {
            if !matches!(byte, b'\t' | b' '..=b'~' | 0x80..=0xff) {
                return false;
            }
            escaped = false;
        } else if *byte == b'\\' {
            escaped = true;
        } else if !matches!(byte, b'\t' | b' ' | b'!' | b'#'..=b'[' | b']'..=b'~' | 0x80..=0xff) {
            return false;
        }
    }
    !escaped
}

struct HstsItems<'a> {
    bytes: &'a [u8],
    start: usize,
    position: usize,
    finished: bool,
}

impl<'a> HstsItems<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            start: 0,
            position: 0,
            finished: false,
        }
    }
}

impl<'a> Iterator for HstsItems<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        while !self.finished {
            let mut quoted = false;
            let mut escaped = false;
            while let Some(byte) = self.bytes.get(self.position).copied() {
                if escaped {
                    escaped = false;
                } else if quoted && byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quoted = !quoted;
                } else if !quoted && byte == b';' {
                    let item = super::super::trim_ows(&self.bytes[self.start..self.position]);
                    self.position += 1;
                    self.start = self.position;
                    if item.is_empty() {
                        continue;
                    }
                    return Some(item);
                }
                self.position += 1;
            }
            self.finished = true;
            let item = super::super::trim_ows(&self.bytes[self.start..]);
            if !item.is_empty() {
                return Some(item);
            }
        }
        None
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use super::{
        HstsItems, StrictTransportSecurity, StrictTransportSecurityOwned, decimal_len, parse_delta_seconds, parse_hsts,
        parse_hsts_directive, split_hsts_directive, valid_quoted_string,
    };
    use crate::sink::{EncodedValues, FieldSink};
    use crate::{DecodeErrorKind, Field, FieldValue, FieldValueRef, TestSink};

    #[test]
    fn hsts_builder_keeps_short_extensions_inline_and_spills_long_ones() {
        let short = StrictTransportSecurityOwned::builder(Duration::from_mins(1)).extension_value("x-mode", "stable");
        let long = StrictTransportSecurityOwned::builder(Duration::from_mins(1))
            .extension_value("extension-directive-with-a-value-that-exceeds-inline-storage", "enabled");
        assert!(!short.extensions[0].name.is_heap_allocated());
        assert!(long.extensions[0].name.is_heap_allocated());
    }

    #[test]
    fn hsts_canonical_fast_path_agrees_with_the_general_parser() {
        let inputs: &[&[u8]] = &[
            b"max-age=0",
            b"max-age=31536000",
            b"max-age=31536000; includeSubDomains",
            b"max-age=31536000; includeSubDomains; preload",
            b"max-age=60;preload",
            b"max-age=60 ; includeSubDomains",
            b"max-age=60\t;\tpreload ",
            b"max-age=18446744073709551615",
            b"max-age=18446744073709551616",
            b"max-age=184467440737095516150",
            b"max-age=",
            b"max-age=60; includeSubDomains; includeSubDomains",
            b"max-age=60; preload; preload",
            b"max-age=60; preload=yes",
            b"max-age=60; unknown",
            b"max-age=60;;preload",
            b"max-age=60; includeSubDomainsX",
            b"MAX-AGE=60; includeSubDomains",
            b"max-age=\"60\"",
            b"includeSubDomains; max-age=60",
            b"",
        ];
        for input in inputs {
            if let Some(summary) = super::parse_canonical_hsts(input) {
                assert_eq!(
                    super::parse_hsts_directives(input),
                    Ok(summary),
                    "fast path disagreed for {input:?}"
                );
            }
            assert_eq!(
                super::parse_hsts(input).ok(),
                super::parse_hsts_directives(input).ok(),
                "dispatch disagreed for {input:?}"
            );
        }
    }

    #[test]
    fn builder_accessors_directives_and_header_round_trip() {
        let value = StrictTransportSecurityOwned::builder(Duration::from_hours(8_760))
            .include_subdomains()
            .preload()
            .extension_value("report-to", "\"hsts\"")
            .extension_flag("flag")
            .build()
            .expect("header builds");
        assert_eq!(value.max_age(), Duration::from_hours(8_760));
        assert!(value.include_subdomains());
        assert!(value.preload());
        assert_eq!(
            value.as_field_value().as_bytes(),
            b"max-age=31536000; includeSubDomains; preload; report-to=\"hsts\"; flag"
        );

        let directives = value.directives().collect::<Result<Vec<_>, _>>().expect("built directives parse");
        assert_eq!(directives[0].name(), "max-age");
        assert_eq!(directives[0].value(), Some(b"31536000".as_slice()));
        assert_eq!(directives[0].value_str(), Ok(Some("31536000")));
        assert_eq!(directives[0].as_bytes(), b"max-age=31536000");
        assert_eq!(directives[4].name(), "flag");
        assert_eq!(directives[4].value(), None);
        assert_eq!(directives[4].value_str(), Ok(None));
        assert_eq!(value.clone().into_field_value().as_bytes(), value.as_field_value().as_bytes());

        let mut table = TestSink::new();
        assert!(StrictTransportSecurity::view(&table).expect("absent header succeeds").is_none());
        StrictTransportSecurity::insert(&mut table, value).expect("header inserts");
        let view = StrictTransportSecurity::view(&table)
            .expect("view decodes")
            .expect("header is present");
        assert_eq!(view.max_age(), Duration::from_hours(8_760));
        assert!(view.include_subdomains());
        assert!(view.preload());
        assert_eq!(
            view.as_field_value().as_bytes(),
            b"max-age=31536000; includeSubDomains; preload; report-to=\"hsts\"; flag"
        );
        assert_eq!(view.directives().count(), 5);
        let owned = StrictTransportSecurity::owned(&table)
            .expect("owned value decodes")
            .expect("header is present");
        assert_eq!(owned.max_age(), Duration::from_hours(8_760));
    }

    #[test]
    fn constructors_and_conversions_accept_general_grammar() {
        let simple = StrictTransportSecurityOwned::new(Duration::from_mins(1)).expect("header builds");
        assert_eq!(simple.as_field_value().as_bytes(), b"max-age=60");
        assert_eq!(
            <StrictTransportSecurity as crate::SingleValueField>::as_field_value(&simple).as_bytes(),
            b"max-age=60"
        );
        assert_eq!(
            <StrictTransportSecurity as crate::SingleValueField>::into_field_value(simple).as_bytes(),
            b"max-age=60"
        );
        assert_eq!(
            <StrictTransportSecurity as crate::SingleValueField>::decode_owned(FieldValue::from_static("max-age=60"),)
                .expect("direct owned decode")
                .max_age(),
            Duration::from_mins(1)
        );
        assert_eq!(
            <StrictTransportSecurity as crate::SingleValueField>::decode_view(FieldValueRef::new(b"max-age=x"),)
                .expect_err("invalid borrowed value")
                .kind(),
            DecodeErrorKind::InvalidNumber
        );

        for value in [
            StrictTransportSecurityOwned::try_from("MAX-AGE=\"6\\0\"; includeSubDomains; preload; future=\"a,b\""),
            StrictTransportSecurityOwned::try_from(String::from("MAX-AGE=\"6\\0\"; includeSubDomains; preload; future=\"a,b\"")),
            StrictTransportSecurityOwned::try_from(
                FieldValue::from_str("MAX-AGE=\"6\\0\"; includeSubDomains; preload; future=\"a,b\"").expect("safe field value"),
            ),
        ] {
            let value = value.expect("general HSTS grammar is valid");
            assert_eq!(value.max_age(), Duration::from_mins(1));
            assert!(value.include_subdomains());
            assert!(value.preload());
        }

        assert_eq!(
            StrictTransportSecurityOwned::try_from(String::from("max-age=1\n"))
                .expect_err("invalid field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            StrictTransportSecurityOwned::try_from("max-age=1\n")
                .expect_err("invalid borrowed field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            StrictTransportSecurityOwned::try_from(FieldValue::from_static("max-age=x"))
                .expect_err("invalid stored value")
                .kind(),
            DecodeErrorKind::InvalidNumber
        );
        assert_eq!(
            parse_hsts(b"max-age=63072000; includeSubDomains; preload")
                .expect("common preload value")
                .max_age,
            Duration::from_hours(17_520)
        );
    }

    #[test]
    fn builder_rejects_reserved_and_invalid_extensions() {
        for (name, value) in [
            ("max-age", None),
            ("IncludeSubDomains", None),
            ("preload", None),
            ("bad name", None),
            ("future", Some("bad value")),
            ("future", Some("\"unterminated")),
        ] {
            let builder = StrictTransportSecurityOwned::builder(Duration::from_secs(1));
            let builder = match value {
                Some(value) => builder.extension_value(name, value),
                None => builder.extension_flag(name),
            };
            assert_eq!(
                builder.build().expect_err("invalid extension").kind(),
                DecodeErrorKind::InvalidSyntax
            );
        }
    }

    #[test]
    fn parser_reports_duplicate_missing_and_malformed_directives() {
        let invalid = [
            b"".as_slice(),
            b"includeSubDomains",
            b"max-age",
            b"max-age=",
            b"max-age=x",
            b"max-age=1; max-age=2",
            b"max-age=1; includeSubDomains; includeSubDomains",
            b"max-age=1; includeSubDomains=yes",
            b"max-age=1; preload; preload",
            b"max-age=1; preload=yes",
            b"max-age=1; =value",
            b"max-age=1; future=\"unterminated",
            b"max-age=18446744073709551616",
            b"max-age=\"\"",
            b"max-age=\"x\"",
            b"max-age=\"1\\\"",
            b"max-age=\"18446744073709551616\"",
        ];
        for wire in invalid {
            let _error = parse_hsts(wire).expect_err("malformed HSTS must fail");
        }
        assert_eq!(
            parse_delta_seconds(b"").expect_err("empty number").kind(),
            DecodeErrorKind::InvalidNumber
        );
        assert_eq!(
            parse_delta_seconds(b"\"12").expect_err("unterminated number").kind(),
            DecodeErrorKind::InvalidNumber
        );
        assert_eq!(
            parse_delta_seconds(b"\"1\\\"").expect_err("trailing quoted escape").kind(),
            DecodeErrorKind::InvalidNumber
        );
    }

    #[test]
    fn directive_helpers_handle_quoted_delimiters_obs_text_and_invalid_utf8_values() {
        assert_eq!(
            HstsItems::new(b"max-age=1; future=\"a;b\";; preload").collect::<Vec<_>>(),
            [b"max-age=1".as_slice(), b"future=\"a;b\"".as_slice(), b"preload".as_slice(),]
        );
        assert_eq!(
            HstsItems::new(b"max-age=1; future=\"a\\\";b\"; preload").collect::<Vec<_>>(),
            [b"max-age=1".as_slice(), b"future=\"a\\\";b\"".as_slice(), b"preload".as_slice(),]
        );
        assert_eq!(
            split_hsts_directive(b" future=token ").expect("directive parses"),
            (b"future".as_slice(), Some(b"token".as_slice()))
        );
        assert!(valid_quoted_string(b"\"quoted\\\"value\""));
        assert!(valid_quoted_string(b"\"\xff\""));
        assert!(valid_quoted_string(b"\"\\\xff\""));
        assert!(!valid_quoted_string(b"\"trailing\\\""));
        assert!(!valid_quoted_string(b"\"line\nbreak\""));
        assert!(!valid_quoted_string(b"\"escaped\\\ncontrol\""));
        assert_eq!(
            parse_hsts_directive(b"bad name").expect_err("invalid directive").kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let raw = b"future=\"\xff\"";
        let directive = parse_hsts_directive(raw).expect("obs-text value parses");
        assert_eq!(directive.name(), "future");
        assert_eq!(directive.value(), Some(b"\"\xff\"".as_slice()));
        assert_eq!(
            directive.value_str().expect_err("obs-text is not UTF-8").kind(),
            DecodeErrorKind::InvalidUtf8
        );
    }

    #[test]
    fn singleton_decoding_rejects_multiple_values_and_decimal_len_counts_digits() {
        let mut table = TestSink::new();
        table
            .set_values(
                StrictTransportSecurity::name(),
                EncodedValues::from_vec(vec![FieldValue::from_static("max-age=1"), FieldValue::from_static("max-age=2")]),
            )
            .expect("table accepts raw values");
        assert_eq!(
            StrictTransportSecurity::view(&table)
                .expect_err("singleton view rejects duplicates")
                .kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        assert_eq!(decimal_len(0), 1);
        assert_eq!(decimal_len(9), 1);
        assert_eq!(decimal_len(10), 2);
        assert_eq!(decimal_len(u64::MAX), 20);
    }
}
