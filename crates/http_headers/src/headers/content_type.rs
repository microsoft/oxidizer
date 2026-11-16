// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Structured `Content-Type` parsing with lazy parameters.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::str;

use crate::sink::{EncodedValues, FieldSink, InsertError};
use crate::source::FieldSource;
use crate::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValue, FieldValueRef, validate};

/// Keeps the common one- or two-parameter media type inline; increasing it enlarges every
/// parsed metadata value while reducing spills for unusually parameter-heavy values.
const INLINE_PARAMETER_CAPACITY: usize = 2;

/// Defines the `Content-Type` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 8.3](https://www.rfc-editor.org/rfc/rfc9110#section-8.3).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{ContentType, ContentTypeOwned};
///
/// let mut map = HeaderMap::new();
/// ContentType::insert(&mut map, ContentTypeOwned::try_from("text/plain")?)?;
/// assert!(ContentType::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct ContentType {
    _private: (),
}

impl ContentType {
    /// Creates the canonical `application/json` response value.
    #[must_use]
    pub fn json() -> ContentTypeOwned {
        ContentTypeOwned::json()
    }
}

/// Owned value for the `Content-Type` header.
///
/// Equality and hashing use the original field-value bytes, ignoring
/// sensitivity and the parsed metadata cache.
///
/// # Specification
///
/// Defined by [RFC 9110 section 8.3].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::ContentTypeOwned::try_from("text/html; charset=utf-8")?;
/// assert_eq!(value.parameter("charset")?, Some(b"utf-8".as_slice()));
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Content-Type: application/json` has no parameters.
/// `Content-Type: text/html; charset=UTF-8` carries a token parameter, while
/// `Content-Type: multipart/form-data; boundary="example boundary"` carries a
/// quoted parameter.
///
/// [RFC 9110 section 8.3]: https://www.rfc-editor.org/rfc/rfc9110#section-8.3
#[derive(Clone, Debug)]
pub struct ContentTypeOwned {
    value: FieldValue,
    metadata: ContentTypeMetadata,
}

impl PartialEq for ContentTypeOwned {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl Eq for ContentTypeOwned {}

impl Hash for ContentTypeOwned {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

/// Borrowed value for the `Content-Type` header.
#[derive(Clone, Debug)]
/// # Examples
///
/// ```
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::header::CONTENT_TYPE;
/// use http::{HeaderMap, HeaderValue};
/// use http_headers::Field;
/// use http_headers::headers::{ContentType, ContentTypeView};
///
/// let mut map = HeaderMap::new();
/// map.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
/// let view: ContentTypeView<'_> = ContentType::view(&map)?.expect("content type");
/// assert_eq!(view.subtype()?, "json");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
pub struct ContentTypeView<'a> {
    value: FieldValueRef<'a>,
    metadata: ContentTypeMetadata,
}

/// One borrowed media type parameter.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```
/// use http_headers::headers::{ContentTypeOwned, MediaTypeParameterView};
///
/// let value = ContentTypeOwned::try_from("text/html; charset=utf-8")?;
/// let parameter: MediaTypeParameterView<'_> = value.parameters().next().expect("charset")?;
/// assert_eq!(parameter.name(), "charset");
/// assert_eq!(parameter.value(), b"utf-8");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct MediaTypeParameterView<'a> {
    name: &'a str,
    value: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ContentTypeHead {
    type_start: u32,
    type_end: u32,
    subtype_start: u32,
    subtype_end: u32,
    parameter_start: u32,
    parameter_count: u32,
    inline_parameters: [InlineParameter; INLINE_PARAMETER_CAPACITY],
    inline_count: u8,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum ContentTypeMetadata {
    ApplicationJsonUtf8,
    TextHtmlUtf8,
    Common { type_end: u8, subtype_end: u8 },
    Parsed(Box<ContentTypeHead>),
}

impl ContentTypeMetadata {
    fn head(&self) -> ContentTypeHead {
        match self {
            // Byte offsets in "application/json; charset=utf-8".
            Self::ApplicationJsonUtf8 => common_parameter_head(11, 16, 18, 25, 26, 31),
            // Byte offsets in "text/html; charset=utf-8".
            Self::TextHtmlUtf8 => common_parameter_head(4, 9, 11, 18, 19, 24),
            Self::Common { type_end, subtype_end } => ContentTypeHead {
                type_start: 0,
                type_end: u32::from(*type_end),
                subtype_start: u32::from(*type_end) + 1,
                subtype_end: u32::from(*subtype_end),
                parameter_start: u32::from(*subtype_end),
                parameter_count: 0,
                inline_parameters: [InlineParameter::EMPTY; INLINE_PARAMETER_CAPACITY],
                inline_count: 0,
            },
            Self::Parsed(head) => **head,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ParameterRange {
    name: Range<u32>,
    value: Range<u32>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct InlineParameter {
    name_start: u16,
    name_end: u16,
    value_start: u16,
    value_end: u16,
}

impl InlineParameter {
    const EMPTY: Self = Self {
        name_start: 0,
        name_end: 0,
        value_start: 0,
        value_end: 0,
    };

    fn from_range(range: &ParameterRange) -> Option<Self> {
        Some(Self {
            name_start: u16::try_from(range.name.start).ok()?,
            name_end: u16::try_from(range.name.end).ok()?,
            value_start: u16::try_from(range.value.start).ok()?,
            value_end: u16::try_from(range.value.end).ok()?,
        })
    }

    fn name_range(self) -> Range<usize> {
        usize::from(self.name_start)..usize::from(self.name_end)
    }

    fn value_range(self) -> Range<usize> {
        usize::from(self.value_start)..usize::from(self.value_end)
    }
}

/// Iterator over borrowed media type parameters.
#[derive(Debug)]
/// # Examples
///
/// ```
/// use http_headers::headers::{ContentTypeOwned, MediaTypeParameters};
///
/// let value = ContentTypeOwned::try_from("multipart/form-data; boundary=----abc")?;
/// let mut parameters: MediaTypeParameters<'_> = value.parameters();
/// let parameter = parameters.next().expect("boundary")?;
/// assert_eq!(parameter.name(), "boundary");
/// assert_eq!(parameter.value(), b"----abc");
/// assert!(parameters.next().is_none());
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct MediaTypeParameters<'a> {
    bytes: &'a [u8],
    scanner: ParameterScanner<'a>,
    remaining: usize,
}

impl ContentTypeOwned {
    #[cfg(all(feature = "serde", feature = "headers-content-type"))]
    pub(crate) fn field_value(&self) -> FieldValueRef<'_> {
        self.value.as_field_value_ref()
    }

    /// Constructs `type/subtype`.
    ///
    /// # Errors
    ///
    /// Returns an error if either component is not an HTTP token.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentTypeOwned;
    ///
    /// let value = ContentTypeOwned::new("multipart", "form-data")?;
    /// assert_eq!(value.type_()?, "multipart");
    /// assert_eq!(value.subtype()?, "form-data");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn new(type_: impl AsRef<str>, subtype: impl AsRef<str>) -> Result<Self, DecodeError> {
        let type_ = type_.as_ref();
        let subtype = subtype.as_ref();
        if !validate::token(type_.as_bytes()) || !validate::token(subtype.as_bytes()) {
            return Err(DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidToken));
        }
        let subtype_start = type_.len() + 1;
        let total_len = subtype_start + subtype.len();
        let type_end = narrow_offset(type_.len())?;
        let subtype_start_offset = narrow_offset(subtype_start)?;
        let total_offset = narrow_offset(total_len)?;
        let mut wire = String::with_capacity(total_len);
        wire.push_str(type_);
        wire.push('/');
        wire.push_str(subtype);
        let value = content_type_value(wire);
        Ok(Self {
            value,
            metadata: ContentTypeMetadata::Parsed(Box::new(ContentTypeHead {
                type_start: 0,
                type_end,
                subtype_start: subtype_start_offset,
                subtype_end: total_offset,
                parameter_start: total_offset,
                parameter_count: 0,
                inline_parameters: [InlineParameter::EMPTY; INLINE_PARAMETER_CAPACITY],
                inline_count: 0,
            })),
        })
    }

    /// Returns `application/json`.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentTypeOwned;
    ///
    /// let value = ContentTypeOwned::json();
    /// assert_eq!(value.into_field_value().as_bytes(), b"application/json");
    /// ```
    pub fn json() -> Self {
        Self {
            value: FieldValue::from_static("application/json"),
            metadata: ContentTypeMetadata::Common {
                type_end: 11,
                subtype_end: 16,
            },
        }
    }

    /// Returns the top-level media type.
    ///
    /// # Errors
    ///
    /// Returns an error if the stored metadata and wire value disagree.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentTypeOwned;
    ///
    /// let value = ContentTypeOwned::try_from("application/json")?;
    /// assert_eq!(value.type_()?, "application");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn type_(&self) -> Result<&str, DecodeError> {
        component(
            self.value.as_bytes(),
            (
                // #[gamma::skip(call.replace_with_default, call_result.default, tag = "equivalent", reason = "The initial media-type token always starts at byte zero in every validated metadata variant")]
                widen_offset(self.metadata.head().type_start)
            )..widen_offset(self.metadata.head().type_end),
        )
    }

    /// Returns the media subtype.
    ///
    /// # Errors
    ///
    /// Returns an error if the stored metadata and wire value disagree.
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentTypeOwned;
    ///
    /// let value = ContentTypeOwned::try_from("text/html; charset=utf-8")?;
    /// assert_eq!(value.subtype()?, "html");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn subtype(&self) -> Result<&str, DecodeError> {
        component(
            self.value.as_bytes(),
            widen_offset(self.metadata.head().subtype_start)..widen_offset(self.metadata.head().subtype_end),
        )
    }

    /// Iterates parameters in wire order without allocating.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentTypeOwned;
    ///
    /// let value = ContentTypeOwned::try_from("text/html; charset=utf-8")?;
    /// let mut parameters = value.parameters();
    /// let parameter = parameters.next().expect("charset")?;
    /// assert_eq!(parameter.name(), "charset");
    /// assert_eq!(parameter.value(), b"utf-8");
    /// assert!(parameters.next().is_none());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn parameters(&self) -> MediaTypeParameters<'_> {
        MediaTypeParameters::lazy(self.value.as_bytes(), self.metadata.head())
    }

    /// Returns the first parameter matching `name` case-insensitively.
    ///
    /// # Errors
    ///
    /// Returns an error if stored metadata does not match the wire value.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::ContentTypeOwned::try_from("text/html; charset=utf-8")?;
    /// assert_eq!(value.parameter("charset")?, Some(b"utf-8".as_slice()));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn parameter(&self, name: &str) -> Result<Option<&[u8]>, DecodeError> {
        find_parameter(self.value.as_bytes(), self.metadata.head(), name)
    }

    /// Returns reusable wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentTypeOwned;
    ///
    /// let value = ContentTypeOwned::try_from("multipart/form-data; boundary=----abc")?;
    /// let field_value = value.into_field_value();
    /// assert_eq!(
    ///     field_value.as_bytes(),
    ///     b"multipart/form-data; boundary=----abc"
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::shared::impl_field_value_conversion!(ContentTypeOwned, |value| value.value);

impl<'a> ContentTypeView<'a> {
    /// Returns the top-level media type.
    ///
    /// # Errors
    ///
    /// Returns an error if stored metadata does not match the wire value.
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use http::header::CONTENT_TYPE;
    /// use http::{HeaderMap, HeaderValue};
    /// use http_headers::Field;
    /// use http_headers::headers::ContentType;
    ///
    /// let mut map = HeaderMap::new();
    /// map.insert(
    ///     CONTENT_TYPE,
    ///     HeaderValue::from_static("multipart/form-data; boundary=----abc"),
    /// );
    /// let view = ContentType::view(&map)?.expect("content type");
    /// assert_eq!(view.type_()?, "multipart");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub fn type_(&self) -> Result<&'a str, DecodeError> {
        component(
            self.value.as_bytes(),
            (
                // #[gamma::skip(call.replace_with_default, call_result.default, tag = "equivalent", reason = "The initial media-type token always starts at byte zero in every validated metadata variant")]
                widen_offset(self.metadata.head().type_start)
            )..widen_offset(self.metadata.head().type_end),
        )
    }

    /// Returns the media subtype.
    ///
    /// # Errors
    ///
    /// Returns an error if stored metadata does not match the wire value.
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use http::header::CONTENT_TYPE;
    /// use http::{HeaderMap, HeaderValue};
    /// use http_headers::Field;
    /// use http_headers::headers::ContentType;
    ///
    /// let mut map = HeaderMap::new();
    /// map.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    /// let view = ContentType::view(&map)?.expect("content type");
    /// assert_eq!(view.subtype()?, "json");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub fn subtype(&self) -> Result<&'a str, DecodeError> {
        component(
            self.value.as_bytes(),
            widen_offset(self.metadata.head().subtype_start)..widen_offset(self.metadata.head().subtype_end),
        )
    }

    /// Iterates parameters in wire order.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use http::header::CONTENT_TYPE;
    /// use http::{HeaderMap, HeaderValue};
    /// use http_headers::Field;
    /// use http_headers::headers::ContentType;
    ///
    /// let mut map = HeaderMap::new();
    /// map.insert(
    ///     CONTENT_TYPE,
    ///     HeaderValue::from_static("text/html; charset=utf-8"),
    /// );
    /// let view = ContentType::view(&map)?.expect("content type");
    /// let parameter = view.parameters().next().expect("charset")?;
    /// assert_eq!(parameter.name(), "charset");
    /// assert_eq!(parameter.value(), b"utf-8");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub fn parameters(&self) -> MediaTypeParameters<'a> {
        MediaTypeParameters::lazy(self.value.as_bytes(), self.metadata.head())
    }

    /// Returns the first parameter matching `name` case-insensitively.
    ///
    /// # Errors
    ///
    /// Returns an error if stored metadata does not match the wire value.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::ContentTypeOwned::try_from("text/html; charset=utf-8")?;
    /// assert_eq!(value.parameter("charset")?, Some(b"utf-8".as_slice()));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn parameter(&self, name: &str) -> Result<Option<&'a [u8]>, DecodeError> {
        find_parameter(self.value.as_bytes(), self.metadata.head(), name)
    }

    /// Returns the original field value.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use http::header::CONTENT_TYPE;
    /// use http::{HeaderMap, HeaderValue};
    /// use http_headers::Field;
    /// use http_headers::headers::ContentType;
    ///
    /// let mut map = HeaderMap::new();
    /// map.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    /// let view = ContentType::view(&map)?.expect("content type");
    /// assert_eq!(view.as_field_value().as_bytes(), b"application/json");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub const fn as_field_value(&self) -> FieldValueRef<'a> {
        self.value
    }
}

impl<'a> MediaTypeParameterView<'a> {
    /// Returns the parameter name.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::ContentTypeOwned;
    ///
    /// let value = ContentTypeOwned::try_from("text/html; charset=utf-8")?;
    /// let parameter = value.parameters().next().expect("charset")?;
    /// assert_eq!(parameter.name(), "charset");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn name(self) -> &'a str {
        self.name
    }

    /// Returns the raw token or quoted-string parameter value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::ContentTypeOwned::try_from("text/html; charset=utf-8")?;
    /// assert_eq!(value.parameter("charset")?, Some(b"utf-8".as_slice()));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn value(self) -> &'a [u8] {
        self.value
    }
}

impl Field for ContentType {
    type View<'a> = ContentTypeView<'a>;
    type Owned = ContentTypeOwned;

    fn name() -> &'static FieldName {
        &FieldName::ContentType
    }

    fn view_with<S>(source: &S, mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_list_item_limit(b';', false)?;
        let value = lines.exactly_one()?;
        let metadata = parse_metadata_with(value.as_bytes(), mode)?;
        Ok(Some(ContentTypeView { value, metadata }))
    }

    fn owned_with<S>(source: &S, mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_list_item_limit(b';', false)?;
        let owned = lines.exactly_one_owned()?;
        let metadata = parse_metadata_with(owned.as_bytes(), mode)?;
        Ok(Some(ContentTypeOwned { value: owned, metadata }))
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), EncodedValues::single(value.value))
    }
}

super::shared::impl_string_conversions!(ContentTypeOwned, &FieldName::ContentType, super::invalid_syntax, value);

impl TryFrom<FieldValue> for ContentTypeOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        let metadata = parse_metadata(value.as_bytes())?;
        Ok(Self { value, metadata })
    }
}

fn content_type_value(wire: String) -> FieldValue {
    FieldValue::try_from(wire).expect("validated media type components produce a field value")
}

impl<'a> MediaTypeParameters<'a> {
    fn lazy(bytes: &'a [u8], head: ContentTypeHead) -> Self {
        Self {
            bytes,
            scanner: ParameterScanner::new(bytes, widen_offset(head.parameter_start)),
            remaining: widen_offset(head.parameter_count),
        }
    }
}

impl<'a> Iterator for MediaTypeParameters<'a> {
    type Item = Result<MediaTypeParameterView<'a>, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        let range = self.scanner.next()?;
        self.remaining = self.remaining.saturating_sub(1);
        Some(range.and_then(|range| parameter_from_range(self.bytes, &range)))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.remaining))
    }
}

fn parse_metadata(bytes: &[u8]) -> Result<ContentTypeMetadata, DecodeError> {
    if let Some(metadata) =
        // #[gamma::skip(call.replace_with_default, call_result.default, tag = "equivalent", reason = "Returning no cached metadata runs the complete prefix and parameter parser over the same validated wire representation")]
        common_metadata(bytes)
    {
        return Ok(metadata);
    }
    let mut head = parse_prefix(bytes)?;
    for parameter in ParameterScanner::new(bytes, widen_offset(head.parameter_start)) {
        let parameter = parameter?;
        // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "The count starts at zero and each validated parameter consumes at least three bytes with u32-bounded endpoints, so fewer than u32::MAX parameters can ever reach observe_parameter")]
        head.observe_parameter(&parameter)?;
    }
    Ok(ContentTypeMetadata::Parsed(Box::new(head)))
}

#[inline]
fn parse_metadata_with(bytes: &[u8], mode: crate::DecodeMode) -> Result<ContentTypeMetadata, DecodeError> {
    if mode == crate::DecodeMode::Strict {
        return parse_metadata(bytes);
    }
    if let Ok(metadata) = parse_metadata(bytes) {
        return Ok(metadata);
    }
    let mut head = parse_prefix_relaxed(bytes)?;
    for parameter in ParameterScanner::new(bytes, widen_offset(head.parameter_start)) {
        let parameter = parameter?;
        // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "The count starts at zero and each validated parameter consumes at least three bytes with u32-bounded endpoints, so fewer than u32::MAX parameters can ever reach observe_parameter")]
        head.observe_parameter(&parameter)?;
    }
    Ok(ContentTypeMetadata::Parsed(Box::new(head)))
}

// #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "The only fallible operations convert literal offsets at most 24 to u8; every recognized table entry fits")]
fn common_metadata(bytes: &[u8]) -> Option<ContentTypeMetadata> {
    if bytes == b"application/json; charset=utf-8" {
        return Some(ContentTypeMetadata::ApplicationJsonUtf8);
    }
    let (type_end, subtype_start, subtype_end) = match bytes {
        b"application/json" => (11, 12, 16),
        b"application/octet-stream" => (11, 12, 24),
        b"text/html" => (4, 5, 9),
        b"text/plain" => (4, 5, 10),
        b"text/css" => (4, 5, 8),
        b"application/javascript" => (11, 12, 22),
        b"text/html; charset=utf-8" => {
            return Some(ContentTypeMetadata::TextHtmlUtf8);
        }
        _ => return None,
    };
    debug_assert_eq!(
        subtype_start,
        type_end + 1,
        "known media type subtype must begin immediately after the slash"
    );
    Some(ContentTypeMetadata::Common {
        type_end: u8::try_from(type_end).ok()?,
        subtype_end: u8::try_from(subtype_end).ok()?,
    })
}

fn common_parameter_head(
    type_end: u32,
    subtype_end: u32,
    name_start: u16,
    name_end: u16,
    value_start: u16,
    value_end: u16,
) -> ContentTypeHead {
    ContentTypeHead {
        type_start: 0,
        type_end,
        subtype_start: type_end + 1,
        subtype_end,
        parameter_start: subtype_end,
        parameter_count: 1,
        inline_parameters: [
            InlineParameter {
                name_start,
                name_end,
                value_start,
                value_end,
            },
            InlineParameter::EMPTY,
        ],
        // #[gamma::skip(literal.int_decrement, tag = "equivalent", reason = "Leaving this one known parameter uncached only makes find_parameter use the complete scanner; parameter_count still preserves iteration and size hints")]
        inline_count: 1,
    }
}

fn parse_prefix(bytes: &[u8]) -> Result<ContentTypeHead, DecodeError> {
    let mut position = 0;
    let type_range = take_token(bytes, &mut position)?;
    if bytes.get(position) != Some(&b'/') {
        return Err(super::invalid_syntax(&FieldName::ContentType));
    }
    position += 1;
    let subtype_range = take_token(bytes, &mut position)?;
    let parameter_start = position;
    Ok(ContentTypeHead {
        // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "The first take_token starts at position zero and returns that unchanged start offset")]
        type_start: narrow_offset(type_range.start)?,
        type_end: narrow_offset(type_range.end)?,
        subtype_start: narrow_offset(subtype_range.start)?,
        subtype_end: narrow_offset(subtype_range.end)?,
        // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "parameter_start equals subtype_range.end, whose identical narrowing conversion succeeded immediately above")]
        parameter_start: narrow_offset(parameter_start)?,
        parameter_count: 0,
        inline_parameters: [InlineParameter::EMPTY; INLINE_PARAMETER_CAPACITY],
        inline_count: 0,
    })
}

fn parse_prefix_relaxed(bytes: &[u8]) -> Result<ContentTypeHead, DecodeError> {
    let mut position = 0;
    let type_range = take_token(bytes, &mut position)?;
    // #[gamma::skip(call_result.default, tag = "equivalent", reason = "default_after preserves skip_ows side effects and replaces its unit result with unit")]
    skip_ows(bytes, &mut position);
    if bytes.get(position) != Some(&b'/') {
        return Err(super::invalid_syntax(&FieldName::ContentType));
    }
    position += 1;
    // #[gamma::skip(call_result.default, tag = "equivalent", reason = "default_after preserves skip_ows side effects and replaces its unit result with unit")]
    skip_ows(bytes, &mut position);
    let subtype_range = take_token(bytes, &mut position)?;
    let parameter_start = position;
    Ok(ContentTypeHead {
        // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "The first take_token starts at position zero and returns that unchanged start offset")]
        type_start: narrow_offset(type_range.start)?,
        type_end: narrow_offset(type_range.end)?,
        subtype_start: narrow_offset(subtype_range.start)?,
        subtype_end: narrow_offset(subtype_range.end)?,
        // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "parameter_start equals subtype_range.end, whose identical narrowing conversion succeeded immediately above")]
        parameter_start: narrow_offset(parameter_start)?,
        parameter_count: 0,
        inline_parameters: [InlineParameter::EMPTY; INLINE_PARAMETER_CAPACITY],
        inline_count: 0,
    })
}

impl ContentTypeHead {
    fn observe_parameter(&mut self, parameter: &ParameterRange) -> Result<(), DecodeError> {
        if widen_offset(self.parameter_count) == usize::from(self.inline_count)
            && usize::from(self.inline_count) < INLINE_PARAMETER_CAPACITY
            && let Some(compact) = InlineParameter::from_range(parameter)
        {
            self.inline_parameters[usize::from(self.inline_count)] = compact;
            self.inline_count += 1;
        }
        self.parameter_count = self
            .parameter_count
            .checked_add(1)
            .ok_or_else(|| DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidNumber))?;
        Ok(())
    }
}

#[derive(Debug)]
struct ParameterScanner<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> ParameterScanner<'a> {
    const fn new(bytes: &'a [u8], position: usize) -> Self {
        Self { bytes, position }
    }
}

impl Iterator for ParameterScanner<'_> {
    type Item = Result<ParameterRange, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // #[gamma::skip(call_result.default, tag = "equivalent", reason = "default_after preserves skip_ows side effects and replaces its unit result with unit")]
            skip_ows(self.bytes, &mut self.position);
            if self.position == self.bytes.len() {
                return None;
            }
            if self.bytes.get(self.position) != Some(&b';') {
                self.position = self.bytes.len();
                return Some(Err(super::invalid_syntax(&FieldName::ContentType)));
            }
            // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            self.position +=
            // #[gamma::skip(literal.int_decrement, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            1;
            // #[gamma::skip(call_result.default, tag = "equivalent", reason = "default_after preserves skip_ows side effects and replaces its unit result with unit")]
            skip_ows(self.bytes, &mut self.position);
            if self.position == self.bytes.len() || self.bytes.get(self.position) == Some(&b';') {
                continue;
            }
            break;
        }

        let name = match take_token(self.bytes, &mut self.position) {
            Ok(name) => name,
            Err(error) => {
                self.position = self.bytes.len();
                return Some(Err(error));
            }
        };
        if self.bytes.get(self.position) != Some(&b'=') {
            self.position = self.bytes.len();
            return Some(Err(super::invalid_syntax(&FieldName::ContentType)));
        }
        self.position += 1;
        let value = match take_parameter_value(self.bytes, &mut self.position) {
            Ok(value) => value,
            Err(error) => {
                self.position = self.bytes.len();
                return Some(Err(error));
            }
        };
        Some(narrow_parameter_range(name, value))
    }
}

fn take_token(bytes: &[u8], position: &mut usize) -> Result<Range<usize>, DecodeError> {
    let start = *position;
    while bytes
        .get(*position)
        .is_some_and(|byte| TOKEN_BITS[usize::from(*byte >> 6)] & (1_u64 << (*byte & 63)) != 0)
    {
        // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        *position +=
        // #[gamma::skip(literal.int_decrement, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        1;
    }

    if *position > start {
        Ok(start..*position)
    } else {
        Err(DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidToken))
    }
}

// Four words avoid branch-heavy token classification without a 256-byte lookup table.
static TOKEN_BITS: [u64; 4] = {
    let mut table = [0; 4];
    let mut byte = 0_u8;
    loop {
        if validate::token_byte(byte) {
            table[(byte >> 6) as usize] |= 1_u64 << (byte & 63);
        }
        if byte == u8::MAX {
            break;
        }
        byte += 1;
    }
    table
};

fn take_parameter_value(bytes: &[u8], position: &mut usize) -> Result<Range<usize>, DecodeError> {
    if bytes.get(*position) != Some(&b'"') {
        let mut end = *position;
        let token = take_token(bytes, &mut end)?;
        *position = end;
        return Ok(token);
    }
    let start = *position;
    *position += 1;
    let mut escaped = false;
    while let Some(byte) = bytes.get(*position).copied() {
        // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        *position +=
        // #[gamma::skip(literal.int_decrement, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        1;
        if escaped {
            if !matches!(byte, b'\t' | b' '..=b'~' | 0x80..=0xff) {
                return Err(super::invalid_syntax(&FieldName::ContentType));
            }
            // #[gamma::skip(assign_value.default, tag = "equivalent", reason = "false is exactly bool::default(); the assignment still executes")]
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return Ok(start..*position);
        } else if !matches!(byte, b'\t' | b' ' | b'!' | b'#'..=b'[' | b']'..=b'~' | 0x80..=0xff) {
            return Err(super::invalid_syntax(&FieldName::ContentType));
        }
    }
    Err(DecodeError::new(&FieldName::ContentType, DecodeErrorKind::UnterminatedQuote))
}

fn skip_ows(bytes: &[u8], position: &mut usize) {
    while bytes.get(*position).is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        *position +=
        // #[gamma::skip(literal.int_decrement, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        1;
    }
}

fn component(bytes: &[u8], range: Range<usize>) -> Result<&str, DecodeError> {
    let component = bytes.get(range).ok_or_else(|| super::invalid_syntax(&FieldName::ContentType))?;
    str::from_utf8(component).map_err(|_invalid| DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidUtf8))
}

fn parameter_from_range<'a>(bytes: &'a [u8], range: &ParameterRange) -> Result<MediaTypeParameterView<'a>, DecodeError> {
    let name = component(bytes, widen_range(range.name.clone()))?;
    let value = bytes
        .get(widen_range(range.value.clone()))
        .ok_or_else(|| super::invalid_syntax(&FieldName::ContentType))?;
    Ok(MediaTypeParameterView { name, value })
}

fn find_parameter<'a>(bytes: &'a [u8], head: ContentTypeHead, name: &str) -> Result<Option<&'a [u8]>, DecodeError> {
    for parameter in &head.inline_parameters[..usize::from(head.inline_count)] {
        let parameter = parameter_from_inline(bytes, *parameter)?;
        if validate::eq_ignore_ascii_case(parameter.name.as_bytes(), name.as_bytes()) {
            return Ok(Some(parameter.value));
        }
    }
    if
    // #[gamma::skip(cond.always_false, relational.le_to_lt, tag = "equivalent", reason = "When every parameter was already searched inline, rescanning and skipping exactly inline_count parameters yields no further items")]
    widen_offset(head.parameter_count) <= usize::from(head.inline_count) {
        return Ok(None);
    }
    for parameter in ParameterScanner::new(bytes, widen_offset(head.parameter_start)).skip(usize::from(head.inline_count)) {
        let range = parameter?;
        // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "A successful ParameterScanner item has in-bounds u32 endpoints and an ASCII token name, so materializing its slices and UTF-8 name cannot fail")]
        let parameter = parameter_from_range(bytes, &range)?;
        if validate::eq_ignore_ascii_case(parameter.name.as_bytes(), name.as_bytes()) {
            return Ok(Some(parameter.value));
        }
    }
    Ok(None)
}

fn narrow_offset(offset: usize) -> Result<u32, DecodeError> {
    #[cfg(test)]
    if tests::FAIL_NARROW_OFFSET.get() == Some(offset) {
        tests::FAIL_NARROW_OFFSET.set(None);
        return Err(DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidNumber));
    }
    u32::try_from(offset).map_err(|_overflow| DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidNumber))
}

fn narrow_range(range: Range<usize>) -> Result<Range<u32>, DecodeError> {
    Ok(narrow_offset(range.start)?..narrow_offset(range.end)?)
}

#[inline]
fn narrow_parameter_range(name: Range<usize>, value: Range<usize>) -> Result<ParameterRange, DecodeError> {
    Ok(ParameterRange {
        name: narrow_range(name)?,
        value: narrow_range(value)?,
    })
}

fn widen_range(range: Range<u32>) -> Range<usize> {
    widen_offset(range.start)..widen_offset(range.end)
}

#[inline]
fn widen_offset(offset: u32) -> usize {
    usize::try_from(offset).unwrap_or(
        // #[gamma::skip(expr.decrement, tag = "equivalent", reason = "This fallback is unreachable: every u32 offset and count fits usize on supported 32-bit and 64-bit targets")]
        usize::MAX,
    )
}

fn parameter_from_inline(bytes: &[u8], range: InlineParameter) -> Result<MediaTypeParameterView<'_>, DecodeError> {
    let name = component(bytes, range.name_range())?;
    let value = bytes
        .get(range.value_range())
        .ok_or_else(|| super::invalid_syntax(&FieldName::ContentType))?;
    Ok(MediaTypeParameterView { name, value })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![expect(
        clippy::assertions_on_result_states,
        reason = "tests classify parser outcomes without needing successful values"
    )]

    std::thread_local! {
        pub(super) static FAIL_NARROW_OFFSET: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    }

    #[test]
    fn gamma_afternoon_public_content_type_callers_propagate_each_narrowing_failure() {
        use crate::Field;

        for offset in [5, 6, 10] {
            FAIL_NARROW_OFFSET.set(Some(offset));
            let result = super::ContentTypeOwned::new("alpha", "beta");
            let consumed = FAIL_NARROW_OFFSET.replace(None);
            assert_eq!(consumed, None);
            assert_eq!(result.unwrap_err().kind(), crate::DecodeErrorKind::InvalidNumber);
        }
        for (wire, mode, offsets) in [
            ("alpha/beta", crate::DecodeMode::Strict, [5, 6, 10]),
            ("alpha / beta", crate::DecodeMode::Relaxed, [5, 8, 12]),
        ] {
            for offset in offsets {
                FAIL_NARROW_OFFSET.set(Some(offset));
                let source = Source(crate::FieldValue::from_static(wire));
                let result = super::ContentType::owned_with(&source, mode);
                let consumed = FAIL_NARROW_OFFSET.replace(None);
                assert_eq!(consumed, None, "{wire}/{offset}");
                let error = result.unwrap_err();
                assert_eq!(error.header(), &crate::FieldName::ContentType);
                assert_eq!(error.kind(), crate::DecodeErrorKind::InvalidNumber);
            }
        }
    }

    use std::slice;

    #[test]
    fn find_parameter_gamma_afternoon_propagates_corrupt_inline_and_lazy_metadata_errors() {
        let bytes = b"a/b;p";
        let mut head = super::parse_prefix(bytes).unwrap();
        head.parameter_count = 1;
        head.inline_count = 1;
        head.inline_parameters[0] = super::InlineParameter {
            name_start: 100,
            name_end: 101,
            value_start: 102,
            value_end: 103,
        };
        assert_eq!(
            super::find_parameter(bytes, head, "p").unwrap_err().kind(),
            crate::DecodeErrorKind::InvalidSyntax
        );
        head.inline_count = 0;
        assert_eq!(
            super::find_parameter(bytes, head, "p").unwrap_err().kind(),
            crate::DecodeErrorKind::InvalidSyntax
        );
    }

    use super::{
        ContentType, ContentTypeHead, ContentTypeMetadata, ContentTypeOwned, InlineParameter, ParameterRange, common_metadata, component,
        narrow_offset, narrow_parameter_range, parameter_from_inline, parameter_from_range, parse_metadata_with, take_parameter_value,
        take_token,
    };
    use crate::sink::FieldSink;
    use crate::source::{FieldLines, FieldSource};
    use crate::{DecodeErrorKind, DecodeMode, FieldName, FieldValue, TestSink};

    struct Source(FieldValue);

    #[test]
    fn gamma_round2_common_content_types_return_cached_metadata() {
        for wire in [
            b"application/json".as_slice(),
            b"application/octet-stream",
            b"text/html",
            b"text/plain",
            b"text/css",
            b"application/javascript",
            b"application/json; charset=utf-8",
            b"text/html; charset=utf-8",
        ] {
            let metadata = super::common_metadata(wire).expect("these literals are recognized by the metadata cache itself");
            let head = metadata.head();
            let end = wire.iter().position(|byte| *byte == b';').unwrap_or(wire.len());
            let slash = wire.iter().position(|byte| *byte == b'/').unwrap();
            let slash = u32::try_from(slash).unwrap();
            let end = u32::try_from(end).unwrap();
            assert_eq!(
                (head.type_start, head.type_end, head.subtype_start, head.subtype_end),
                (0, slash, slash + 1, end)
            );
            assert_eq!(head.parameter_count, u32::from(wire.contains(&b';')));
        }
    }

    #[test]
    fn gamma_round2_narrow_range_checks_the_end_without_allocating_payload() {
        let overflow = usize::try_from(u32::MAX).unwrap().checked_add(1);
        if let Some(overflow) = overflow {
            assert_eq!(
                super::narrow_range(0..overflow).unwrap_err().kind(),
                crate::DecodeErrorKind::InvalidNumber
            );
        }
    }

    #[test]
    fn gamma_round2_parameter_scanner_errors_are_terminal_at_every_stage() {
        for bytes in [
            b"x".as_slice(),
            b"xname=a",
            b";@=a",
            b";name",
            b";name other=x",
            b";name=",
            b";name=@;next=x",
            b";name=\"unterminated",
        ] {
            let mut scanner = super::ParameterScanner::new(bytes, 0);
            assert!(scanner.next().unwrap().err().is_some(), "{bytes:?}");
            assert!(scanner.next().is_none(), "{bytes:?}");
            assert!(scanner.next().is_none(), "{bytes:?}");
        }
    }

    #[test]
    fn gamma_round2_inline_parameter_range_rejects_each_unrepresentable_endpoint() {
        let overflow = u32::from(u16::MAX) + 1;
        for range in [
            ParameterRange {
                name: 0..1,
                value: overflow..overflow,
            },
            ParameterRange {
                name: 0..1,
                value: 2..overflow,
            },
        ] {
            assert!(InlineParameter::from_range(&range).is_none());
        }
        let mut head = super::parse_prefix(b"a/b").unwrap();
        head.observe_parameter(&ParameterRange {
            name: 0..1,
            value: overflow..overflow,
        })
        .unwrap();
        head.observe_parameter(&ParameterRange { name: 0..1, value: 2..3 }).unwrap();
        assert_eq!(head.parameter_count, 2);
        assert_eq!(
            head.inline_count, 0,
            "an uncacheable first parameter must not shift later inline slots"
        );
    }

    impl FieldSource for Source {
        fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
            (name == &FieldName::ContentType)
                .then(|| FieldLines::from_slice(name, slice::from_ref(&self.0)))
                .flatten()
        }
    }

    #[test]
    fn ordinary_view_uses_lazy_inline_metadata() {
        let source = Source(FieldValue::from_static("application/json; charset=utf-8"));
        let view = ContentType::view(&source).expect("valid media type").expect("media type present");
        assert_eq!(view.parameters().count(), 1);
        assert_eq!(view.type_(), Ok("application"));
    }

    #[test]
    fn token_prefix_scanning_checks_every_byte_and_preserves_the_delimiter() {
        const TOKEN_CHARACTERS: &[u8] = b"!#$%&'*+-.^_`|~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

        for byte in u8::MIN..=u8::MAX {
            let bytes = [b'/', byte, b'/'];
            let mut position = 1;
            let token = take_token(&bytes, &mut position);
            if TOKEN_CHARACTERS.contains(&byte) {
                assert_eq!(token, Ok(1..2), "{byte}");
                assert_eq!(position, 2, "{byte}");
            } else {
                assert_eq!(token.unwrap_err().kind(), DecodeErrorKind::InvalidToken, "{byte}");
                assert_eq!(position, 1, "{byte}");
            }
            let prefixed = [b'/', b'a', byte, b'/'];
            let mut position = 1;
            let end = if TOKEN_CHARACTERS.contains(&byte) { 3 } else { 2 };
            assert_eq!(take_token(&prefixed, &mut position), Ok(1..end), "{byte}");
            assert_eq!(position, end, "{byte}");
        }
    }

    #[test]
    fn parameter_lookup_falls_back_beyond_inline_capacity() {
        let content_type = ContentTypeOwned::try_from("text/plain; a=1; b=2; c=3; d=4").expect("valid media type");
        assert_eq!(content_type.metadata.head().inline_count, 2);
        assert_eq!(content_type.parameter("a"), Ok(Some(b"1".as_slice())));
        assert_eq!(content_type.parameter("b"), Ok(Some(b"2".as_slice())));
        assert_eq!(content_type.parameter("d"), Ok(Some(b"4".as_slice())));
        assert_eq!(content_type.parameter("missing"), Ok(None));
    }

    #[test]
    fn common_and_constructed_types_cover_accessors_and_round_trips() {
        for (wire, type_, subtype) in [
            ("application/json", "application", "json"),
            ("application/octet-stream", "application", "octet-stream"),
            ("text/html", "text", "html"),
            ("text/plain", "text", "plain"),
            ("text/css", "text", "css"),
            ("application/javascript", "application", "javascript"),
        ] {
            let content_type = ContentTypeOwned::try_from(wire).expect("common media type");
            assert_eq!(content_type.type_(), Ok(type_));
            assert_eq!(content_type.subtype(), Ok(subtype));
            assert_eq!(content_type.parameters().size_hint(), (0, Some(0)));
            assert_eq!(content_type.parameter("missing"), Ok(None));
        }

        let constructed = ContentTypeOwned::new("image", "svg+xml").expect("valid tokens");
        assert_eq!(constructed.type_(), Ok("image"));
        assert_eq!(constructed.subtype(), Ok("svg+xml"));
        assert_eq!(constructed.clone().into_field_value(), FieldValue::from_static("image/svg+xml"));
        assert_eq!(ContentTypeOwned::json().type_(), Ok("application"));
        assert_eq!(ContentType::json().subtype(), Ok("json"));
        assert!(ContentTypeOwned::new("", "plain").is_err());
        assert!(ContentTypeOwned::new("text", "bad value").is_err());
        assert_eq!(
            ContentTypeOwned::try_from(String::from("audio/ogg"))
                .expect("valid owned media type")
                .subtype(),
            Ok("ogg")
        );

        let html_utf8 = ContentTypeOwned::try_from("text/html; charset=utf-8").expect("common parameter form");
        assert_eq!(html_utf8.parameters().count(), 1);
        assert_eq!(html_utf8.parameter("charset"), Ok(Some(b"utf-8".as_slice())));

        let mut table = TestSink::new();
        ContentType::insert(&mut table, constructed).expect("table accepts content type");
        let view = ContentType::view(&table).expect("valid content type").expect("present");
        assert_eq!(view.type_(), Ok("image"));
        assert_eq!(view.subtype(), Ok("svg+xml"));
        assert_eq!(view.parameter("missing"), Ok(None));
        assert_eq!(view.parameters().count(), 0);
        assert_eq!(view.as_field_value().as_bytes(), b"image/svg+xml");
        assert!(ContentType::owned(&table).expect("valid owned content type").is_some());
        table.remove_values(&FieldName::ContentType);
        assert!(ContentType::view(&table).expect("absence is valid").is_none());
        assert!(ContentType::owned(&table).expect("absence is valid").is_none());
    }

    #[test]
    fn parameters_cover_inline_lazy_quoted_and_scanner_error_paths() {
        let value = ContentTypeOwned::try_from("text/plain; Charset=utf-8; note=\"a\\\\b\"; third=value").expect("valid parameters");
        let parameters = value
            .parameters()
            .collect::<Result<Vec<_>, _>>()
            .expect("stored parameters remain valid");
        assert_eq!(parameters.len(), 3);
        assert_eq!(parameters[0].name(), "Charset");
        assert_eq!(parameters[0].value(), b"utf-8");
        assert_eq!(parameters[1].value(), b"\"a\\\\b\"");
        assert_eq!(value.parameter("charset"), Ok(Some(b"utf-8".as_slice())));
        assert_eq!(value.parameter("THIRD"), Ok(Some(b"value".as_slice())));
        assert_eq!(
            ContentTypeOwned::try_from("text/plain; ; charset=utf-8;")
                .expect("empty parameter slots are valid")
                .parameters()
                .count(),
            1
        );

        for wire in [
            "text",
            "text/",
            "/plain",
            "text plain",
            "text/plain trailing",
            "text/plain; name",
            "text/plain; =value",
            "text/plain; name=",
            "text/plain; name=\"unterminated",
            "text/plain; name=\"bad\\\n\"",
        ] {
            assert!(ContentTypeOwned::try_from(wire).is_err(), "{wire:?}");
        }
        assert!(ContentTypeOwned::try_from("text/plain; name =value").is_err());
        assert!(ContentTypeOwned::try_from("text/plain; name= value").is_err());

        let bytes = b"\"quoted\"";
        let mut position = 0;
        assert_eq!(take_parameter_value(bytes, &mut position), Ok(0..8));
        let mut position = 0;
        assert!(take_parameter_value(b"\"unterminated", &mut position).is_err());
        let mut position = 0;
        assert!(take_parameter_value(b"\"bad\\\n\"", &mut position).is_err());
        let mut position = 0;
        assert!(take_parameter_value(b"\"bad\n\"", &mut position).is_err());
    }

    #[test]
    fn relaxed_parsing_and_private_metadata_errors_are_covered() {
        assert!(parse_metadata_with(b"text / plain", DecodeMode::Strict).is_err());
        let relaxed = parse_metadata_with(b"text / plain", DecodeMode::Relaxed).expect("relaxed spacing");
        let head = relaxed.head();
        assert_eq!(head.type_end, 4);
        assert_eq!(head.subtype_start, 7);
        assert!(parse_metadata_with(b"bad", DecodeMode::Relaxed).is_err());
        assert!(parse_metadata_with(b"text/plain", DecodeMode::Relaxed).is_ok());
        assert!(parse_metadata_with(b"text / plain; charset=utf-8", DecodeMode::Relaxed).is_ok());

        assert!(common_metadata(b"unknown/type").is_none());
        assert!(matches!(
            common_metadata(b"application/json; charset=utf-8"),
            Some(ContentTypeMetadata::ApplicationJsonUtf8)
        ));
        assert!(matches!(
            common_metadata(b"text/html; charset=utf-8"),
            Some(ContentTypeMetadata::TextHtmlUtf8)
        ));

        assert!(component(b"abc", 4..5).is_err());
        assert_eq!(
            component(&[0xff], 0..1).expect_err("component is not UTF-8").kind(),
            DecodeErrorKind::InvalidUtf8
        );
        assert!(narrow_offset(u32::MAX as usize + 1).is_err());

        let bad_range = ParameterRange { name: 0..5, value: 6..7 };
        assert!(parameter_from_range(b"a=b", &bad_range).is_err());
        assert!(
            parameter_from_inline(
                b"a=b",
                InlineParameter {
                    name_start: 0,
                    name_end: 1,
                    value_start: 9,
                    value_end: 10,
                }
            )
            .is_err()
        );
        assert!(
            parameter_from_inline(
                b"a=b",
                InlineParameter {
                    name_start: 9,
                    name_end: 10,
                    value_start: 2,
                    value_end: 3,
                }
            )
            .is_err()
        );
        assert!(
            InlineParameter::from_range(&ParameterRange {
                name: 0..(u32::from(u16::MAX) + 1),
                value: 0..1,
            })
            .is_none()
        );

        let mut overflowing = ContentTypeHead {
            type_start: 0,
            type_end: 1,
            subtype_start: 2,
            subtype_end: 3,
            parameter_start: 3,
            parameter_count: u32::MAX,
            inline_parameters: [InlineParameter::EMPTY; 2],
            inline_count: 0,
        };
        assert!(overflowing.observe_parameter(&ParameterRange { name: 0..1, value: 2..3 }).is_err());
    }

    #[test]
    fn private_conversion_error_paths_are_covered() {
        let overflow = u32::MAX as usize + 1;
        assert!(narrow_parameter_range(overflow..overflow, 0..0).is_err());
        assert!(narrow_parameter_range(0..0, overflow..overflow).is_err());
        assert!(ContentTypeOwned::try_from(String::from("\n")).is_err());
        assert!(parameter_from_range(b"", &ParameterRange { name: 0..0, value: 1..2 }).is_err());
    }
}
