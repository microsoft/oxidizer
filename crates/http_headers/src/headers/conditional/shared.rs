// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{slice, str};

use crate::sink::EncodedValues;
use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef};

/// Last second representable by IMF-fixdate: 9999-12-31 23:59:59 UTC.
const MAX_HTTP_DATE_SECONDS: u64 = 253_402_300_799;

/// A borrowed entity tag from a conditional request field.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```
/// use http_headers::headers::{ConditionalTagView, IfMatchOwned};
///
/// let value = IfMatchOwned::try_from("\"revision\"")?;
/// let tag: ConditionalTagView<'_> = value.tags().next().expect("tag");
/// assert_eq!(tag.as_bytes(), b"\"revision\"");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct ConditionalTagView<'a> {
    pub(super) wire: &'a [u8],
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub(super) enum TagValues {
    One(FieldValue),
    Many(Vec<FieldValue>),
}

pub(super) trait OwnedTagSource {
    fn next_tag(&mut self) -> Option<crate::headers::ETagOwned>;
}

pub(super) struct OwnedTagSourceAdapter<I>(pub(super) I);

impl<I> OwnedTagSource for OwnedTagSourceAdapter<I>
where
    I: Iterator<Item = crate::headers::ETagOwned>,
{
    // LLVM emits an uncallable polymorphized instance for this type adapter.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn next_tag(&mut self) -> Option<crate::headers::ETagOwned> {
        self.0.next()
    }
}

pub(super) fn tag_values_from_source(name: &'static FieldName, tags: &mut dyn OwnedTagSource) -> Result<TagValues, DecodeError> {
    let Some(first) = tags.next_tag() else {
        return Err(DecodeError::new(name, DecodeErrorKind::MissingValue));
    };
    let first = first.into_field_value();
    let Some(second) = tags.next_tag() else {
        return Ok(TagValues::One(first));
    };
    let mut values = Vec::with_capacity(4);
    values.push(first);
    values.push(second.into_field_value());
    while let Some(tag) = tags.next_tag() {
        values.push(tag.into_field_value());
    }
    Ok(TagValues::Many(values))
}

impl TagValues {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Many(values) => values.len(),
        }
    }

    pub(super) fn iter(&self) -> slice::Iter<'_, FieldValue> {
        match self {
            Self::One(value) => slice::from_ref(value).iter(),
            Self::Many(values) => values.iter(),
        }
    }

    pub(super) fn into_encoded(self) -> EncodedValues {
        match self {
            Self::One(value) => EncodedValues::single(value),
            Self::Many(values) => EncodedValues::from_vec(values),
        }
    }
}

impl<'a> ConditionalTagView<'a> {
    /// Returns the complete wire-format entity tag.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::IfMatchOwned;
    ///
    /// let value = IfMatchOwned::try_from("\"revision\"")?;
    /// let tag = value.tags().next().expect("tag");
    /// assert_eq!(tag.as_bytes(), b"\"revision\"");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_bytes(self) -> &'a [u8] {
        self.wire
    }

    /// Returns the unquoted opaque tag.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::IfMatchOwned;
    ///
    /// let value = IfMatchOwned::try_from("W/\"revision\"")?;
    /// let tag = value.tags().next().expect("tag");
    /// assert_eq!(tag.opaque_tag(), b"revision");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn opaque_tag(self) -> &'a [u8] {
        let start = if self.is_weak() { 3 } else { 1 };
        let (_, opaque_and_quote) = self.wire.split_at(start);
        let (opaque, _) = opaque_and_quote.split_at(opaque_and_quote.len() - 1);
        opaque
    }

    /// Returns whether the tag uses the weak validator prefix.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::IfMatchOwned;
    ///
    /// let value = IfMatchOwned::try_from("W/\"revision\"")?;
    /// let tag = value.tags().next().expect("tag");
    /// assert!(tag.is_weak());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn is_weak(self) -> bool {
        matches!(self.wire.first(), Some(b'W' | b'w'))
    }
}

macro_rules! entity_tag_list_header {
    (
        $descriptor:ident,
        $owned:ident,
        $borrowed:ident,
        $header_name:literal,
        $constant:expr,
        $doc:literal,
        $view_doc:literal,
        $specification:literal,
        $examples:literal
    ) => {
        #[doc = concat!("Defines the `", $header_name, "` header.")]
        #[doc = ""]
        #[doc = "# Specification"]
        #[doc = ""]
        #[doc = $specification]
        #[derive(Debug)]
        pub struct $descriptor {
            _private: (),
        }

        #[doc = $doc]
        #[doc = ""]
        #[doc = "# Specification"]
        #[doc = ""]
        #[doc = $specification]
        #[doc = ""]
        #[doc = "# Examples"]
        #[doc = ""]
        #[doc = $examples]
        #[derive(Clone, Eq, Hash, PartialEq)]
        /// # Examples
        ///
        /// ```
        /// use http_headers::headers::IfMatchOwned;
        ///
        /// let value = IfMatchOwned::try_from("\"revision\"")?;
        /// let rendered = format!("{value:?}");
        /// assert!(rendered.contains("IfMatchOwned"));
        /// # Ok::<(), http_headers::DecodeError>(())
        /// ```
        pub struct $owned {
            values: super::shared::TagValues,
            wildcard: bool,
        }

        #[doc = $view_doc]
        /// # Examples
        ///
        /// ```
        /// # #[cfg(feature = "http")]
        /// # fn main() -> Result<(), http_headers::DecodeError> {
        /// use http::HeaderMap;
        /// use http_headers::Field;
        /// use http_headers::headers::IfMatch;
        ///
        /// let mut map = HeaderMap::new();
        /// map.insert("if-match", http::HeaderValue::from_static("\"revision\""));
        /// let value = IfMatch::view(&map)?.expect("present");
        /// let rendered = format!("{value:?}");
        /// assert!(rendered.contains("IfMatchView"));
        /// # Ok::<(), http_headers::DecodeError>(())
        /// # }
        /// # #[cfg(not(feature = "http"))]
        /// # fn main() {}
        /// ```
        pub struct $borrowed<'a> {
            values: FieldLines<'a>,
            wildcard: bool,
        }

        impl fmt::Debug for $owned {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($owned))
                    .field("value_count", &self.values.len())
                    .field("wildcard", &self.wildcard)
                    .finish()
            }
        }

        impl fmt::Debug for $borrowed<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($borrowed))
                    .field("value_count", &self.values.len())
                    .field("wildcard", &self.wildcard)
                    .finish()
            }
        }

        impl $owned {
            #[cfg(feature = "serde")]
            pub(crate) fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'_>> + '_ {
                self.values.iter().map(FieldValue::as_field_value_ref)
            }

            /// Constructs the wildcard form.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::IfMatchOwned;
            ///
            /// let value = IfMatchOwned::wildcard();
            /// assert!(value.is_wildcard());
            /// assert_eq!(value.tags().count(), 0);
            /// ```
            pub fn wildcard() -> Self {
                Self {
                    values: super::shared::TagValues::One(FieldValue::from_static("*")),
                    wildcard: true,
                }
            }

            /// Constructs a tag list without combining its field lines.
            ///
            /// # Errors
            ///
            /// Returns an error when no tags are supplied.
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::{ETagOwned, IfMatchOwned};
            ///
            /// let value = IfMatchOwned::from_tags([ETagOwned::strong("first")?, ETagOwned::weak("second")?])?;
            /// assert_eq!(value.tags().count(), 2);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            // LLVM emits an uncallable polymorphized instance for this adapter.
            #[cfg_attr(coverage_nightly, coverage(off))]
            pub fn from_tags<I>(tags: I) -> Result<Self, DecodeError>
            where
                I: IntoIterator<Item = $crate::headers::ETagOwned>,
            {
                let mut tags = super::shared::OwnedTagSourceAdapter(tags.into_iter());
                let stored = super::shared::tag_values_from_source($constant, &mut tags)?;
                Ok(Self {
                    values: stored,
                    wildcard: false,
                })
            }

            /// Returns whether this is the wildcard form.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::IfMatchOwned;
            ///
            /// let value = IfMatchOwned::wildcard();
            /// assert!(value.is_wildcard());
            /// ```
            pub const fn is_wildcard(&self) -> bool {
                self.wildcard
            }

            /// Iterates entity tags in field-line and list order.
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::IfMatchOwned;
            ///
            /// let value = IfMatchOwned::try_from("\"first\", W/\"second\"")?;
            /// let tags: Vec<_> = value.tags().map(|tag| tag.as_bytes()).collect();
            /// assert_eq!(
            ///     tags,
            ///     vec![b"\"first\"".as_slice(), b"W/\"second\"".as_slice()],
            /// );
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn tags(&self) -> impl Iterator<Item = ConditionalTagView<'_>> {
                TagIter::new(self.values.iter().map(FieldValue::as_field_value_ref))
            }
        }

        impl<'a> $borrowed<'a> {
            /// Returns whether this is the wildcard form.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::IfMatch;
            ///
            /// let mut map = HeaderMap::new();
            /// map.insert("if-match", http::HeaderValue::from_static("*"));
            /// let value = IfMatch::view(&map)?.expect("present");
            /// assert!(value.is_wildcard());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub const fn is_wildcard(&self) -> bool {
                self.wildcard
            }

            /// Iterates entity tags in field-line and list order.
            /// # Examples
            ///
            /// ```
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::IfMatch;
            ///
            /// let mut map = HeaderMap::new();
            /// map.insert(
            ///     "if-match",
            ///     http::HeaderValue::from_static("\"first\", W/\"second\""),
            /// );
            /// let value = IfMatch::view(&map)?.expect("present");
            /// let tags: Vec<_> = value.tags().map(|tag| tag.opaque_tag()).collect();
            /// assert_eq!(tags, vec![b"first".as_slice(), b"second".as_slice()]);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub fn tags(&self) -> impl Iterator<Item = ConditionalTagView<'a>> + '_ {
                TagIter::new(self.values.repeated())
            }
        }

        impl Field for $descriptor {
            type View<'a> = $borrowed<'a>;
            type Owned = $owned;

            fn name() -> &'static FieldName {
                $constant
            }

            fn view_with<S>(source: &S, mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
            where
                S: FieldSource + ?Sized,
            {
                let Some(lines) = source.lines(Self::name()) else {
                    return Ok(None);
                };
                lines.validate_entity_tag_item_limit()?;
                // The walk is spelled out here rather than shared with a
                // helper so that the validated view is assembled in place.
                let wildcard = {
                    let mut lines = lines.repeated();
                    let first = lines.next().expect("FieldLines always contains at least one field line");
                    let mut state = validate_tag_line_outlined_with(first.as_bytes(), TagListState::EMPTY, mode)
                        .ok_or_else(|| $crate::headers::invalid_syntax($constant).at_value(0))?;
                    let mut value_index = 1;
                    for value in lines {
                        state = validate_tag_line_outlined_with(value.as_bytes(), state, mode)
                            .ok_or_else(|| $crate::headers::invalid_syntax($constant).at_value(value_index))?;
                        value_index += 1;
                    }
                    state.finish($constant)?
                };
                Ok(Some($borrowed { values: lines, wildcard }))
            }

            fn owned_with<S>(source: &S, mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
            where
                S: FieldSource + ?Sized,
            {
                // Repeating this header over several field lines is rare, so
                // that walk lives out of line and leaves the one-line path
                // sole owner of the returned value.
                #[cold]
                #[inline(never)]
                fn many<'a>(
                    first: FieldValue,
                    second: FieldValueRef<'a>,
                    second_owned: FieldValue,
                    state: TagListState,
                    lines: impl Iterator<Item = (FieldValueRef<'a>, FieldValue)>,
                    mode: crate::DecodeMode,
                ) -> Result<$owned, DecodeError> {
                    let mut state = validate_tag_line_with(second.as_bytes(), state, mode)
                        .ok_or_else(|| $crate::headers::invalid_syntax($constant).at_value(1))?;
                    let mut storage = Vec::with_capacity(lines.size_hint().0.saturating_add(2));
                    storage.push(first);
                    storage.push(second_owned);
                    let mut value_index = 2;
                    for (value, owned) in lines {
                        state = validate_tag_line_with(value.as_bytes(), state, mode)
                            .ok_or_else(|| $crate::headers::invalid_syntax($constant).at_value(value_index))?;
                        storage.push(owned);
                        value_index += 1;
                    }
                    Ok($owned {
                        values: super::shared::TagValues::Many(storage),
                        wildcard: state.finish($constant)?,
                    })
                }

                let Some(lines) = source.lines(Self::name()) else {
                    return Ok(None);
                };
                lines.validate_entity_tag_item_limit()?;
                // One field line is the overwhelmingly common shape, so the
                // first line is validated and captured before the walk decides
                // whether any storage beyond the inline slot is needed.
                let mut lines = lines.repeated_owned()?;
                let (first, first_owned) = lines.next().expect("FieldLines always contains at least one field line");
                let state = validate_tag_line_with(first.as_bytes(), TagListState::EMPTY, mode)
                    .ok_or_else(|| $crate::headers::invalid_syntax($constant).at_value(0))?;
                let Some((second, second_owned)) = lines.next() else {
                    let wildcard = state.finish($constant)?;
                    return Ok(Some($owned {
                        values: super::shared::TagValues::One(first_owned),
                        wildcard,
                    }));
                };
                many(first_owned, second, second_owned, state, lines, mode).map(Some)
            }

            fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
            where
                S: FieldSink + ?Sized,
            {
                sink.set_values(Self::name(), value.values.into_encoded())
            }
        }

        impl TryFrom<&str> for $owned {
            type Error = DecodeError;

            fn try_from(wire: &str) -> Result<Self, Self::Error> {
                let value = FieldValue::from_str(wire).map_err(|_invalid| $crate::headers::invalid_syntax($constant))?;
                Self::try_from(value)
            }
        }

        impl TryFrom<String> for $owned {
            type Error = DecodeError;

            fn try_from(wire: String) -> Result<Self, Self::Error> {
                let value = FieldValue::try_from(wire).map_err(|_invalid| $crate::headers::invalid_syntax($constant))?;
                Self::try_from(value)
            }
        }

        impl TryFrom<FieldValue> for $owned {
            type Error = DecodeError;

            fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
                let wildcard = validate_tag_slice(value.as_bytes(), $constant)?;
                Ok(Self {
                    values: super::shared::TagValues::One(value),
                    wildcard,
                })
            }
        }
    };
}

pub(super) use entity_tag_list_header;

macro_rules! date_header {
    (
        $descriptor:ident,
        $owned:ident,
        $borrowed:ident,
        $header_name:literal,
        $constant:expr,
        $doc:literal,
        $view_doc:literal,
        $specification:literal,
        $examples:literal
    ) => {
        #[doc = concat!("Defines the `", $header_name, "` header.")]
        #[doc = ""]
        #[doc = "# Specification"]
        #[doc = ""]
        #[doc = $specification]
        #[derive(Debug)]
        pub struct $descriptor {
            _private: (),
        }

        #[doc = $doc]
        #[doc = ""]
        #[doc = "# Specification"]
        #[doc = ""]
        #[doc = $specification]
        #[doc = ""]
        #[doc = "# Examples"]
        #[doc = ""]
        #[doc = $examples]
        #[derive(Clone, Eq, Hash, PartialEq)]
        /// # Examples
        ///
        /// ```
        /// use http_headers::headers::IfModifiedSinceOwned;
        ///
        /// let value = IfModifiedSinceOwned::try_from("Sat, 29 Oct 1994 19:43:31 GMT")?;
        /// let rendered = format!("{value:?}");
        /// assert!(rendered.contains("IfModifiedSinceOwned"));
        /// # Ok::<(), http_headers::DecodeError>(())
        /// ```
        pub struct $owned {
            value: FieldValue,
            date: SystemTime,
        }

        #[doc = $view_doc]
        #[derive(Clone, Copy, Eq, Hash, PartialEq)]
        /// # Examples
        ///
        /// ```
        /// use http_headers::headers::IfModifiedSince;
        /// use http_headers::{FieldValue, SingleValueField};
        ///
        /// let field = FieldValue::from_static("Sat, 29 Oct 1994 19:43:31 GMT");
        /// let value = <IfModifiedSince as SingleValueField>::decode_view(field.as_field_value_ref())?;
        /// let rendered = format!("{value:?}");
        /// assert!(rendered.contains("IfModifiedSinceView"));
        /// # Ok::<(), http_headers::DecodeError>(())
        /// ```
        pub struct $borrowed<'a> {
            value: FieldValueRef<'a>,
            date: SystemTime,
        }

        impl fmt::Debug for $owned {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_struct(stringify!($owned))
                    .field("date", &self.date)
                    .finish_non_exhaustive()
            }
        }

        impl fmt::Debug for $borrowed<'_> {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_struct(stringify!($borrowed))
                    .field("date", &self.date)
                    .finish_non_exhaustive()
            }
        }

        impl $owned {
            /// Constructs a canonical IMF-fixdate value.
            ///
            /// Subsecond precision is omitted on the wire.
            ///
            /// # Errors
            ///
            /// Returns an error when the date cannot be represented by `httpdate`.
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::IfModifiedSinceOwned;
            ///
            /// let instant = std::time::UNIX_EPOCH + std::time::Duration::from_secs(783_459_811);
            /// let value = IfModifiedSinceOwned::new(instant)?;
            /// assert_eq!(value.date(), instant);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn new(date: SystemTime) -> Result<Self, DecodeError> {
                super::shared::format_http_date($constant, date).and_then(Self::try_from)
            }

            /// Returns the parsed date.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::IfModifiedSinceOwned;
            ///
            /// let instant = std::time::UNIX_EPOCH + std::time::Duration::from_secs(783_459_811);
            /// let value = IfModifiedSinceOwned::try_from("Sat, 29 Oct 1994 19:43:31 GMT")?;
            /// assert_eq!(value.date(), instant);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub const fn date(&self) -> SystemTime {
                self.date
            }

            /// Returns the preserved field value.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::IfModifiedSinceOwned;
            ///
            /// let value = IfModifiedSinceOwned::try_from("Sat, 29 Oct 1994 19:43:31 GMT")?;
            /// assert_eq!(value.as_field_value(), "Sat, 29 Oct 1994 19:43:31 GMT");
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn as_field_value(&self) -> &FieldValue {
                &self.value
            }

            /// Consumes the header and returns its field value.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::IfModifiedSinceOwned;
            ///
            /// let value = IfModifiedSinceOwned::try_from("Sat, 29 Oct 1994 19:43:31 GMT")?;
            /// let field = value.into_field_value();
            /// assert_eq!(field, "Sat, 29 Oct 1994 19:43:31 GMT");
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn into_field_value(self) -> FieldValue {
                self.into()
            }
        }

        crate::headers::shared::impl_field_value_conversion!($owned, |value| value.value);

        impl<'a> $borrowed<'a> {
            /// Returns the parsed date.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::IfModifiedSince;
            /// use http_headers::{FieldValue, SingleValueField};
            ///
            /// let instant = std::time::UNIX_EPOCH + std::time::Duration::from_secs(783_459_811);
            /// let field = FieldValue::from_static("Sat, 29 Oct 1994 19:43:31 GMT");
            /// let value = <IfModifiedSince as SingleValueField>::decode_view(field.as_field_value_ref())?;
            /// assert_eq!(value.date(), instant);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub const fn date(self) -> SystemTime {
                self.date
            }

            /// Returns the borrowed field value.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::IfModifiedSince;
            /// use http_headers::{FieldValue, SingleValueField};
            ///
            /// let field = FieldValue::from_static("Sat, 29 Oct 1994 19:43:31 GMT");
            /// let value = <IfModifiedSince as SingleValueField>::decode_view(field.as_field_value_ref())?;
            /// assert_eq!(value.as_field_value(), field.as_field_value_ref());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub const fn as_field_value(self) -> FieldValueRef<'a> {
                self.value
            }
        }

        impl SingleValueField for $descriptor {
            type View<'a> = $borrowed<'a>;
            type Owned = $owned;

            fn name() -> &'static FieldName {
                $constant
            }

            fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
                Ok($borrowed {
                    value,
                    date: parse_http_date($constant, value)?,
                })
            }

            fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
                $owned::try_from(value)
            }

            fn decode_view_with(value: FieldValueRef<'_>, mode: crate::DecodeMode) -> Result<Self::View<'_>, DecodeError> {
                Ok($borrowed {
                    value,
                    date: super::shared::parse_http_date_with($constant, value, mode)?,
                })
            }

            fn decode_owned_with(value: FieldValue, mode: crate::DecodeMode) -> Result<Self::Owned, DecodeError> {
                let date = super::shared::parse_http_date_with($constant, value.as_field_value_ref(), mode)?;
                Ok($owned { value, date })
            }

            fn as_field_value(value: &Self::Owned) -> &FieldValue {
                &value.value
            }

            fn into_field_value(value: Self::Owned) -> FieldValue {
                value.value
            }
        }

        impl TryFrom<&str> for $owned {
            type Error = DecodeError;

            fn try_from(wire: &str) -> Result<Self, Self::Error> {
                let value = FieldValue::from_str(wire).map_err(|_invalid| $crate::headers::invalid_syntax($constant))?;
                Self::try_from(value)
            }
        }

        impl TryFrom<String> for $owned {
            type Error = DecodeError;

            fn try_from(wire: String) -> Result<Self, Self::Error> {
                let value = FieldValue::try_from(wire).map_err(|_invalid| $crate::headers::invalid_syntax($constant))?;
                Self::try_from(value)
            }
        }

        impl TryFrom<FieldValue> for $owned {
            type Error = DecodeError;

            fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
                let date = parse_http_date($constant, value.as_field_value_ref())?;
                Ok(Self { value, date })
            }
        }
    };
}

pub(super) use date_header;

pub(super) struct TagIter<'a, I> {
    values: I,
    current: Option<&'a [u8]>,
    position: usize,
}

impl<'a, I> TagIter<'a, I>
where
    I: Iterator<Item = FieldValueRef<'a>>,
{
    pub(super) fn new(values: I) -> Self {
        Self {
            values,
            current: None,
            position: 0,
        }
    }
}

impl<'a, I> Iterator for TagIter<'a, I>
where
    I: Iterator<Item = FieldValueRef<'a>>,
{
    type Item = ConditionalTagView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(bytes) = self.current {
                match next_list_item(bytes, &mut self.position) {
                    Some(ListItem::Tag(tag)) => return Some(tag),
                    Some(ListItem::Wildcard) => continue,
                    Some(ListItem::Malformed) => return None,
                    None => self.current = None,
                }
            }
            self.current = Some(self.values.next()?.as_bytes());
            self.position = 0;
        }
    }
}

/// One item recognized by [`next_list_item`].
enum ListItem<'a> {
    Tag(ConditionalTagView<'a>),
    Wildcard,
    Malformed,
}

/// Scans one comma-delimited list item starting at `position`.
///
/// Empty items are skipped as required by the list grammar, so `None` means
/// the field line held no further items.
fn next_list_item<'a>(bytes: &'a [u8], position: &mut usize) -> Option<ListItem<'a>> {
    let length = bytes.len();
    let mut index = *position;

    while index < length && matches!(bytes[index], b' ' | b'\t' | b',') {
        index += 1;
    }
    if index == length {
        *position = index;
        return None;
    }

    let start = index;
    let item = if bytes[index] == b'*' {
        index += 1;
        ListItem::Wildcard
    } else {
        let weak = matches!(bytes[index], b'W' | b'w');
        if weak {
            index += 1;
            if bytes.get(index) != Some(&b'/') {
                *position = length;
                return Some(ListItem::Malformed);
            }
            index += 1;
        }
        if bytes.get(index) != Some(&b'"') {
            *position = length;
            return Some(ListItem::Malformed);
        }
        index += 1;
        while index < length && valid_opaque_byte(bytes[index]) {
            index += 1;
        }
        if bytes.get(index) != Some(&b'"') {
            *position = length;
            return Some(ListItem::Malformed);
        }
        index += 1;
        ListItem::Tag(ConditionalTagView {
            wire: &bytes[start..index],
        })
    };

    while index < length && matches!(bytes[index], b' ' | b'\t') {
        index += 1;
    }
    if index < length && bytes[index] != b',' {
        *position = length;
        return Some(ListItem::Malformed);
    }
    *position = index;
    Some(item)
}

pub(super) fn validate_tag_slice(bytes: &[u8], name: &'static FieldName) -> Result<bool, DecodeError> {
    validate_tag_line(bytes, TagListState::EMPTY)
        .ok_or_else(|| crate::headers::invalid_syntax(name))?
        .finish(name)
}

/// Whether a wildcard and whether any entity tag has been seen so far.
#[derive(Clone, Copy)]
pub(super) struct TagListState(u8);

impl TagListState {
    pub(super) const EMPTY: Self = Self(0);

    /// Set once the field has named the wildcard.
    const WILDCARD: u8 = 0b01;

    /// Set once the field has named an entity tag.
    const TAGGED: u8 = 0b10;

    /// Returns whether the field is the wildcard form, rejecting an empty list.
    pub(super) fn finish(self, name: &'static FieldName) -> Result<bool, DecodeError> {
        if self.0 == 0 {
            Err(DecodeError::new(name, DecodeErrorKind::MissingValue))
        } else {
            Ok(self.0 & Self::WILDCARD != 0)
        }
    }
}

/// Byte classes used by the entity-tag list scanner.
///
/// `OWS` marks SP and HTAB, `COMMA` marks the list delimiter, and `QUOTE`
/// marks DQUOTE. A zero class is exactly a byte that may appear inside an
/// opaque tag, so one table load classifies every byte the scanner meets.
/// Independent bits let one byte carry every scanner role without branches.
const TAG_OWS: u8 = 0b0_0001;
const TAG_COMMA: u8 = 0b0_0010;
const TAG_QUOTE: u8 = 0b0_0100;
const TAG_STAR: u8 = 0b0_1000;
const TAG_WEAK: u8 = 0b1_0000;
const TAG_RELAXED_WEAK: u8 = 0b10_0000;

const TAG_CLASS: [u8; 256] = {
    let mut table = [0_u8; 256];
    table[b'\t' as usize] = TAG_OWS;
    table[b' ' as usize] = TAG_OWS;
    table[b',' as usize] = TAG_COMMA;
    table[b'"' as usize] = TAG_QUOTE;
    table[b'*' as usize] = TAG_STAR;
    table[b'W' as usize] = TAG_WEAK;
    table[b'w' as usize] = TAG_RELAXED_WEAK;
    table
};

/// Validates one field line of the `"*" / 1#entity-tag` grammar in a single
/// pass, accumulating the list state across the field lines of one field.
///
/// Returns `None` when the line violates the grammar or repeats a wildcard.
#[inline]
pub(super) fn validate_tag_line(bytes: &[u8], state: TagListState) -> Option<TagListState> {
    validate_tag_line_with(bytes, state, crate::DecodeMode::Strict)
}

#[inline]
pub(super) fn validate_tag_line_with(bytes: &[u8], state: TagListState, mode: crate::DecodeMode) -> Option<TagListState> {
    let mut seen = state.0;
    let length = bytes.len();
    let mut index = 0;

    'line: loop {
        // The class of the byte that opens the item also selects the item
        // shape, so the scan never re-examines a byte it has classified.
        let class = loop {
            match bytes.get(index) {
                None => break 'line,
                Some(&byte) => {
                    let class = TAG_CLASS[byte as usize];
                    if class & (TAG_OWS | TAG_COMMA) == 0 {
                        break class;
                    }
                    index += 1;
                }
            }
        };

        if class & TAG_QUOTE != 0 {
            index += 1;
            seen |= TagListState::TAGGED;
        } else if class & TAG_WEAK != 0 || class & TAG_RELAXED_WEAK != 0 && mode == crate::DecodeMode::Relaxed {
            if bytes.get(index + 1..index + 3) != Some(b"/\"".as_slice()) {
                return None;
            }
            index += 3;
            seen |= TagListState::TAGGED;
        } else if class & TAG_STAR != 0 {
            if seen & TagListState::WILDCARD != 0 {
                return None;
            }
            seen |= TagListState::WILDCARD;
            index += 1;
            continue;
        } else {
            return None;
        }

        while index < length && TAG_CLASS[bytes[index] as usize] & (TAG_OWS | TAG_QUOTE) == 0 {
            index += 1;
        }
        if bytes.get(index) != Some(&b'"') {
            return None;
        }
        index += 1;

        while index < length && TAG_CLASS[bytes[index] as usize] & TAG_OWS != 0 {
            index += 1;
        }
        match bytes.get(index) {
            None => break,
            Some(&b',') => index += 1,
            Some(_) => return None,
        }
    }

    // A wildcard is only ever legal on its own, which one test at the end of
    // the line decides just as well as a test on every list item.
    if seen == TagListState::WILDCARD | TagListState::TAGGED {
        return None;
    }
    Some(TagListState(seen))
}

/// Validates one field line out of line.
///
/// A borrowed decode keeps nothing but the field lines alive, so calling the
/// scan rather than inlining it leaves that frame with a shorter prologue than
/// the scan's own register demand would otherwise force on it.
#[inline(never)]
pub(super) fn validate_tag_line_outlined_with(bytes: &[u8], state: TagListState, mode: crate::DecodeMode) -> Option<TagListState> {
    validate_tag_line_with(bytes, state, mode)
}

/// Returns whether `byte` is a legal `etagc`.
///
/// Every caller scans field-value bytes, which already exclude the controls
/// and DEL the entity-tag grammar forbids, so HTAB, SP, and DQUOTE are the
/// only bytes left to reject.
const fn valid_opaque_byte(byte: u8) -> bool {
    !matches!(byte, b'\t' | b' ' | b'"')
}

pub(super) fn parse_http_date(name: &'static FieldName, value: FieldValueRef<'_>) -> Result<SystemTime, DecodeError> {
    let bytes = value.as_bytes();
    if let Some(seconds) = parse_imf_fixdate(bytes) {
        return Ok(UNIX_EPOCH + Duration::from_secs(seconds));
    }
    if bytes.first().is_some_and(|byte| matches!(byte, b' ' | b'\t')) || bytes.last().is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        return Err(crate::headers::invalid_syntax(name));
    }
    let wire = str::from_utf8(bytes).map_err(|_invalid| crate::headers::invalid_syntax(name))?;
    httpdate::parse_http_date(wire).map_err(|_invalid| crate::headers::invalid_syntax(name))
}

pub(super) fn parse_http_date_with(
    name: &'static FieldName,
    value: FieldValueRef<'_>,
    mode: crate::DecodeMode,
) -> Result<SystemTime, DecodeError> {
    if mode == crate::DecodeMode::Strict {
        return parse_http_date(name, value);
    }
    if let Ok(date) = parse_http_date(name, value) {
        return Ok(date);
    }
    let wire = str::from_utf8(value.as_bytes())
        .map_err(|_invalid| crate::headers::invalid_syntax(name))?
        .trim_matches([' ', '\t']);
    parse_relaxed_http_date(wire).ok_or_else(|| crate::headers::invalid_syntax(name))
}

fn parse_relaxed_http_date(wire: &str) -> Option<SystemTime> {
    if let Ok(date) = httpdate::parse_http_date(wire) {
        return Some(date);
    }
    let mut parts = wire.split(' ');
    let weekday = parts.next()?.strip_suffix(',')?;
    let day = parse_short_decimal(parts.next()?, 2)?;
    let month = parts.next()?;
    let year = parse_short_decimal(parts.next()?, 4)?;
    let time = parts.next()?;
    let zone = parts.next()?;
    if parts.next().is_some()
        || !matches!(weekday, "Sun" | "Mon" | "Tue" | "Wed" | "Thu" | "Fri" | "Sat")
        || !matches!(
            month,
            "Jan" | "Feb" | "Mar" | "Apr" | "May" | "Jun" | "Jul" | "Aug" | "Sep" | "Oct" | "Nov" | "Dec"
        )
        || !matches!(zone, "GMT" | "UTC")
        || !(1970..=9999).contains(&year)
    {
        return None;
    }
    let mut clock = time.split(':');
    let hour = parse_short_decimal(clock.next()?, 2)?;
    let minute = parse_short_decimal(clock.next()?, 2)?;
    let second = parse_short_decimal(clock.next()?, 2)?;
    if clock.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let normalized = format!("{weekday}, {day:02} {month} {year:04} {hour:02}:{minute:02}:{second:02} GMT");
    httpdate::parse_http_date(&normalized).ok()
}

fn parse_short_decimal(value: &str, max_digits: usize) -> Option<u16> {
    if value.is_empty() || value.len() > max_digits || !value.as_bytes().iter().all(u8::is_ascii_digit) {
        return None;
    }
    value.parse().ok()
}

/// Days elapsed in a non-leap year before the first of each month.
const MONTH_START_DAY: [u16; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

/// Number of days in each month of a non-leap year.
const MONTH_LENGTH: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// Parses the preferred `IMF-fixdate` form, returning seconds since the epoch.
///
/// Returns `None` for anything this fast path does not fully recognize, which
/// leaves the general `httpdate` parser to accept or reject the value. Every
/// value accepted here is accepted by `httpdate` with the same instant.
/// Reads the four bytes at `offset` as one little-endian word.
const fn word_at(bytes: &[u8; 29], offset: usize) -> u32 {
    u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

/// Builds the word a four-byte name occupies in an `IMF-fixdate`.
#[cfg_attr(coverage_nightly, coverage(off))]
const fn name_word(name: [u8; 4]) -> u32 {
    u32::from_le_bytes(name)
}

const SUN: u32 = name_word(*b"Sun,");
const MON: u32 = name_word(*b"Mon,");
const TUE: u32 = name_word(*b"Tue,");
const WED: u32 = name_word(*b"Wed,");
const THU: u32 = name_word(*b"Thu,");
const FRI: u32 = name_word(*b"Fri,");
const SAT: u32 = name_word(*b"Sat,");

const JAN: u32 = name_word(*b"Jan ");
const FEB: u32 = name_word(*b"Feb ");
const MAR: u32 = name_word(*b"Mar ");
const APR: u32 = name_word(*b"Apr ");
const MAY: u32 = name_word(*b"May ");
const JUN: u32 = name_word(*b"Jun ");
const JUL: u32 = name_word(*b"Jul ");
const AUG: u32 = name_word(*b"Aug ");
const SEP: u32 = name_word(*b"Sep ");
const OCT: u32 = name_word(*b"Oct ");
const NOV: u32 = name_word(*b"Nov ");
const DEC: u32 = name_word(*b"Dec ");

fn parse_imf_fixdate(bytes: &[u8]) -> Option<u64> {
    // `Sun, 06 Nov 1994 08:49:37 GMT`
    let bytes: &[u8; 29] = bytes.try_into().ok()?;
    if bytes[4] != b' ' || bytes[16] != b' ' || bytes[19] != b':' {
        return None;
    }
    if bytes[22] != b':' || bytes[25] != b' ' || bytes[26] != b'G' || bytes[27] != b'M' {
        return None;
    }
    if bytes[28] != b'T' {
        return None;
    }

    // Matching four bytes at a time turns each name table into one load and a
    // switch over integers, where a slice pattern would compare byte runs arm
    // by arm. The trailing delimiter rides along inside the word, so the
    // comma after the weekday and the spaces around the month need no
    // separate test.
    let weekday = match word_at(bytes, 0) {
        SUN => 0,
        MON => 1,
        TUE => 2,
        WED => 3,
        THU => 4,
        FRI => 5,
        SAT => 6,
        _ => return None,
    };
    if bytes[7] != b' ' {
        return None;
    }
    let month = match word_at(bytes, 8) {
        JAN => 1_u16,
        FEB => 2,
        MAR => 3,
        APR => 4,
        MAY => 5,
        JUN => 6,
        JUL => 7,
        AUG => 8,
        SEP => 9,
        OCT => 10,
        NOV => 11,
        DEC => 12,
        _ => return None,
    };

    let day = two_digits(bytes[5], bytes[6])?;
    let hour = two_digits(bytes[17], bytes[18])?;
    let minute = two_digits(bytes[20], bytes[21])?;
    let second = two_digits(bytes[23], bytes[24])?;
    let year = u16::from(two_digits(bytes[12], bytes[13])?) * 100 + u16::from(two_digits(bytes[14], bytes[15])?);

    if hour > 23 || minute > 59 || second > 59 || !(1970..=9999).contains(&year) {
        return None;
    }

    let month_index = usize::from(month - 1);
    let leap_year = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_length = MONTH_LENGTH[month_index] + u8::from(leap_year && month == 2);
    if day == 0 || day > month_length {
        return None;
    }

    let previous_year = year - 1;
    let leap_days = (previous_year - 1968) / 4 - (previous_year - 1900) / 100 + (previous_year - 1600) / 400;
    let year_day = MONTH_START_DAY[month_index] + u16::from(day) + u16::from(leap_year && month > 2) - 1;
    let days = u64::from(year - 1970) * 365 + u64::from(leap_days) + u64::from(year_day);

    // The epoch fell on a Thursday, which is index 4 in the Sunday-based table.
    if (days + 4) % 7 != weekday {
        return None;
    }

    Some(u64::from(second) + u64::from(minute) * 60 + u64::from(hour) * 3600 + days * 86_400)
}

fn two_digits(high: u8, low: u8) -> Option<u8> {
    let high = high.wrapping_sub(b'0');
    let low = low.wrapping_sub(b'0');
    (high < 10 && low < 10).then(|| high * 10 + low)
}

pub(super) fn format_http_date(name: &'static FieldName, date: SystemTime) -> Result<FieldValue, DecodeError> {
    let elapsed = date
        .duration_since(UNIX_EPOCH)
        .map_err(|_before_epoch| DecodeError::new(name, DecodeErrorKind::InvalidNumber))?;
    if elapsed.as_secs() > MAX_HTTP_DATE_SECONDS {
        return Err(DecodeError::new(name, DecodeErrorKind::InvalidNumber));
    }
    Ok(FieldValue::try_from(httpdate::fmt_http_date(date)).expect("an HTTP date contains only valid field-value bytes"))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::iter;
    use std::time::{Duration, UNIX_EPOCH};

    use super::{
        MAX_HTTP_DATE_SECONDS, TagIter, parse_http_date, parse_http_date_with, parse_imf_fixdate, parse_relaxed_http_date,
        parse_short_decimal, two_digits, validate_tag_line_with,
    };
    use crate::headers::{
        ETagOwned, IfMatch, IfMatchOwned, IfModifiedSince, IfModifiedSinceOwned, IfNoneMatch, IfNoneMatchOwned, IfUnmodifiedSince,
        IfUnmodifiedSinceOwned, LastModified, LastModifiedOwned,
    };
    use crate::sink::{EncodedValues, FieldSink, InsertError};
    use crate::source::{FieldLines, FieldSource};
    use crate::{DecodeErrorKind, DecodeMode, Field, FieldName, FieldValue, SingleValueField};

    struct Source {
        name: &'static FieldName,
        values: Vec<FieldValue>,
    }

    impl FieldSource for Source {
        fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
            (name == self.name).then(|| FieldLines::from_slice(name, &self.values)).flatten()
        }
    }

    impl FieldSink for Source {
        fn set_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
            self.name = name;
            self.values = values.into_iter().collect();
            Ok(())
        }

        fn append_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
            if self.name == name {
                self.values.extend(values);
            } else {
                self.name = name;
                self.values = values.into_iter().collect();
            }
            Ok(())
        }

        fn remove_values(&mut self, name: &'static FieldName) {
            if self.name == name {
                self.values.clear();
            }
        }
    }

    #[test]
    fn date_headers_reject_malformed_duplicate_and_out_of_range_values() {
        let error = LastModifiedOwned::try_from("not a date").expect_err("malformed date");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);

        let source = Source {
            name: &FieldName::IfModifiedSince,
            values: vec![
                FieldValue::from_static("Sun, 06 Nov 1994 08:49:37 GMT"),
                FieldValue::from_static("Mon, 07 Nov 1994 08:49:37 GMT"),
            ],
        };
        let error = IfModifiedSince::view(&source).expect_err("singleton duplication must fail");
        assert_eq!(error.kind(), DecodeErrorKind::UnexpectedMultipleValues);

        let too_late = UNIX_EPOCH
            .checked_add(Duration::from_secs(MAX_HTTP_DATE_SECONDS + 1))
            .expect("platform represents test date");
        let error = LastModifiedOwned::new(too_late).expect_err("year 10000 must fail");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidNumber);
        let error = IfModifiedSinceOwned::new(too_late).expect_err("year 10000 must fail");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidNumber);
        let error = IfUnmodifiedSinceOwned::new(too_late).expect_err("year 10000 must fail");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidNumber);
        let error = LastModifiedOwned::new(UNIX_EPOCH - Duration::from_secs(1)).expect_err("pre-epoch date must fail");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidNumber);
    }

    #[test]
    fn entity_tag_headers_cover_construction_decoding_and_storage() {
        let strong = ETagOwned::try_from("\"strong\"").expect("strong tag");
        let weak = ETagOwned::try_from("W/\"weak\"").expect("weak tag");

        let missing = IfMatchOwned::from_tags(Vec::<ETagOwned>::new()).expect_err("a tag list must not be empty");
        assert_eq!(missing.kind(), DecodeErrorKind::MissingValue);

        let one = IfMatchOwned::from_tags([strong.clone()]).expect("one tag");
        assert!(!one.is_wildcard());
        assert_eq!(one.tags().next().expect("tag").opaque_tag(), b"strong");
        assert!(format!("{one:?}").contains("value_count"));

        let many = IfNoneMatchOwned::from_tags([strong, weak, ETagOwned::try_from("\"last\"").expect("tag")]).expect("several tags");
        assert!(format!("{many:?}").contains("value_count"));
        let tags: Vec<_> = many.tags().collect();
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[0].as_bytes(), b"\"strong\"");
        assert!(tags[1].is_weak());
        assert_eq!(tags[1].opaque_tag(), b"weak");

        let wildcard = IfMatchOwned::wildcard();
        assert!(wildcard.is_wildcard());
        assert_eq!(wildcard.tags().count(), 0);
        let mut wildcard_sink = Source {
            name: &FieldName::Accept,
            values: Vec::new(),
        };
        IfMatch::insert(&mut wildcard_sink, wildcard).expect("insert wildcard");
        assert_eq!(wildcard_sink.values, [FieldValue::from_static("*")]);

        let from_string = IfMatchOwned::try_from(String::from("\"owned\"")).expect("owned tag string");
        assert_eq!(from_string.tags().next().expect("tag").opaque_tag(), b"owned");
        let from_value = IfMatchOwned::try_from(FieldValue::from_static("\"field\"")).expect("tag field value");
        assert_eq!(from_value.tags().next().expect("tag").opaque_tag(), b"field");

        let mut sink = Source {
            name: &FieldName::Accept,
            values: Vec::new(),
        };
        IfNoneMatch::insert(&mut sink, many).expect("insert tag fields");
        assert_eq!(sink.name, &FieldName::IfNoneMatch);
        assert_eq!(sink.values.len(), 3);

        let view = IfNoneMatch::view(&sink).expect("decode view").expect("present");
        assert!(!view.is_wildcard());
        assert_eq!(view.tags().count(), 3);
        assert!(format!("{view:?}").contains("value_count"));

        let owned = IfNoneMatch::owned(&sink).expect("decode owned").expect("present");
        assert_eq!(owned.tags().count(), 3);
        let mut round_trip = Source {
            name: &FieldName::Accept,
            values: Vec::new(),
        };
        IfNoneMatch::insert(&mut round_trip, owned).expect("reinsert owned tags");
        assert_eq!(round_trip.values, sink.values, "wire field lines are retained");

        let absent = Source {
            name: &FieldName::Accept,
            values: Vec::new(),
        };
        assert!(IfMatch::view(&absent).expect("absent header").is_none());
        assert!(IfMatch::owned(&absent).expect("absent header").is_none());
    }

    #[test]
    fn entity_tag_headers_report_line_and_mode_errors() {
        let malformed_first = Source {
            name: &FieldName::IfMatch,
            values: vec![FieldValue::from_static("not-a-tag")],
        };
        assert_eq!(
            IfMatch::view(&malformed_first).expect_err("malformed first line").value_index(),
            Some(0)
        );
        assert_eq!(
            IfMatch::owned(&malformed_first).expect_err("malformed first line").value_index(),
            Some(0)
        );

        let malformed_later = Source {
            name: &FieldName::IfMatch,
            values: vec![
                FieldValue::from_static("\"one\""),
                FieldValue::from_static("\"two\""),
                FieldValue::from_static("bad"),
            ],
        };
        assert_eq!(
            IfMatch::view(&malformed_later).expect_err("malformed third line").value_index(),
            Some(2)
        );
        assert_eq!(
            IfMatch::owned(&malformed_later).expect_err("malformed third line").value_index(),
            Some(2)
        );

        for (wire, kind) in [
            ("", DecodeErrorKind::MissingValue),
            ("*, \"tag\"", DecodeErrorKind::InvalidSyntax),
            ("*, *", DecodeErrorKind::InvalidSyntax),
            ("Wtag", DecodeErrorKind::InvalidSyntax),
            ("\"unterminated", DecodeErrorKind::InvalidSyntax),
        ] {
            let error = IfMatchOwned::try_from(wire).expect_err("invalid tag list");
            assert_eq!(error.kind(), kind, "{wire:?}");
        }
        let error = IfMatchOwned::try_from(String::from("bad\nvalue")).expect_err("invalid field-value bytes");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);

        let relaxed = Source {
            name: &FieldName::IfNoneMatch,
            values: vec![FieldValue::from_static("w/\"weak\"")],
        };
        IfNoneMatch::view(&relaxed).expect_err("strict lowercase weak tag");
        let view = IfNoneMatch::view_with(&relaxed, DecodeMode::Relaxed)
            .expect("relaxed weak tag")
            .expect("present");
        assert!(view.tags().next().expect("tag").is_weak());
        assert!(
            IfNoneMatch::owned_with(&relaxed, DecodeMode::Relaxed)
                .expect("relaxed weak tag")
                .expect("present")
                .tags()
                .next()
                .expect("tag")
                .is_weak()
        );

        let tagged = validate_tag_line_with(b"\"tag\"", super::TagListState::EMPTY, DecodeMode::Strict).expect("valid tag");
        assert!(validate_tag_line_with(b"*", tagged, DecodeMode::Strict).is_none());
        assert!(validate_tag_line_with(b"\"one\" \t, \"two\"", super::TagListState::EMPTY, DecodeMode::Strict,).is_some());
        assert!(validate_tag_line_with(b"\"tag\" suffix", super::TagListState::EMPTY, DecodeMode::Strict,).is_none());

        assert_eq!(
            IfMatchOwned::try_from("bad\nvalue")
                .expect_err("invalid borrowed field bytes")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            IfNoneMatchOwned::try_from("bad\nvalue")
                .expect_err("invalid borrowed field bytes")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn both_entity_tag_header_variants_exercise_generated_apis() {
        let source = Source {
            name: &FieldName::IfMatch,
            values: vec![FieldValue::from_static("\"one\", W/\"two\"")],
        };
        let view = IfMatch::view(&source).expect("If-Match view").expect("present");
        assert!(!view.is_wildcard());
        assert_eq!(view.tags().count(), 2);
        assert!(format!("{view:?}").contains("value_count"));
        let owned = IfMatch::owned(&source).expect("If-Match owned").expect("present");
        assert_eq!(owned.tags().count(), 2);

        let wildcard = IfNoneMatchOwned::wildcard();
        assert!(wildcard.is_wildcard());
        assert_eq!(wildcard.tags().count(), 0);
        let mut sink = Source {
            name: &FieldName::Accept,
            values: Vec::new(),
        };
        IfNoneMatch::insert(&mut sink, wildcard).expect("insert If-None-Match wildcard");
        let view = IfNoneMatch::view(&sink).expect("wildcard view").expect("present");
        assert!(view.is_wildcard());
        assert_eq!(view.tags().count(), 0);

        assert_eq!(
            IfNoneMatchOwned::try_from("\"borrowed\"").expect("borrowed string").tags().count(),
            1
        );
        assert_eq!(
            IfNoneMatchOwned::try_from(String::from("\"owned\""))
                .expect("owned string")
                .tags()
                .count(),
            1
        );
        assert_eq!(
            IfNoneMatchOwned::try_from(FieldValue::from_static("\"field\""))
                .expect("field value")
                .tags()
                .count(),
            1
        );
    }

    #[test]
    fn repeated_tag_line_errors_report_exact_indices_for_both_headers() {
        let if_match = Source {
            name: &FieldName::IfMatch,
            values: vec![FieldValue::from_static("\"valid\""), FieldValue::from_static("invalid")],
        };
        assert_eq!(
            IfMatch::owned(&if_match).expect_err("invalid second If-Match line").value_index(),
            Some(1)
        );

        let if_none_match = Source {
            name: &FieldName::IfNoneMatch,
            values: vec![
                FieldValue::from_static("\"first\""),
                FieldValue::from_static("\"second\""),
                FieldValue::from_static("invalid"),
            ],
        };
        assert_eq!(
            IfNoneMatch::owned(&if_none_match)
                .expect_err("invalid third If-None-Match line")
                .value_index(),
            Some(2)
        );
        assert_eq!(
            IfNoneMatch::view(&if_none_match)
                .expect_err("invalid third If-None-Match line")
                .value_index(),
            Some(2)
        );
    }

    #[test]
    fn tag_iterator_stops_safely_on_malformed_preserved_input() {
        let values = [
            FieldValue::from_static("*, \"one\""),
            FieldValue::from_static("w/\"two\", \"three\""),
            FieldValue::from_static("Wbad"),
            FieldValue::from_static("\"unreached\""),
        ];
        let tags: Vec<_> = TagIter::new(values.iter().map(FieldValue::as_field_value_ref)).collect();
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[0].opaque_tag(), b"one");
        assert_eq!(tags[1].opaque_tag(), b"two");
        assert_eq!(tags[2].opaque_tag(), b"three");

        for malformed in ["bare", "\"open", "\"tag\" suffix"] {
            let value = FieldValue::from_str(malformed).expect("valid field bytes");
            assert_eq!(TagIter::new(iter::once(value.as_field_value_ref())).count(), 0);
        }
    }

    #[test]
    fn date_headers_cover_constructors_accessors_and_decode_modes() {
        let instant = httpdate::parse_http_date("Tue, 08 Nov 1994 08:49:37 GMT").expect("reference date");
        let last = LastModifiedOwned::new(instant).expect("representable date");
        assert_eq!(last.date(), instant);
        assert_eq!(last.as_field_value().as_bytes(), b"Tue, 08 Nov 1994 08:49:37 GMT");
        assert!(format!("{last:?}").contains("date"));

        let from_string = IfModifiedSinceOwned::try_from(String::from("Tue, 08 Nov 1994 08:49:37 GMT")).expect("date string");
        assert_eq!(from_string.date(), instant);
        let from_value = IfUnmodifiedSinceOwned::try_from(FieldValue::from_static("Tue, 08 Nov 1994 08:49:37 GMT")).expect("date field");
        assert_eq!(from_value.date(), instant);

        let view = <LastModified as SingleValueField>::decode_view(last.as_field_value().as_field_value_ref()).expect("date view");
        assert_eq!(view.date(), instant);
        assert_eq!(view.as_field_value(), last.as_field_value().as_field_value_ref());
        assert!(format!("{view:?}").contains("date"));

        let owned = <LastModified as SingleValueField>::decode_owned(last.clone().into_field_value()).expect("owned date");
        assert_eq!(<LastModified as SingleValueField>::as_field_value(&owned), last.as_field_value());
        assert_eq!(
            <LastModified as SingleValueField>::into_field_value(owned),
            last.clone().into_field_value()
        );
        assert_eq!(<LastModified as SingleValueField>::name(), &FieldName::LastModified);

        let relaxed = FieldValue::from_static(" Tue, 8 Nov 1994 8:49:37 UTC ");
        <IfModifiedSince as SingleValueField>::decode_view(relaxed.as_field_value_ref()).expect_err("strict noncanonical date");
        let view = <IfModifiedSince as SingleValueField>::decode_view_with(relaxed.as_field_value_ref(), DecodeMode::Relaxed)
            .expect("relaxed date view");
        assert_eq!(view.date(), instant);
        let owned = <IfModifiedSince as SingleValueField>::decode_owned_with(relaxed, DecodeMode::Relaxed).expect("relaxed owned date");
        assert_eq!(owned.date(), instant);
        let strict_owned = <IfModifiedSince as SingleValueField>::decode_owned_with(
            FieldValue::from_static("Tue, 08 Nov 1994 08:49:37 GMT"),
            DecodeMode::Strict,
        )
        .expect("strict owned date");
        assert_eq!(strict_owned.date(), instant);
    }

    #[test]
    fn every_date_header_variant_exercises_generated_apis() {
        macro_rules! exercise {
            ($descriptor:ty, $owned:ty, $name:expr) => {{
                let wire = "Tue, 08 Nov 1994 08:49:37 GMT";
                let instant = httpdate::parse_http_date(wire).expect("reference date");
                let constructed = <$owned>::new(instant).expect("constructed date");
                assert_eq!(constructed.date(), instant);
                assert_eq!(constructed.as_field_value(), wire);
                assert!(format!("{constructed:?}").contains("date"));
                assert_eq!(constructed.clone().into_field_value(), wire);

                assert_eq!(<$owned>::try_from(wire).expect("borrowed date").date(), instant);
                assert_eq!(<$owned>::try_from(String::from(wire)).expect("owned date").date(), instant);
                assert_eq!(
                    <$owned>::try_from(FieldValue::from_static(wire)).expect("field date").date(),
                    instant
                );
                assert_eq!(
                    <$owned>::try_from("bad\nvalue")
                        .expect_err("invalid borrowed field bytes")
                        .kind(),
                    DecodeErrorKind::InvalidSyntax
                );
                assert_eq!(
                    <$owned>::try_from(String::from("bad\nvalue"))
                        .expect_err("invalid owned field bytes")
                        .kind(),
                    DecodeErrorKind::InvalidSyntax
                );

                assert_eq!(<$descriptor as SingleValueField>::name(), $name);
                let field = FieldValue::from_static(wire);
                let view = <$descriptor as SingleValueField>::decode_view(field.as_field_value_ref()).expect("date view");
                assert_eq!(view.date(), instant);
                assert_eq!(view.as_field_value(), wire);
                assert!(format!("{view:?}").contains("date"));
                assert_eq!(
                    <$descriptor as SingleValueField>::decode_view_with(field.as_field_value_ref(), DecodeMode::Relaxed,)
                        .expect("explicit-mode date view")
                        .date(),
                    instant
                );

                let owned = <$descriptor as SingleValueField>::decode_owned(field.clone()).expect("decoded owned date");
                assert_eq!(<$descriptor as SingleValueField>::as_field_value(&owned), &field);
                assert_eq!(<$descriptor as SingleValueField>::into_field_value(owned), field);
                assert_eq!(
                    <$descriptor as SingleValueField>::decode_owned_with(FieldValue::from_static(wire), DecodeMode::Relaxed,)
                        .expect("explicit-mode owned date")
                        .date(),
                    instant
                );
                assert_eq!(
                    <$descriptor as SingleValueField>::decode_owned_with(FieldValue::from_static("not a date"), DecodeMode::Relaxed,)
                        .expect_err("invalid explicit-mode date")
                        .kind(),
                    DecodeErrorKind::InvalidSyntax
                );
            }};
        }

        exercise!(LastModified, LastModifiedOwned, &FieldName::LastModified);
        exercise!(IfModifiedSince, IfModifiedSinceOwned, &FieldName::IfModifiedSince);
        exercise!(IfUnmodifiedSince, IfUnmodifiedSinceOwned, &FieldName::IfUnmodifiedSince);
    }

    #[test]
    fn private_date_parsers_cover_fast_and_fallback_branches() {
        let dates = [
            "Sat, 01 Jan 2000 00:00:00 GMT",
            "Sun, 06 Nov 1994 08:49:37 GMT",
            "Mon, 07 Feb 2000 00:00:00 GMT",
            "Tue, 08 Mar 2005 00:00:00 GMT",
            "Wed, 09 Apr 2008 00:00:00 GMT",
            "Thu, 10 May 2012 00:00:00 GMT",
            "Fri, 11 Jun 2021 00:00:00 GMT",
            "Sat, 12 Jul 2014 00:00:00 GMT",
            "Sun, 13 Aug 2017 00:00:00 GMT",
            "Fri, 14 Sep 2018 00:00:00 GMT",
            "Tue, 15 Oct 2019 00:00:00 GMT",
            "Mon, 16 Nov 2020 00:00:00 GMT",
            "Fri, 17 Dec 2021 00:00:00 GMT",
            "Thu, 29 Feb 2024 00:00:00 GMT",
        ];
        for wire in dates {
            let value = FieldValue::from_str(wire).expect("date field");
            assert_eq!(
                parse_http_date(&FieldName::LastModified, value.as_field_value_ref()).expect("valid date"),
                httpdate::parse_http_date(wire).expect("reference date")
            );
        }

        for wire in [
            "short",
            " Sun, 06 Nov 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 08:49:37 GMT ",
            "Sun. 06 Nov 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 08-49:37 GMT",
            "Sun, 06 Nov 1994 08:49-37 GMT",
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Sun, 06XNov 1994 08:49:37 GMT",
            "Bad, 06 Nov 1994 08:49:37 GMT",
            "Sun, 06 Xxx 1994 08:49:37 GMT",
            "Sun, 00 Nov 1994 08:49:37 GMT",
            "Sun, 31 Nov 1994 08:49:37 GMT",
            "Sun, 06 Nov 1969 08:49:37 GMT",
            "Sun, 06 Nov 1994 24:49:37 GMT",
            "Sun, 06 Nov 1994 08:60:37 GMT",
            "Sun, 06 Nov 1994 08:49:60 GMT",
            "Mon, 06 Nov 1994 08:49:37 GMT",
            "Sun, x6 Nov 1994 08:49:37 GMT",
        ] {
            assert!(parse_imf_fixdate(wire.as_bytes()).is_none(), "{wire}");
        }

        assert!(parse_relaxed_http_date("Tue, 8 Nov 1994 8:49:37 UTC").is_some());
        assert!(parse_relaxed_http_date("Tue, 08 Nov 1994 08:49:37 GMT").is_some());
        for wire in [
            "Tue 8 Nov 1994 8:49:37 UTC",
            "Bad, 8 Nov 1994 8:49:37 UTC",
            "Tue, 8 Bad 1994 8:49:37 UTC",
            "Tue, 8 Nov 1969 8:49:37 UTC",
            "Tue, 8 Nov 1994 8:49:37 PST",
            "Tue, 8 Nov 1994 24:49:37 UTC",
            "Tue, 8 Nov 1994 8:60:37 UTC",
            "Tue, 8 Nov 1994 8:49:60 UTC",
            "Tue, 8 Nov 1994 8:49:37 UTC extra",
        ] {
            assert!(parse_relaxed_http_date(wire).is_none(), "{wire}");
        }
        assert_eq!(parse_short_decimal("42", 2), Some(42));
        assert_eq!(parse_short_decimal("", 2), None);
        assert_eq!(parse_short_decimal("123", 2), None);
        assert_eq!(parse_short_decimal("x", 2), None);
        assert_eq!(two_digits(b'4', b'2'), Some(42));
        assert_eq!(two_digits(b'x', b'2'), None);

        let canonical = FieldValue::from_static("Tue, 08 Nov 1994 08:49:37 GMT");
        assert_eq!(
            parse_http_date_with(&FieldName::LastModified, canonical.as_field_value_ref(), DecodeMode::Relaxed,)
                .expect("canonical relaxed date"),
            httpdate::parse_http_date("Tue, 08 Nov 1994 08:49:37 GMT").expect("reference date")
        );
        assert!(parse_imf_fixdate(b"Sun, 06 Nov 1994 08:49:37 GMX").is_none());

        let invalid_utf8 = FieldValue::from_bytes([0xff]).expect("field byte is permitted");
        parse_http_date_with(&FieldName::LastModified, invalid_utf8.as_field_value_ref(), DecodeMode::Relaxed)
            .expect_err("invalid UTF-8 date");

        let mut sink = Source {
            name: &FieldName::LastModified,
            values: vec![FieldValue::from_static("date")],
        };
        sink.remove_values(&FieldName::Accept);
        assert_eq!(sink.values.len(), 1);
        sink.remove_values(&FieldName::LastModified);
        assert!(sink.values.is_empty());
    }
}
