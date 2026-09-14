// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `Cache-Control` parsing, summaries, and construction.

use std::fmt::Write as _;
use std::time::Duration;
use std::{fmt, str};

use compact_str::CompactString;
use smallvec::SmallVec;

use super::ExtensionValue;
use crate::sink::{EncodedValues, FieldEncodeOutput, FieldEncoder, FieldSensitivity, FieldSink, FieldValueWriter, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValue, FieldValueRef, validate};

/// Defines the `Cache-Control` header.
///
/// # Specification
///
/// Defined by [RFC 9111 section 5.2](https://www.rfc-editor.org/rfc/rfc9111#section-5.2).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{CacheControl, CacheControlOwned};
///
/// let mut map = HeaderMap::new();
/// CacheControl::insert(
///     &mut map,
///     CacheControlOwned::try_from("max-age=60, no-cache")?,
/// )?;
/// assert!(CacheControl::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct CacheControl {
    _private: (),
}

/// Owned value for the `Cache-Control` header.
///
/// # Specification
///
/// Defined by [RFC 9111 section 5.2].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::CacheControlOwned::try_from("max-age=60, no-cache")?;
/// assert_eq!(value.max_age(), Some(std::time::Duration::from_secs(60)));
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Cache-Control: no-cache` prevents reuse without validation.
/// `Cache-Control: public, max-age=86400, stale-while-revalidate=60` combines
/// standard and extension directives.
///
/// [RFC 9111 section 5.2]: https://www.rfc-editor.org/rfc/rfc9111#section-5.2
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct CacheControlOwned {
    values: SmallVec<[FieldValue; 1]>,
    summary: CacheSummary,
}

/// Borrowed value for the `Cache-Control` header.
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), http_headers::DecodeError> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{CacheControl, CacheControlView};
///
/// let mut map = HeaderMap::new();
/// map.insert(
///     http::header::CACHE_CONTROL,
///     http::HeaderValue::from_static("public, max-age=31536000, immutable"),
/// );
/// let view: CacheControlView<'_> = CacheControl::view(&map)?.expect("header present");
/// assert_eq!(
///     view.max_age(),
///     Some(std::time::Duration::from_secs(31536000))
/// );
/// # Ok::<(), http_headers::DecodeError>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
pub struct CacheControlView<'a> {
    values: FieldLines<'a>,
    summary: CacheSummary,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct CacheSummary {
    no_cache: bool,
    max_age: Option<Duration>,
}

impl fmt::Debug for CacheControlOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CacheControlOwned")
            .field("value_count", &self.values.len())
            .field("summary", &self.summary)
            .finish()
    }
}

impl fmt::Debug for CacheControlView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CacheControlView")
            .field("value_count", &self.values.len())
            .field("summary", &self.summary)
            .finish()
    }
}

/// One borrowed cache directive.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{CacheControlOwned, CacheDirectiveView};
///
/// let value = CacheControlOwned::try_from("s-maxage=120")?;
/// let directive: CacheDirectiveView<'_> = value.directives().next().expect("one directive");
/// assert_eq!(directive.name(), "s-maxage");
/// assert_eq!(directive.value(), Some(b"120".as_slice()));
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct CacheDirectiveView<'a> {
    name: &'a str,
    value: Option<&'a [u8]>,
    raw: &'a [u8],
}

/// Builder for a canonical `Cache-Control` field value.
#[derive(Clone, Debug, Default)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{CacheControlBuilder, CacheControlOwned};
///
/// let builder: CacheControlBuilder = CacheControlOwned::builder()
///     .public()
///     .max_age(std::time::Duration::from_secs(31536000))
///     .immutable();
/// let value = builder.build()?;
/// assert_eq!(
///     value.max_age(),
///     Some(std::time::Duration::from_secs(31536000))
/// );
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct CacheControlBuilder {
    directives: SmallVec<[BuilderDirective; 4]>,
}

#[derive(Clone, Debug)]
enum BuilderDirective {
    Static(&'static str),
    MaxAge(Duration),
    Extension { name: CompactString, value: Option<CompactString> },
}

impl CacheControlOwned {
    #[cfg(all(feature = "serde", feature = "headers-cache-control"))]
    pub(crate) fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'_>> + '_ {
        self.values.iter().map(FieldValue::as_field_value_ref)
    }

    pub(crate) fn encoded_value(&self) -> Option<FieldValue> {
        super::normalized_comma_value(&FieldName::CacheControl, self.directives().map(CacheDirectiveView::as_bytes))
            .expect("validated cache directives remain valid when comma-joined")
    }

    /// Creates a builder.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::CacheControlOwned;
    ///
    /// let value = CacheControlOwned::builder()
    ///     .public()
    ///     .max_age(std::time::Duration::from_secs(60))
    ///     .build()?;
    /// assert_eq!(value.max_age(), Some(std::time::Duration::from_secs(60)));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn builder() -> CacheControlBuilder {
        CacheControlBuilder {
            directives: SmallVec::new_const(),
        }
    }

    /// Iterates all directives in wire order.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::CacheControlOwned;
    ///
    /// let value = CacheControlOwned::try_from("public, max-age=31536000, immutable")?;
    /// let names: Vec<_> = value
    ///     .directives()
    ///     .map(|directive| directive.name())
    ///     .collect();
    /// assert_eq!(names, ["public", "max-age", "immutable"]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn directives(&self) -> impl Iterator<Item = CacheDirectiveView<'_>> {
        self.values
            .iter()
            .flat_map(|value| DirectiveItems::new(value.as_bytes(), true))
            .filter_map(Result::ok)
            .filter_map(|item| parse_directive(item).ok())
    }

    /// Returns whether a `no-cache` directive is present.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::CacheControlOwned;
    ///
    /// let value = CacheControlOwned::try_from("no-cache")?;
    /// assert!(value.no_cache());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn no_cache(&self) -> bool {
        self.summary.no_cache
    }

    /// Returns the first valid `max-age` value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::CacheControlOwned::try_from("max-age=60")?;
    /// assert_eq!(value.max_age(), Some(std::time::Duration::from_secs(60)));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn max_age(&self) -> Option<Duration> {
        self.summary.max_age
    }
}

impl<'a> CacheControlView<'a> {
    pub(crate) fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'a>> + '_ {
        self.values.repeated()
    }

    /// Iterates all directives in wire order.
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), http_headers::DecodeError> {
    /// use http::HeaderMap;
    /// use http_headers::Field;
    /// use http_headers::headers::CacheControl;
    ///
    /// let mut map = HeaderMap::new();
    /// map.insert(
    ///     http::header::CACHE_CONTROL,
    ///     http::HeaderValue::from_static("s-maxage=120, no-store"),
    /// );
    /// let view = CacheControl::view(&map)?.expect("header present");
    /// let names: Vec<_> = view
    ///     .directives()
    ///     .map(|directive| directive.name())
    ///     .collect();
    /// assert_eq!(names, ["s-maxage", "no-store"]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub fn directives(&self) -> impl Iterator<Item = CacheDirectiveView<'a>> + '_ {
        self.values
            .comma_items()
            .filter_map(Result::ok)
            .filter_map(|item| parse_directive(item).ok())
    }

    /// Returns whether a `no-cache` directive is present.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), http_headers::DecodeError> {
    /// use http::HeaderMap;
    /// use http_headers::Field;
    /// use http_headers::headers::CacheControl;
    ///
    /// let mut map = HeaderMap::new();
    /// map.insert(
    ///     http::header::CACHE_CONTROL,
    ///     http::HeaderValue::from_static("no-cache, no-store"),
    /// );
    /// let view = CacheControl::view(&map)?.expect("header present");
    /// assert!(view.no_cache());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub fn no_cache(&self) -> bool {
        self.summary.no_cache
    }

    /// Returns the first valid `max-age` value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::CacheControlOwned::try_from("max-age=60")?;
    /// assert_eq!(value.max_age(), Some(std::time::Duration::from_secs(60)));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn max_age(&self) -> Option<Duration> {
        self.summary.max_age
    }
}

impl<'a> CacheDirectiveView<'a> {
    /// Returns the directive name.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::CacheControlOwned;
    ///
    /// let value = CacheControlOwned::try_from("private, max-age=60")?;
    /// let directive = value.directives().next().expect("one directive");
    /// assert_eq!(directive.name(), "private");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn name(self) -> &'a str {
        self.name
    }

    /// Returns the optional raw token or quoted-string value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::CacheControlOwned::try_from("max-age=60")?;
    /// assert_eq!(value.max_age(), Some(std::time::Duration::from_secs(60)));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn value(self) -> Option<&'a [u8]> {
        self.value
    }

    /// Returns the optional value as UTF-8.
    ///
    /// # Errors
    ///
    /// Returns an error when a quoted string contains non-UTF-8 `obs-text`.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::CacheControlOwned;
    ///
    /// let value = CacheControlOwned::try_from("s-maxage=120")?;
    /// let directive = value.directives().next().expect("one directive");
    /// assert_eq!(directive.value_str()?, Some("120"));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn value_str(self) -> Result<Option<&'a str>, DecodeError> {
        self.value
            .map(str::from_utf8)
            .transpose()
            .map_err(|_invalid| DecodeError::new(&FieldName::CacheControl, DecodeErrorKind::InvalidUtf8))
    }

    /// Returns the complete directive bytes after surrounding OWS trimming.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::CacheControlOwned;
    ///
    /// let value = CacheControlOwned::try_from("public, max-age=31536000, immutable")?;
    /// let directive = value.directives().nth(1).expect("max-age directive");
    /// assert_eq!(directive.as_bytes(), b"max-age=31536000");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_bytes(self) -> &'a [u8] {
        self.raw
    }
}

impl Field for CacheControl {
    type View<'a> = CacheControlView<'a>;
    type Owned = CacheControlOwned;

    fn name() -> &'static FieldName {
        &FieldName::CacheControl
    }

    fn view_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_list_item_limit(b',', true)?;
        let mut summary = CacheSummary::default();
        for (value_index, value) in lines.repeated().enumerate() {
            observe_directives(value.as_bytes(), value_index, &mut summary)?;
        }
        Ok(Some(CacheControlView { values: lines, summary }))
    }

    fn owned_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_list_item_limit(b',', true)?;
        let mut summary = CacheSummary::default();
        let mut copied = SmallVec::new();
        for (value_index, (value, owned)) in lines.repeated_owned()?.enumerate() {
            observe_directives(value.as_bytes(), value_index, &mut summary)?;
            copied.push(owned);
        }
        Ok(Some(CacheControlOwned { values: copied, summary }))
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        let encoded = value.encoded_value().map_or_else(EncodedValues::new, EncodedValues::single);
        sink.set_values(Self::name(), encoded)
    }
}

fn observe_directives(bytes: &[u8], value_index: usize, summary: &mut CacheSummary) -> Result<(), DecodeError> {
    // Quoted directive values are rare, and the prescan for them lowers to a
    // vectorised byte search, so it costs far less than watching every byte of
    // the split for a quote would.
    if bytes.contains(&b'"') {
        return observe_quoted_directives(bytes, value_index, summary);
    }
    for item in bytes.split(|byte| *byte == b',') {
        let item = super::trim_ows(item);
        if !item.is_empty() {
            summary.observe(parse_directive_parts(item)?);
        }
    }
    Ok(())
}

/// Observes a line that carries quoted directive values.
#[cold]
#[inline(never)]
fn observe_quoted_directives(bytes: &[u8], value_index: usize, summary: &mut CacheSummary) -> Result<(), DecodeError> {
    for item in DirectiveItems::new(bytes, true) {
        let item = item.map_err(|error| error.at_value(value_index))?;
        summary.observe(parse_directive_parts(item)?);
    }
    Ok(())
}

impl TryFrom<&str> for CacheControlOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let header = FieldValue::from_str(value).map_err(|_invalid| super::invalid_syntax(&FieldName::CacheControl))?;
        let summary = validate_outgoing(header.as_field_value_ref())?;
        Ok(Self {
            values: SmallVec::from_buf([header]),
            summary,
        })
    }
}

impl TryFrom<String> for CacheControlOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let header = FieldValue::try_from(value).map_err(|_invalid| super::invalid_syntax(&FieldName::CacheControl))?;
        let summary = validate_outgoing(header.as_field_value_ref())?;
        Ok(Self {
            values: SmallVec::from_buf([header]),
            summary,
        })
    }
}

impl TryFrom<FieldValue> for CacheControlOwned {
    type Error = DecodeError;

    fn try_from(header: FieldValue) -> Result<Self, Self::Error> {
        let summary = validate_outgoing(header.as_field_value_ref())?;
        Ok(Self {
            values: SmallVec::from_buf([header]),
            summary,
        })
    }
}

fn validate_outgoing(header: FieldValueRef<'_>) -> Result<CacheSummary, DecodeError> {
    let mut summary = CacheSummary::default();
    for item in DirectiveItems::new(header.as_bytes(), false) {
        let item = item?;
        if item.is_empty() {
            return Err(super::invalid_syntax(&FieldName::CacheControl));
        }
        summary.observe(parse_directive_parts(item)?);
    }
    Ok(summary)
}

impl CacheSummary {
    fn observe(&mut self, directive: DirectiveParts<'_>) {
        match directive.kind {
            DirectiveKind::NoCache => self.no_cache = true,
            DirectiveKind::MaxAge if self.max_age.is_none() => {
                self.max_age = directive.delta_seconds.map(Duration::from_secs);
            }
            DirectiveKind::MaxAge | DirectiveKind::Other => {}
        }
    }
}

impl CacheControlBuilder {
    /// Adds `public`.
    #[must_use]
    pub fn public(mut self) -> Self {
        self.directives.push(BuilderDirective::Static("public"));
        self
    }

    /// Adds `no-cache`.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::CacheControlOwned;
    ///
    /// let value = CacheControlOwned::builder().no_cache().build()?;
    /// assert!(value.no_cache());
    /// assert_eq!(
    ///     value.directives().next().expect("one directive").name(),
    ///     "no-cache"
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn no_cache(mut self) -> Self {
        self.directives.push(BuilderDirective::Static("no-cache"));
        self
    }

    /// Adds `private`.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::CacheControlOwned;
    ///
    /// let value = CacheControlOwned::builder().private().build()?;
    /// let directive = value.directives().next().expect("one directive");
    /// assert_eq!(directive.name(), "private");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn private(mut self) -> Self {
        self.directives.push(BuilderDirective::Static("private"));
        self
    }

    /// Adds `no-store`.
    #[must_use]
    pub fn no_store(mut self) -> Self {
        self.directives.push(BuilderDirective::Static("no-store"));
        self
    }

    /// Adds `must-revalidate`.
    #[must_use]
    pub fn must_revalidate(mut self) -> Self {
        self.directives.push(BuilderDirective::Static("must-revalidate"));
        self
    }

    /// Adds `immutable`.
    #[must_use]
    pub fn immutable(mut self) -> Self {
        self.directives.push(BuilderDirective::Static("immutable"));
        self
    }

    /// Adds `max-age`.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::CacheControlOwned::try_from("max-age=60")?;
    /// assert_eq!(value.max_age(), Some(std::time::Duration::from_secs(60)));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn max_age(mut self, duration: Duration) -> Self {
        self.directives
            .push(BuilderDirective::MaxAge(Duration::from_secs(duration.as_secs())));
        self
    }

    /// Adds an extension directive for validation by [`Self::build`].
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{CacheControlOwned, ExtensionValue};
    ///
    /// let value = CacheControlOwned::builder()
    ///     .extension("s-maxage", ExtensionValue::Value("120"))
    ///     .build()?;
    /// let directive = value.directives().next().expect("one directive");
    /// assert_eq!(directive.name(), "s-maxage");
    /// assert_eq!(directive.value_str()?, Some("120"));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[must_use]
    pub fn extension(mut self, name: impl AsRef<str>, value: ExtensionValue<'_>) -> Self {
        let name = name.as_ref();
        let value = match value {
            ExtensionValue::Flag => None,
            ExtensionValue::Value(value) => Some(CompactString::from(value)),
        };
        self.directives.push(BuilderDirective::Extension {
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

    /// Builds a nonempty header.
    ///
    /// # Errors
    ///
    /// Returns an error when no directives were added or any pending extension
    /// is malformed.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::CacheControlOwned;
    ///
    /// let value = CacheControlOwned::builder()
    ///     .public()
    ///     .max_age(std::time::Duration::from_secs(31536000))
    ///     .immutable()
    ///     .build()?;
    /// let names: Vec<_> = value
    ///     .directives()
    ///     .map(|directive| directive.name())
    ///     .collect();
    /// assert_eq!(names, ["public", "max-age", "immutable"]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn build(self) -> Result<CacheControlOwned, DecodeError> {
        if self.directives.is_empty() {
            return Err(DecodeError::new(&FieldName::CacheControl, DecodeErrorKind::InvalidSyntax));
        }
        if self.directives.iter().any(|directive| {
            let BuilderDirective::Extension { name, value } = directive else {
                return false;
            };
            !validate::token(name.as_bytes()) || value.as_ref().is_some_and(|value| !valid_directive_value(value.as_bytes()))
        }) {
            return Err(super::invalid_syntax(&FieldName::CacheControl));
        }
        let directives = self.directives;
        builder_wire_len(directives.iter().map(BuilderDirective::wire_len), directives.len()).and_then(|capacity| {
            let mut wire = String::with_capacity(capacity);
            for (index, directive) in directives.into_iter().enumerate() {
                if index != 0 {
                    wire.push_str(", ");
                }
                directive.write_to(&mut wire);
            }
            CacheControlOwned::try_from(wire)
        })
    }
}

impl CacheControl {
    /// Starts an allocation-free response plan with `public`.
    #[must_use]
    pub fn public() -> CacheControlBuilder {
        CacheControlOwned::builder().public()
    }

    /// Starts an allocation-free response plan with `private`.
    #[must_use]
    pub fn private() -> CacheControlBuilder {
        CacheControlOwned::builder().private()
    }

    /// Starts an allocation-free response plan with `no-cache`.
    #[must_use]
    pub fn no_cache() -> CacheControlBuilder {
        CacheControlOwned::builder().no_cache()
    }
}

impl FieldEncoder for CacheControlBuilder {
    fn encode<O>(self, output: &mut O) -> Result<(), InsertError>
    where
        O: FieldEncodeOutput,
    {
        if self.directives.is_empty() {
            return Err(InsertError);
        }
        if self.directives.iter().any(|directive| {
            let BuilderDirective::Extension { name, value } = directive else {
                return false;
            };
            !validate::token(name.as_bytes()) || value.as_ref().is_some_and(|value| !valid_directive_value(value.as_bytes()))
        }) {
            return Err(InsertError);
        }
        let length = self.directives.iter().map(BuilderDirective::wire_len).sum::<usize>() + self.directives.len().saturating_sub(1) * 2;
        let mut writer = output.begin_value(length, FieldSensitivity::NonSensitive)?;
        for (index, directive) in self.directives.into_iter().enumerate() {
            if index != 0 {
                writer.write_bytes(b", ")?;
            }
            match directive {
                BuilderDirective::Static(value) => writer.write_bytes(value.as_bytes())?,
                BuilderDirective::MaxAge(duration) => {
                    writer.write_bytes(b"max-age=")?;
                    write_decimal(&mut writer, duration.as_secs())?;
                }
                BuilderDirective::Extension { name, value } => {
                    writer.write_bytes(name.as_bytes())?;
                    if let Some(value) = value {
                        writer.write_bytes(b"=")?;
                        writer.write_bytes(value.as_bytes())?;
                    }
                }
            }
        }
        writer.finish()
    }
}

fn write_decimal<W>(writer: &mut W, mut value: u64) -> Result<(), InsertError>
where
    W: FieldValueWriter,
{
    let mut storage = [0_u8; 20];
    let mut start = storage.len();
    loop {
        start -= 1;
        storage[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    writer.write_bytes(&storage[start..])
}

impl BuilderDirective {
    fn wire_len(&self) -> usize {
        match self {
            Self::Static(value) => value.len(),
            Self::Extension { name, value } => name.len() + value.as_ref().map_or(0, |value| 1 + value.len()),
            Self::MaxAge(value) => "max-age=".len() + decimal_len(value.as_secs()),
        }
    }

    fn write_to(self, wire: &mut String) {
        match self {
            Self::Static(value) => wire.push_str(value),
            Self::Extension { name, value } => {
                wire.push_str(&name);
                if let Some(value) = value {
                    wire.push('=');
                    wire.push_str(&value);
                }
            }
            Self::MaxAge(value) => {
                write!(wire, "max-age={}", value.as_secs()).expect("formatting into a String cannot fail");
            }
        }
    }
}

#[inline]
fn builder_wire_len(lengths: impl IntoIterator<Item = usize>, directive_count: usize) -> Result<usize, DecodeError> {
    let content_len = lengths
        .into_iter()
        .try_fold(0_usize, usize::checked_add)
        .ok_or_else(cache_size_error)?;
    let separators = directive_count.saturating_sub(1).checked_mul(2).ok_or_else(cache_size_error)?;
    content_len.checked_add(separators).ok_or_else(cache_size_error)
}

#[cold]
fn cache_size_error() -> DecodeError {
    DecodeError::new(&FieldName::CacheControl, DecodeErrorKind::InvalidNumber)
}

const fn decimal_len(mut value: u64) -> usize {
    let mut length = 1;
    while value >= 10 {
        value /= 10;
        length += 1;
    }
    length
}

fn parse_directive(bytes: &[u8]) -> Result<CacheDirectiveView<'_>, DecodeError> {
    let directive = project_directive(bytes)?;
    let name = str::from_utf8(directive.name).expect("validated directive names contain only ASCII");
    Ok(CacheDirectiveView {
        name,
        value: directive.value,
        raw: bytes,
    })
}

fn project_directive(bytes: &[u8]) -> Result<DirectiveParts<'_>, DecodeError> {
    if bytes.is_empty() {
        return Err(super::invalid_syntax(&FieldName::CacheControl));
    }
    if bytes.get(7) == Some(&b'=') && bytes.get(..7).is_some_and(|name| eq_ignore_ascii_case_scalar(name, b"max-age")) {
        let value = &bytes[8..];
        validate_directive_value(value, false)?;
        return Ok(DirectiveParts {
            name: &bytes[..7],
            value: Some(value),
            kind: DirectiveKind::MaxAge,
            delta_seconds: None,
        });
    }
    let bare_kind = match bytes.len() {
        6 if eq_ignore_ascii_case_scalar(bytes, b"public") => Some(DirectiveKind::Other),
        7 if eq_ignore_ascii_case_scalar(bytes, b"private") => Some(DirectiveKind::Other),
        8 if eq_ignore_ascii_case_scalar(bytes, b"no-cache") => Some(DirectiveKind::NoCache),
        8 if eq_ignore_ascii_case_scalar(bytes, b"no-store") => Some(DirectiveKind::Other),
        9 if eq_ignore_ascii_case_scalar(bytes, b"immutable") => Some(DirectiveKind::Other),
        12 if eq_ignore_ascii_case_scalar(bytes, b"no-transform") => Some(DirectiveKind::Other),
        14 if eq_ignore_ascii_case_scalar(bytes, b"only-if-cached") => Some(DirectiveKind::Other),
        15 if eq_ignore_ascii_case_scalar(bytes, b"must-revalidate") => Some(DirectiveKind::Other),
        16 if eq_ignore_ascii_case_scalar(bytes, b"proxy-revalidate") => Some(DirectiveKind::Other),
        _ => None,
    };
    if let Some(kind) = bare_kind {
        return Ok(DirectiveParts {
            name: bytes,
            value: None,
            kind,
            delta_seconds: None,
        });
    }
    let mut equals = None;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if byte == b'=' {
            equals = Some(index);
            break;
        }
        if !is_token_byte(byte) {
            return Err(DecodeError::new(&FieldName::CacheControl, DecodeErrorKind::InvalidToken));
        }
    }
    let (name, value) = if let Some(equals) = equals {
        let name = &bytes[..equals];
        if name.is_empty() {
            return Err(DecodeError::new(&FieldName::CacheControl, DecodeErrorKind::InvalidToken));
        }
        let value = &bytes[equals + 1..];
        validate_directive_value(value, false)?;
        (name, Some(value))
    } else {
        (bytes, None)
    };
    Ok(DirectiveParts {
        name,
        value,
        kind: classify_directive(name),
        delta_seconds: None,
    })
}

#[derive(Clone, Copy)]
struct DirectiveParts<'a> {
    name: &'a [u8],
    value: Option<&'a [u8]>,
    kind: DirectiveKind,
    delta_seconds: Option<u64>,
}

#[derive(Clone, Copy)]
enum DirectiveKind {
    NoCache,
    MaxAge,
    Other,
}

fn parse_directive_parts(bytes: &[u8]) -> Result<DirectiveParts<'_>, DecodeError> {
    if bytes.is_empty() {
        return Err(super::invalid_syntax(&FieldName::CacheControl));
    }
    if bytes.get(7) == Some(&b'=') && bytes.get(..7).is_some_and(|name| eq_ignore_ascii_case_scalar(name, b"max-age")) {
        let value = &bytes[8..];
        return Ok(DirectiveParts {
            name: &bytes[..7],
            value: Some(value),
            kind: DirectiveKind::MaxAge,
            delta_seconds: validate_directive_value(value, true)?,
        });
    }
    // Bare directives are a closed set in practice, and grouping the literals
    // by length turns recognition into one length test plus one comparison,
    // which is cheaper than the byte-at-a-time token scan below.
    let bare_kind = match bytes.len() {
        6 if eq_ignore_ascii_case_scalar(bytes, b"public") => Some(DirectiveKind::Other),
        7 if eq_ignore_ascii_case_scalar(bytes, b"private") => Some(DirectiveKind::Other),
        8 if eq_ignore_ascii_case_scalar(bytes, b"no-cache") => Some(DirectiveKind::NoCache),
        8 if eq_ignore_ascii_case_scalar(bytes, b"no-store") => Some(DirectiveKind::Other),
        9 if eq_ignore_ascii_case_scalar(bytes, b"immutable") => Some(DirectiveKind::Other),
        12 if eq_ignore_ascii_case_scalar(bytes, b"no-transform") => Some(DirectiveKind::Other),
        14 if eq_ignore_ascii_case_scalar(bytes, b"only-if-cached") => Some(DirectiveKind::Other),
        15 if eq_ignore_ascii_case_scalar(bytes, b"must-revalidate") => Some(DirectiveKind::Other),
        16 if eq_ignore_ascii_case_scalar(bytes, b"proxy-revalidate") => Some(DirectiveKind::Other),
        _ => None,
    };
    if let Some(kind) = bare_kind {
        return Ok(DirectiveParts {
            name: bytes,
            value: None,
            kind,
            delta_seconds: None,
        });
    }
    let mut equals = None;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if byte == b'=' {
            equals = Some(index);
            break;
        }
        if !is_token_byte(byte) {
            return Err(DecodeError::new(&FieldName::CacheControl, DecodeErrorKind::InvalidToken));
        }
    }
    let (name, value) = if let Some(equals) = equals {
        let name = &bytes[..equals];
        if name.is_empty() {
            return Err(DecodeError::new(&FieldName::CacheControl, DecodeErrorKind::InvalidToken));
        }
        (name, Some(&bytes[equals + 1..]))
    } else {
        (bytes, None)
    };
    let kind = classify_directive(name);
    let delta_seconds = value
        .map(|value| validate_directive_value(value, matches!(kind, DirectiveKind::MaxAge)))
        .transpose()?
        .flatten();
    Ok(DirectiveParts {
        name,
        value,
        kind,
        delta_seconds,
    })
}

fn classify_directive(name: &[u8]) -> DirectiveKind {
    if eq_ignore_ascii_case_scalar(name, b"no-cache") {
        DirectiveKind::NoCache
    } else if eq_ignore_ascii_case_scalar(name, b"max-age") {
        DirectiveKind::MaxAge
    } else {
        DirectiveKind::Other
    }
}

fn validate_directive_value(bytes: &[u8], parse_seconds: bool) -> Result<Option<u64>, DecodeError> {
    if bytes.first() == Some(&b'"') {
        validate_quoted_value(bytes, parse_seconds)
    } else {
        validate_token_value(bytes, parse_seconds)
    }
}

fn validate_token_value(bytes: &[u8], parse_seconds: bool) -> Result<Option<u64>, DecodeError> {
    if bytes.is_empty() {
        return Err(super::invalid_syntax(&FieldName::CacheControl));
    }
    // Digits are a subset of `token`, so a parsed number settles validation
    // too and skips the byte-at-a-time token scan below. A value that
    // overflows falls through to that scan, which accepts it as a token while
    // reporting no seconds.
    if parse_seconds && let Some(seconds) = validate::decimal_u64(bytes) {
        return Ok(Some(seconds));
    }
    let mut seconds = parse_seconds.then_some(0_u64);
    for byte in bytes.iter().copied() {
        if !is_token_byte(byte) {
            return Err(super::invalid_syntax(&FieldName::CacheControl));
        }
        seconds = seconds.and_then(|value| {
            byte.is_ascii_digit()
                .then(|| value.checked_mul(10)?.checked_add(u64::from(byte - b'0')))
                .flatten()
        });
    }
    Ok(seconds)
}

fn validate_quoted_value(bytes: &[u8], parse_seconds: bool) -> Result<Option<u64>, DecodeError> {
    if bytes.len() < 2 || bytes.first() != Some(&b'"') || bytes.last() != Some(&b'"') {
        return Err(super::invalid_syntax(&FieldName::CacheControl));
    }

    let mut escaped = false;
    let mut seconds = parse_seconds.then_some(0_u64);
    let mut has_digit = false;
    for byte in bytes[1..bytes.len() - 1].iter().copied() {
        if escaped {
            if !matches!(byte, b'\t' | b' '..=b'~' | 0x80..=0xff) {
                return Err(super::invalid_syntax(&FieldName::CacheControl));
            }
            escaped = false;
            seconds = None;
        } else if byte == b'\\' {
            escaped = true;
            seconds = None;
        } else if !matches!(byte, b'\t' | b' ' | b'!' | b'#'..=b'[' | b']'..=b'~' | 0x80..=0xff) {
            return Err(super::invalid_syntax(&FieldName::CacheControl));
        } else {
            has_digit |= byte.is_ascii_digit();
            seconds = seconds.and_then(|value| {
                byte.is_ascii_digit()
                    .then(|| value.checked_mul(10)?.checked_add(u64::from(byte - b'0')))
                    .flatten()
            });
        }
    }
    if escaped {
        Err(super::invalid_syntax(&FieldName::CacheControl))
    } else {
        Ok(seconds.filter(|_value| has_digit))
    }
}

fn valid_directive_value(bytes: &[u8]) -> bool {
    validate_directive_value(bytes, false).is_ok()
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn eq_ignore_ascii_case_scalar(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(left, right)| left.eq_ignore_ascii_case(right))
}

struct DirectiveItems<'a> {
    bytes: &'a [u8],
    start: usize,
    position: usize,
    finished: bool,
    skip_empty: bool,
}

impl<'a> DirectiveItems<'a> {
    const fn new(bytes: &'a [u8], skip_empty: bool) -> Self {
        Self {
            bytes,
            start: 0,
            position: 0,
            finished: false,
            skip_empty,
        }
    }
}

impl<'a> Iterator for DirectiveItems<'a> {
    type Item = Result<&'a [u8], DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        let mut quoted = false;
        let mut escaped = false;
        while let Some(byte) = self.bytes.get(self.position).copied() {
            if escaped {
                escaped = false;
            } else if quoted && byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = !quoted;
            } else if !quoted && byte == b',' {
                let item = super::trim_ows(&self.bytes[self.start..self.position]);
                self.position += 1;
                self.start = self.position;
                if self.skip_empty && item.is_empty() {
                    continue;
                }
                return Some(Ok(item));
            }
            self.position += 1;
        }
        self.finished = true;
        if quoted || escaped {
            Some(Err(DecodeError::new(&FieldName::CacheControl, DecodeErrorKind::UnterminatedQuote)))
        } else {
            let item = super::trim_ows(&self.bytes[self.start..]);
            if self.skip_empty && item.is_empty() { None } else { Some(Ok(item)) }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![expect(
        clippy::assertions_on_result_states,
        reason = "tests classify parser outcomes without needing successful values"
    )]

    use std::time::Duration;

    use super::{
        CacheControl, CacheControlOwned, CacheSummary, DirectiveItems, DirectiveKind, builder_wire_len, classify_directive, decimal_len,
        observe_directives, observe_quoted_directives, parse_directive, parse_directive_parts, project_directive, validate_directive_value,
    };
    use crate::DecodeError;

    /// The quote-free split is a shortcut around the general item walk, so
    /// both have to reach the same summary and the same diagnostic.
    #[test]
    fn the_unquoted_directive_split_agrees_with_the_general_walk() {
        let alphabet = b"m,\"=a1 \\";
        let mut line = Vec::with_capacity(4);
        for first in alphabet {
            for second in alphabet {
                for third in alphabet {
                    for fourth in alphabet {
                        line.clear();
                        line.extend_from_slice(&[*first, *second, *third, *fourth]);
                        if line.contains(&b'"') {
                            continue;
                        }

                        let mut split = CacheSummary::default();
                        let split = observe_directives(&line, 0, &mut split).map(|()| split);

                        let mut walk = CacheSummary::default();
                        let walk = observe_quoted_directives(&line, 0, &mut walk).map(|()| walk);

                        assert_eq!(
                            split.as_ref().map_err(DecodeError::kind),
                            walk.as_ref().map_err(DecodeError::kind),
                            "{:?}",
                            String::from_utf8_lossy(&line)
                        );
                    }
                }
            }
        }
    }

    use crate::sink::{EncodedValues, FieldSink};
    use crate::source::FieldSource;
    use crate::{DecodeErrorKind, FieldName, FieldValue, TestSink};

    #[test]
    fn directive_projection_covers_bare_and_malformed_inputs() {
        assert!(project_directive(b"").is_err());
        let bare = project_directive(b"no-cache").expect("known bare directive");
        assert_eq!(bare.name, b"no-cache");
        assert_eq!(bare.value, None);
        assert!(project_directive(b"bad value").is_err());
    }

    #[test]
    fn builder_keeps_short_extensions_inline_and_spills_long_ones() {
        let short = CacheControlOwned::builder().extension_value("stale-if-error", "30");
        let long = CacheControlOwned::builder().extension_value("extension-directive-with-a-value-that-exceeds-inline-storage", "enabled");
        assert!(matches!(
            short.directives.as_slice(),
            [super::BuilderDirective::Extension { name, value: Some(value) }]
                if !name.is_heap_allocated() && !value.is_heap_allocated()
        ));
        assert!(matches!(
            long.directives.as_slice(),
            [super::BuilderDirective::Extension { name, value: Some(value) }]
                if name.is_heap_allocated() && !value.is_heap_allocated()
        ));
    }

    #[test]
    fn builder_wire_length_checks_boundaries_without_allocating() {
        assert_eq!(builder_wire_len([3, 4], 2), Ok(9));
        assert_eq!(builder_wire_len([], 0), Ok(0));
        assert!(builder_wire_len([usize::MAX, 1], 2).is_err());
        assert!(builder_wire_len([0], usize::MAX).is_err());
        assert!(builder_wire_len([usize::MAX], 2).is_err());
    }

    #[test]
    fn directives_views_summaries_and_debug_cover_repeated_lines() {
        let mut table = TestSink::new();
        table
            .set_values(
                &FieldName::CacheControl,
                EncodedValues::from_vec(vec![
                    FieldValue::from_static("no-cache, x-mode=\"fast, safe\""),
                    FieldValue::from_static("max-age=30, public"),
                ]),
            )
            .expect("table accepts cache control");

        let view = CacheControl::view(&table).expect("valid cache control").expect("present");
        assert!(view.no_cache());
        assert_eq!(view.max_age(), Some(Duration::from_secs(30)));
        assert_eq!(view.field_values().count(), 2);
        assert!(format!("{view:?}").contains("value_count"));
        let directive = view
            .directives()
            .find(|directive| directive.name() == "x-mode")
            .expect("extension present");
        assert_eq!(directive.value(), Some(b"\"fast, safe\"".as_slice()));
        assert_eq!(directive.value_str(), Ok(Some("\"fast, safe\"")));
        assert_eq!(directive.as_bytes(), b"x-mode=\"fast, safe\"");

        let owned = CacheControl::owned(&table).expect("valid owned cache control").expect("present");
        assert!(owned.no_cache());
        assert_eq!(owned.max_age(), Some(Duration::from_secs(30)));
        assert_eq!(owned.directives().count(), 4);
        assert!(format!("{owned:?}").contains("summary"));

        let mut encoded = TestSink::new();
        CacheControl::insert(&mut encoded, owned).expect("table accepts normalized value");
        assert_eq!(
            encoded
                .lines(&FieldName::CacheControl)
                .expect("stored")
                .repeated()
                .next()
                .expect("one line")
                .as_bytes(),
            b"no-cache, x-mode=\"fast, safe\", max-age=30, public"
        );
        table.remove_values(&FieldName::CacheControl);
        assert!(CacheControl::view(&table).expect("absence is valid").is_none());
        assert!(CacheControl::owned(&table).expect("absence is valid").is_none());
    }

    #[test]
    fn builders_cover_static_numeric_extension_and_direct_encoding() {
        assert!(CacheControlOwned::builder().build().is_err());
        for builder in [
            CacheControl::public(),
            CacheControl::private(),
            CacheControl::no_cache(),
            CacheControlOwned::builder().no_store(),
            CacheControlOwned::builder().must_revalidate(),
            CacheControlOwned::builder().immutable(),
        ] {
            assert!(builder.build().is_ok());
        }

        let builder = CacheControlOwned::builder()
            .public()
            .no_cache()
            .private()
            .no_store()
            .must_revalidate()
            .immutable()
            .max_age(Duration::from_secs(u64::MAX))
            .extension_value("stale-if-error", "\"30\"")
            .extension_flag("custom");
        let built = builder.clone().build().expect("nonempty builder");
        assert_eq!(built.max_age(), Some(Duration::from_secs(u64::MAX)));

        let mut table = TestSink::new();
        table
            .set_encoded(&FieldName::CacheControl, builder)
            .expect("direct encoder succeeds");
        let wire = table
            .lines(&FieldName::CacheControl)
            .expect("stored")
            .repeated()
            .next()
            .expect("one line");
        assert!(wire.as_bytes().starts_with(b"public, no-cache, private"));
        assert!(wire.as_bytes().ends_with(b"stale-if-error=\"30\", custom"));

        assert!(table.set_encoded(&FieldName::CacheControl, CacheControlOwned::builder()).is_err());
        assert!(CacheControlOwned::builder().extension_flag("").build().is_err());
        assert!(CacheControlOwned::builder().extension_value("ok", "bad value").build().is_err());
        assert_eq!(decimal_len(0), 1);
        assert_eq!(decimal_len(9), 1);
        assert_eq!(decimal_len(10), 2);
        assert_eq!(decimal_len(u64::MAX), 20);
    }

    #[test]
    fn parser_covers_quotes_escapes_tokens_and_error_kinds() {
        assert!(CacheControlOwned::try_from("\n").is_err());
        assert!(CacheControlOwned::try_from(String::from("\n")).is_err());
        assert!(matches!(
            classify_directive(std::hint::black_box(b"MAX-AGE")),
            DirectiveKind::MaxAge
        ));
        let borrowed = CacheControlOwned::try_from("max-age=15").expect("valid borrowed input string");
        assert_eq!(borrowed.max_age(), Some(Duration::from_secs(15)));
        let owned = CacheControlOwned::try_from(String::from("MAX-AGE=\"45\", no-cache")).expect("valid owned string");
        assert_eq!(owned.max_age(), Some(Duration::from_secs(45)));
        assert!(CacheControlOwned::try_from(FieldValue::from_static("public")).is_ok());
        assert_eq!(
            "public".parse::<CacheControlOwned>().expect("shared FromStr").directives().count(),
            1
        );

        let obs = parse_directive(b"x-note=\"\xff\"").expect("valid obs-text quoted value");
        assert_eq!(
            obs.value_str().expect_err("obs-text is not UTF-8").kind(),
            DecodeErrorKind::InvalidUtf8
        );
        assert!(parse_directive(&[0xff]).is_err());

        for valid in [
            b"public".as_slice(),
            b"must-revalidate",
            b"no-cache",
            b"max-age=12",
            b"max-age=\"12\"",
            b"extension=value",
            b"extension=\"a,b\"",
            b"extension=\"a\\\\b\"",
        ] {
            assert!(parse_directive_parts(valid).is_ok(), "{valid:?}");
        }
        for invalid in [
            b"".as_slice(),
            b"=value",
            b"bad name",
            b"max-age=",
            b"x=\"unterminated",
            b"x=\"bad\\\n\"",
            b"x=\"trailing\\\"",
        ] {
            assert!(parse_directive_parts(invalid).is_err(), "{invalid:?}");
        }
        assert!(
            parse_directive_parts(b"max-age=12x")
                .expect("token value remains syntactically valid")
                .delta_seconds
                .is_none()
        );
        assert_eq!(validate_directive_value(b"123", true), Ok(Some(123)));
        assert_eq!(validate_directive_value(b"abc", true), Ok(None));
        assert_eq!(validate_directive_value(b"\"\"", true), Ok(None));
        assert_eq!(validate_directive_value(b"\"123\"", true), Ok(Some(123)));
        assert!(parse_directive_parts(b"no-cache=value").is_ok());

        let mut items = DirectiveItems::new(b", one, \"two,three\",", true);
        assert_eq!(items.next(), Some(Ok(b"one".as_slice())));
        assert_eq!(items.next(), Some(Ok(b"\"two,three\"".as_slice())));
        assert_eq!(items.next(), None);
        assert_eq!(items.next(), None);
        assert!(
            DirectiveItems::new(b"one, \"unterminated", false)
                .last()
                .expect("error item")
                .is_err()
        );

        assert!(CacheControlOwned::try_from("public,,private").is_err());
        let mut table = TestSink::new();
        table
            .set_values(
                &FieldName::CacheControl,
                EncodedValues::single(FieldValue::from_static("x=\"unterminated")),
            )
            .expect("table accepts raw field value");
        assert_eq!(
            CacheControl::view(&table).expect_err("unterminated quote must fail").value_index(),
            Some(0)
        );
        assert!(CacheControlOwned::try_from("").is_err());
        assert!(parse_directive_parts(b"x=\"bad\n\"").is_err());
    }
}
