// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::{iter, slice};

use http_headers_simd::{EmptyMembers, TokenListScan, scan_token_list};

#[cfg(test)]
use super::super::tokens::common_header_name;
pub(super) use super::super::tokens::common_method;
use super::super::{FieldNameView, MethodView};
use crate::sink::EncodedValues;
use crate::source::FieldLines;
use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef, validate};

/// One byte is an RFC 9110 token byte.
const CLASS_TOKEN: u8 = 1 << 0;
/// One byte is optional whitespace.
pub(super) const CLASS_OWS: u8 = 1 << 1;
/// One byte is an ASCII decimal digit.
pub(super) const CLASS_DIGIT: u8 = 1 << 2;
/// One byte is permitted anywhere in a serialized domain label.
pub(super) const CLASS_DOMAIN: u8 = 1 << 3;
/// One byte is permitted at the edges of a serialized domain label.
pub(super) const CLASS_LABEL_EDGE: u8 = 1 << 4;
/// One byte is permitted in a serialized authority outside `:`.
pub(super) const CLASS_AUTHORITY: u8 = 1 << 5;
/// One byte is a hexadecimal digit of an IPv6 address in its serialized form.
pub(super) const CLASS_HEX: u8 = 1 << 6;
/// One byte is permitted in the scheme of a serialized origin.
pub(super) const CLASS_SCHEME: u8 = 1 << 7;

/// Byte classes shared by every CORS parser in this module.
pub(super) static BYTE_CLASS: [u8; 256] = {
    let mut table = [0_u8; 256];
    let mut byte = 0_u8;
    loop {
        let mut class = 0_u8;
        if validate::token_byte(byte) {
            class |= CLASS_TOKEN;
        }
        if byte == b' ' || byte == b'\t' {
            class |= CLASS_OWS;
        }
        if byte.is_ascii_digit() {
            class |= CLASS_DIGIT | CLASS_DOMAIN | CLASS_LABEL_EDGE | CLASS_HEX;
        }
        if byte.is_ascii_lowercase() {
            class |= CLASS_DOMAIN | CLASS_LABEL_EDGE;
        }
        if byte == b'-' {
            class |= CLASS_DOMAIN;
        }
        if byte.is_ascii() && !matches!(byte, b'/' | b'?' | b'#' | b'@' | b':') {
            class |= CLASS_AUTHORITY;
        }
        if matches!(byte, b'a'..=b'f') {
            class |= CLASS_HEX;
        }
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.') {
            class |= CLASS_SCHEME;
        }
        table[byte as usize] = class;
        if byte == u8::MAX {
            break;
        }
        byte += 1;
    }
    table
};

/// Returns the byte class at `index`, or no class when `index` is past the end.
#[inline]
#[cfg(test)]
fn byte_class(bytes: &[u8], index: usize) -> u8 {
    match bytes.get(index) {
        Some(byte) => BYTE_CLASS[usize::from(*byte)],
        None => 0,
    }
}

/// Owned list storage keeping the single-field-line case inline.
///
/// Multiple field lines are rare, so the one-line case avoids both the heap
/// allocation and the length word a growable buffer would need.
#[derive(Clone, Eq)]
pub(super) enum CorsList {
    One(FieldValue),
    Many(Vec<FieldValue>),
}

impl PartialEq for CorsList {
    fn eq(&self, other: &Self) -> bool {
        self.field_values().eq(other.field_values())
    }
}

impl Hash for CorsList {
    // LLVM emits an uncallable polymorphized instance for this hasher adapter.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn hash<H: Hasher>(&self, state: &mut H) {
        hash_cors_list(self, state);
    }
}

#[inline(never)]
fn hash_cors_list(list: &CorsList, mut state: &mut dyn Hasher) {
    for value in list.field_values() {
        value.hash(&mut state);
    }
}

pub(super) struct CorsListView<'a> {
    pub(super) values: FieldLines<'a>,
}

impl CorsList {
    #[inline]
    fn from_values(values: Vec<FieldValue>) -> Self {
        let mut values = values;
        if values.len() == 1 {
            Self::One(values.pop().expect("length checked above to contain exactly one value"))
        } else {
            Self::Many(values)
        }
    }

    #[inline]
    pub(super) fn field_values(&self) -> slice::Iter<'_, FieldValue> {
        match self {
            Self::One(value) => slice::from_ref(value).iter(),
            Self::Many(values) => values.iter(),
        }
    }

    #[inline]
    pub(super) fn value_count(&self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Many(values) => values.len(),
        }
    }

    pub(super) fn into_field_values(self) -> Vec<FieldValue> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }

    pub(super) fn into_encoded(self) -> EncodedValues {
        match self {
            Self::One(value) => EncodedValues::single(value),
            Self::Many(values) => EncodedValues::from_vec(values),
        }
    }

    // LLVM emits an uncallable polymorphized instance for this adapter.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub(super) fn from_items<I, S>(name: &'static FieldName, items: I, allow_empty: bool) -> Result<Self, DecodeError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let items = items.into_iter();
        let capacity = items.size_hint().0.saturating_mul(8);
        let mut items = ListItemSourceAdapter(items);
        list_from_source(name, &mut items, capacity, allow_empty)
    }

    pub(super) fn wildcard() -> Self {
        Self::One(FieldValue::from_static("*"))
    }

    pub(super) fn empty() -> Self {
        Self::One(FieldValue::from_static(""))
    }

    pub(super) fn from_field_values(name: &'static FieldName, values: Vec<FieldValue>, allow_empty: bool) -> Result<Self, DecodeError> {
        validate_list(name, values.iter().map(FieldValue::as_field_value_ref), allow_empty)?;
        Ok(Self::from_values(values))
    }

    pub(super) fn from_field_value(name: &'static FieldName, value: FieldValue, allow_empty: bool) -> Result<Self, DecodeError> {
        validate_single_list(name, value.as_field_value_ref(), allow_empty)?;
        Ok(Self::One(value))
    }

    /// Validates and adopts field lines in one pass over the map entry.
    ///
    /// Decoding straight to owned storage keeps a second walk off the owned
    /// decoding path.
    #[expect(clippy::inline_always, reason = "measured: fusing the decode into the caller saves 32 Ir")]
    #[inline(always)]
    pub(super) fn decode(name: &'static FieldName, values: &FieldLines<'_>, allow_empty: bool) -> Result<Self, DecodeError> {
        values.validate_list_item_limit(b',', true)?;
        let mut repeated = values.repeated_owned()?;
        let (first, first_owned) = repeated.next().expect("FieldLines always contains at least one field line");
        let Some(saw_item) = validate_list_value(first.as_bytes()) else {
            return Err(invalid_token(name).at_value(0));
        };
        if let Some((second, second_owned)) = repeated.next() {
            return Self::decode_rest(name, first_owned, second, second_owned, repeated, saw_item, allow_empty);
        }
        if !saw_item && !allow_empty {
            return Err(invalid_syntax(name));
        }
        Ok(Self::One(first_owned))
    }

    /// Handles the rare field line repetition split out of [`Self::decode`].
    // LLVM emits an uncallable polymorphized instance for this iterator adapter.
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[inline(never)]
    fn decode_rest<'a>(
        name: &'static FieldName,
        first_owned: FieldValue,
        second: FieldValueRef<'a>,
        second_owned: FieldValue,
        rest: impl Iterator<Item = (FieldValueRef<'a>, FieldValue)>,
        first_saw_item: bool,
        allow_empty: bool,
    ) -> Result<Self, DecodeError> {
        let mut rest = rest;
        Self::decode_rest_from_source(name, first_owned, second, second_owned, &mut rest, first_saw_item, allow_empty)
    }

    fn decode_rest_from_source<'a>(
        name: &'static FieldName,
        first_owned: FieldValue,
        second: FieldValueRef<'a>,
        second_owned: FieldValue,
        rest: &mut dyn Iterator<Item = (FieldValueRef<'a>, FieldValue)>,
        first_saw_item: bool,
        allow_empty: bool,
    ) -> Result<Self, DecodeError> {
        let mut saw_item = first_saw_item;
        let mut stored = Vec::with_capacity(2_usize.saturating_add(rest.size_hint().0));
        stored.push(first_owned);
        for (offset, (value, owned)) in iter::once((second, second_owned)).chain(rest).enumerate() {
            let Some(has_item) = validate_list_value(value.as_bytes()) else {
                return Err(invalid_token(name).at_value(offset.saturating_add(1)));
            };
            saw_item |= has_item;
            stored.push(owned);
        }
        if !saw_item && !allow_empty {
            return Err(invalid_syntax(name));
        }
        Ok(Self::Many(stored))
    }
}

struct ListBuilder {
    name: &'static FieldName,
    bytes: Vec<u8>,
    allow_empty: bool,
}

trait ListItemSource {
    fn next_with(&mut self, visitor: &mut dyn FnMut(&str) -> Result<(), DecodeError>) -> Result<bool, DecodeError>;
}

struct ListItemSourceAdapter<I>(I);

impl<I, S> ListItemSource for ListItemSourceAdapter<I>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    // LLVM emits an uncallable polymorphized instance for this type adapter.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn next_with(&mut self, visitor: &mut dyn FnMut(&str) -> Result<(), DecodeError>) -> Result<bool, DecodeError> {
        let Some(item) = self.0.next() else {
            return Ok(false);
        };
        visitor(item.as_ref())?;
        Ok(true)
    }
}

fn list_from_source(
    name: &'static FieldName,
    items: &mut dyn ListItemSource,
    capacity: usize,
    allow_empty: bool,
) -> Result<CorsList, DecodeError> {
    let mut builder = ListBuilder {
        name,
        bytes: Vec::with_capacity(capacity),
        allow_empty,
    };
    while items.next_with(&mut |item| append_list_item(&mut builder, item))? {}
    finish_list_items(builder)
}

fn append_list_item(builder: &mut ListBuilder, item: &str) -> Result<(), DecodeError> {
    validate_list_item(builder.name, item.as_bytes())?;
    if !builder.bytes.is_empty() {
        builder.bytes.extend_from_slice(b", ");
    }
    builder.bytes.extend_from_slice(item.as_bytes());
    Ok(())
}

fn finish_list_items(builder: ListBuilder) -> Result<CorsList, DecodeError> {
    if builder.bytes.is_empty() && !builder.allow_empty {
        return Err(invalid_syntax(builder.name));
    }
    Ok(CorsList::One(
        super::super::value_from_bytes(builder.name, builder.bytes)
            .expect("validated CORS list items contain only valid field-value bytes"),
    ))
}

pub(super) fn view_list_values<'a>(
    name: &'static FieldName,
    values: Option<FieldLines<'a>>,
    allow_empty: bool,
) -> Result<Option<CorsListView<'a>>, DecodeError> {
    let Some(values) = values else {
        return Ok(None);
    };
    values.validate_list_item_limit(b',', true)?;
    let mut repeated = values.repeated();
    let first = repeated.next().expect("FieldLines always contains at least one field line");
    if repeated.next().is_none() {
        validate_single_list(name, first, allow_empty)?;
        return Ok(Some(CorsListView { values }));
    }
    validate_list(name, values.repeated(), allow_empty)?;
    Ok(Some(CorsListView { values }))
}

pub(super) fn owned_list_values(
    name: &'static FieldName,
    values: Option<FieldLines<'_>>,
    allow_empty: bool,
) -> Result<Option<CorsList>, DecodeError> {
    let Some(values) = values else {
        return Ok(None);
    };
    CorsList::decode(name, &values, allow_empty).map(Some)
}

macro_rules! define_header_name_list {
    (
        $descriptor:ident,
        $owned:ident,
        $borrowed:ident,
        $header_name:literal,
        $name:expr,
        $allow_empty:expr,
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

        #[doc = concat!("Owned value for the `", $header_name, "` header.")]
        ///
        /// # Specification
        #[doc = $specification]
        ///
        /// # Examples
        ///
        /// ```
        /// use http_headers::headers::AccessControlAllowHeadersOwned;
        ///
        /// let value = AccessControlAllowHeadersOwned::from_header_names(["content-type"])?;
        /// let fmt = format!("{value:?}");
        /// assert!(fmt.contains("AccessControlAllowHeadersOwned"));
        /// assert!(fmt.contains("header_name_count"));
        /// # Ok::<(), http_headers::DecodeError>(())
        /// ```
        #[doc = $examples]
        #[derive(Clone, Eq, Hash, PartialEq)]
        pub struct $owned(CorsList);

        #[doc = concat!("Borrowed value for the `", $header_name, "` header.")]
        /// # Examples
        ///
        /// ```
        /// # #[cfg(feature = "http")]
        /// # fn main() -> Result<(), http_headers::DecodeError> {
        /// use http::HeaderMap;
        /// use http_headers::Field;
        /// use http_headers::headers::AccessControlAllowHeaders;
        ///
        /// let mut headers = HeaderMap::new();
        /// headers.insert(
        ///     "access-control-allow-headers",
        ///     http::HeaderValue::from_static("content-type"),
        /// );
        /// let value = AccessControlAllowHeaders::view(&headers)?.expect("present");
        /// let fmt = format!("{value:?}");
        /// assert!(fmt.contains("AccessControlAllowHeadersView"));
        /// assert!(fmt.contains("header_name_count"));
        /// # Ok::<(), http_headers::DecodeError>(())
        /// # }
        /// # #[cfg(not(feature = "http"))]
        /// # fn main() {}
        /// ```
        pub struct $borrowed<'a>(CorsListView<'a>);

        impl fmt::Debug for $owned {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($owned))
                    .field("value_count", &self.0.value_count())
                    .field("header_name_count", &self.len())
                    .finish()
            }
        }

        impl fmt::Display for $owned {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                crate::headers::shared::fmt_ascii_values(self.field_values(), f)
            }
        }

        impl fmt::Debug for $borrowed<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($borrowed))
                    .field("value_count", &self.0.values.len())
                    .field("header_name_count", &self.len())
                    .finish()
            }
        }

        impl $owned {
            /// Constructs one canonical field line from validated field names.
            ///
            /// Duplicate field names are retained in input order.
            ///
            /// # Errors
            ///
            /// Returns an error if an item is not an HTTP field-name token or
            /// if this header requires at least one member and input is empty.
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::AccessControlAllowHeadersOwned;
            ///
            /// let value = AccessControlAllowHeadersOwned::from_header_names(["content-type", "X-Trace-Id"])?;
            /// let names = value.iter().map(|name| name.as_str()).collect::<Vec<_>>();
            /// assert_eq!(names, ["content-type", "X-Trace-Id"]);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            // LLVM emits an uncallable polymorphized instance for this adapter.
            #[cfg_attr(coverage_nightly, coverage(off))]
            pub fn from_header_names<I, S>(names: I) -> Result<Self, DecodeError>
            where
                I: IntoIterator<Item = S>,
                S: AsRef<str>,
            {
                CorsList::from_items($name, names, $allow_empty).map(Self)
            }

            /// Validates and adopts complete field lines.
            ///
            /// # Errors
            ///
            /// Returns an error for no field lines, malformed members, or an
            /// empty list when this header requires at least one member.
            /// # Examples
            ///
            /// ```
            /// use http_headers::FieldValue;
            /// use http_headers::headers::AccessControlAllowHeadersOwned;
            ///
            /// let value = AccessControlAllowHeadersOwned::from_field_values(vec![FieldValue::from_static(
            ///     "content-type, x-trace-id",
            /// )])?;
            /// assert_eq!(value.len(), 2);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn from_field_values(values: Vec<FieldValue>) -> Result<Self, DecodeError> {
                CorsList::from_field_values($name, values, $allow_empty).map(Self)
            }

            /// Iterates field names in wire order without allocating.
            ///
            /// Duplicate names and their original casing are preserved.
            /// Names compare and hash case-insensitively through [`FieldNameView`].
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::AccessControlAllowHeadersOwned;
            ///
            /// let value = AccessControlAllowHeadersOwned::from_header_names(["content-type", "x-trace-id"])?;
            /// let names = value.iter().map(|name| name.as_str()).collect::<Vec<_>>();
            /// assert_eq!(names, ["content-type", "x-trace-id"]);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn iter(&self) -> super::CorsHeaderNames<'_> {
                super::CorsHeaderNames::new(self.0.field_values())
            }

            /// Returns the number of list members, including duplicates.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::AccessControlAllowHeadersOwned;
            ///
            /// let value = AccessControlAllowHeadersOwned::from_header_names([
            ///     "content-type",
            ///     "x-trace-id",
            ///     "content-type",
            /// ])?;
            /// assert_eq!(value.len(), 3);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn len(&self) -> usize {
                self.iter().count()
            }

            /// Returns whether the list has no members.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::AccessControlAllowHeadersOwned;
            ///
            /// let empty = AccessControlAllowHeadersOwned::empty();
            /// assert!(empty.is_empty());
            ///
            /// let value = AccessControlAllowHeadersOwned::from_header_names(["content-type"])?;
            /// assert!(!value.is_empty());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn is_empty(&self) -> bool {
                self.iter().next().is_none()
            }

            /// Iterates original field lines in wire order.
            /// # Examples
            ///
            /// ```
            /// use http_headers::FieldValue;
            /// use http_headers::headers::AccessControlAllowHeadersOwned;
            ///
            /// let value = AccessControlAllowHeadersOwned::try_from(FieldValue::from_static(
            ///     "content-type, x-trace-id",
            /// ))?;
            /// let fields = value
            ///     .field_values()
            ///     .map(|field| field.as_bytes())
            ///     .collect::<Vec<_>>();
            /// assert_eq!(fields, [b"content-type, x-trace-id".as_slice()]);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'_>> {
                self.0.field_values().map(FieldValue::as_field_value_ref)
            }

            /// Returns the original field lines.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// use http_headers::FieldValue;
            /// use http_headers::headers::AccessControlAllowHeadersOwned;
            ///
            /// let value = AccessControlAllowHeadersOwned::try_from(vec![
            ///     FieldValue::from_static("content-type"),
            ///     FieldValue::from_static("x-trace-id"),
            /// ])?;
            /// let fields = value.into_field_values();
            /// assert_eq!(fields.len(), 2);
            /// assert_eq!(fields[0].as_bytes(), b"content-type");
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn into_field_values(self) -> Vec<FieldValue> {
                self.0.into_field_values()
            }
        }

        impl<'a> IntoIterator for &'a $owned {
            type Item = FieldNameView<'a>;
            type IntoIter = super::CorsHeaderNames<'a>;

            fn into_iter(self) -> Self::IntoIter {
                self.iter()
            }
        }

        impl<'a> $borrowed<'a> {
            /// Iterates field names in wire order without allocating.
            ///
            /// Duplicate names and their original casing are preserved.
            /// Names compare and hash case-insensitively through [`FieldNameView`].
            /// # Examples
            ///
            /// ```
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::AccessControlAllowHeaders;
            ///
            /// let mut headers = HeaderMap::new();
            /// headers.insert(
            ///     "access-control-allow-headers",
            ///     http::HeaderValue::from_static("content-type, x-trace-id"),
            /// );
            /// let value = AccessControlAllowHeaders::view(&headers)?.expect("present");
            /// let names = value.iter().map(|name| name.as_str()).collect::<Vec<_>>();
            /// assert_eq!(names, ["content-type", "x-trace-id"]);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub fn iter(&self) -> impl Iterator<Item = FieldNameView<'a>> + '_ {
                self.0
                    .values
                    .repeated()
                    .flat_map(|value| value.as_bytes().split(|byte| *byte == b','))
                    .map(validate::trim_ows)
                    .filter(|item| !item.is_empty())
                    .map(super::shared::header_name_ref_validated)
            }

            /// Returns the number of list members, including duplicates.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::AccessControlAllowHeaders;
            ///
            /// let mut headers = HeaderMap::new();
            /// headers.insert(
            ///     "access-control-allow-headers",
            ///     http::HeaderValue::from_static("content-type"),
            /// );
            /// headers.append(
            ///     "access-control-allow-headers",
            ///     http::HeaderValue::from_static("x-trace-id"),
            /// );
            /// let value = AccessControlAllowHeaders::view(&headers)?.expect("present");
            /// assert_eq!(value.len(), 2);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub fn len(&self) -> usize {
                self.iter().count()
            }

            /// Returns whether the list has no members.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::AccessControlAllowHeaders;
            ///
            /// let mut headers = HeaderMap::new();
            /// headers.insert(
            ///     "access-control-allow-headers",
            ///     http::HeaderValue::from_static(""),
            /// );
            /// let value = AccessControlAllowHeaders::view(&headers)?.expect("present");
            /// assert!(value.is_empty());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub fn is_empty(&self) -> bool {
                self.iter().next().is_none()
            }

            /// Iterates original field lines in wire order.
            /// # Examples
            ///
            /// ```
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::AccessControlAllowHeaders;
            ///
            /// let mut headers = HeaderMap::new();
            /// headers.insert(
            ///     "access-control-allow-headers",
            ///     http::HeaderValue::from_static("content-type"),
            /// );
            /// headers.append(
            ///     "access-control-allow-headers",
            ///     http::HeaderValue::from_static("x-trace-id"),
            /// );
            /// let value = AccessControlAllowHeaders::view(&headers)?.expect("present");
            /// let fields = value
            ///     .field_values()
            ///     .map(|field| field.as_bytes())
            ///     .collect::<Vec<_>>();
            /// assert_eq!(
            ///     fields,
            ///     [b"content-type".as_slice(), b"x-trace-id".as_slice()]
            /// );
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'a>> + '_ {
                self.0.values.repeated()
            }
        }

        impl Field for $descriptor {
            type View<'a> = $borrowed<'a>;
            type Owned = $owned;

            fn name() -> &'static FieldName {
                $name
            }

            // LLVM emits an uncallable polymorphized instance for this source adapter.
            #[cfg_attr(coverage_nightly, coverage(off))]
            fn view_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
            where
                S: FieldSource + ?Sized,
            {
                super::shared::view_list_values($name, source.lines(Self::name()), $allow_empty).map(|view| view.map($borrowed))
            }

            // LLVM emits an uncallable polymorphized instance for this source adapter.
            #[cfg_attr(coverage_nightly, coverage(off))]
            fn owned_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
            where
                S: FieldSource + ?Sized,
            {
                super::shared::owned_list_values($name, source.lines(Self::name()), $allow_empty).map(|owned| owned.map($owned))
            }

            // LLVM emits an uncallable polymorphized instance for this sink adapter.
            #[cfg_attr(coverage_nightly, coverage(off))]
            fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
            where
                S: FieldSink + ?Sized,
            {
                sink.set_values(Self::name(), value.0.into_encoded())
            }
        }

        impl TryFrom<FieldValue> for $owned {
            type Error = DecodeError;

            fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
                CorsList::from_field_value($name, value, $allow_empty).map(Self)
            }
        }

        impl TryFrom<Vec<FieldValue>> for $owned {
            type Error = DecodeError;

            fn try_from(values: Vec<FieldValue>) -> Result<Self, Self::Error> {
                Self::from_field_values(values)
            }
        }
    };
}

pub(super) use define_header_name_list;

macro_rules! impl_header_name_wildcard {
    ($owned:ident, $borrowed:ident) => {
        impl $owned {
            /// Constructs the wildcard field value.
            ///
            /// Whether `*` has wildcard semantics depends on the request and
            /// is deliberately not inferred here.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::AccessControlAllowHeadersOwned;
            ///
            /// let wildcard = AccessControlAllowHeadersOwned::wildcard();
            /// assert!(wildcard.contains_wildcard());
            /// assert!(wildcard.is_wildcard());
            /// ```
            pub fn wildcard() -> Self {
                Self(CorsList::wildcard())
            }

            /// Returns whether any member is `*`.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::AccessControlAllowHeadersOwned;
            ///
            /// let wildcard = AccessControlAllowHeadersOwned::wildcard();
            /// assert!(wildcard.contains_wildcard());
            ///
            /// let explicit = AccessControlAllowHeadersOwned::from_header_names(["content-type"])?;
            /// assert!(!explicit.contains_wildcard());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn contains_wildcard(&self) -> bool {
                self.iter().any(|name| name.as_bytes() == b"*")
            }

            /// Returns whether `*` is the only list member.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// use http_headers::headers::AccessControlAllowHeadersOwned;
            ///
            /// let wildcard = AccessControlAllowHeadersOwned::wildcard();
            /// assert!(wildcard.is_wildcard());
            ///
            /// let mixed = AccessControlAllowHeadersOwned::from_header_names(["*", "content-type"])?;
            /// assert!(mixed.contains_wildcard());
            /// assert!(!mixed.is_wildcard());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn is_wildcard(&self) -> bool {
                let mut names = self.iter();
                names.next().is_some_and(|name| name.as_bytes() == b"*") && names.next().is_none()
            }
        }

        impl $borrowed<'_> {
            /// Returns whether any member is `*`.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::AccessControlAllowHeaders;
            ///
            /// let mut headers = HeaderMap::new();
            /// headers.insert(
            ///     "access-control-allow-headers",
            ///     http::HeaderValue::from_static("*, content-type"),
            /// );
            /// let value = AccessControlAllowHeaders::view(&headers)?.expect("present");
            /// assert!(value.contains_wildcard());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub fn contains_wildcard(&self) -> bool {
                self.iter().any(|name| name.as_bytes() == b"*")
            }

            /// Returns whether `*` is the only list member.
            #[must_use]
            /// # Examples
            ///
            /// ```
            /// # #[cfg(feature = "http")]
            /// # fn main() -> Result<(), http_headers::DecodeError> {
            /// use http::HeaderMap;
            /// use http_headers::Field;
            /// use http_headers::headers::AccessControlAllowHeaders;
            ///
            /// let mut headers = HeaderMap::new();
            /// headers.insert(
            ///     "access-control-allow-headers",
            ///     http::HeaderValue::from_static("*"),
            /// );
            /// let wildcard = AccessControlAllowHeaders::view(&headers)?.expect("present");
            /// assert!(wildcard.is_wildcard());
            ///
            /// let mut mixed_headers = HeaderMap::new();
            /// mixed_headers.insert(
            ///     "access-control-allow-headers",
            ///     http::HeaderValue::from_static("*, content-type"),
            /// );
            /// let mixed = AccessControlAllowHeaders::view(&mixed_headers)?.expect("present");
            /// assert!(!mixed.is_wildcard());
            /// # Ok::<(), http_headers::DecodeError>(())
            /// # }
            /// # #[cfg(not(feature = "http"))]
            /// # fn main() {}
            /// ```
            pub fn is_wildcard(&self) -> bool {
                let mut names = self.iter();
                names.next().is_some_and(|name| name.as_bytes() == b"*") && names.next().is_none()
            }
        }
    };
}

pub(super) use impl_header_name_wildcard;

#[expect(clippy::inline_always, reason = "preserves pre-split inlining in hot borrowed CORS decoders")]
#[inline(always)]
// LLVM emits an uncallable polymorphized instance for this iterator adapter.
#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn validate_list<'a>(
    name: &'static FieldName,
    values: impl IntoIterator<Item = FieldValueRef<'a>>,
    allow_empty: bool,
) -> Result<(), DecodeError> {
    let mut values = values.into_iter();
    validate_list_from_source(name, &mut values, allow_empty)
}

#[inline(never)]
fn validate_list_from_source(
    name: &'static FieldName,
    values: &mut dyn Iterator<Item = FieldValueRef<'_>>,
    allow_empty: bool,
) -> Result<(), DecodeError> {
    let Some(first) = values.next() else {
        return Err(DecodeError::new(name, DecodeErrorKind::MissingValue));
    };
    let Some(mut saw_item) = validate_list_value(first.as_bytes()) else {
        return Err(invalid_token(name).at_value(0));
    };
    for (value_index, value) in (1_usize..).zip(values) {
        let Some(has_item) = validate_list_value(value.as_bytes()) else {
            return Err(invalid_token(name).at_value(value_index));
        };
        saw_item |= has_item;
    }
    if !saw_item && !allow_empty {
        return Err(invalid_syntax(name));
    }
    Ok(())
}

/// Validates one comma-separated token list, reporting whether it has members.
///
/// Returns `None` when a member is not a token after optional whitespace was
/// trimmed. Empty members are skipped, matching `#rule` list expansion.
#[expect(clippy::inline_always, reason = "preserves pre-split inlining in hot CORS list decoding")]
#[inline(always)]
fn validate_list_value(bytes: &[u8]) -> Option<bool> {
    if let Some(members) = common_list_line(bytes) {
        return Some(members);
    }
    match scan_token_list(bytes, EmptyMembers::Skip) {
        TokenListScan::Members => Some(true),
        TokenListScan::Empty => Some(false),
        TokenListScan::Rejected => None,
    }
}

pub(super) fn validate_single_list(name: &'static FieldName, value: FieldValueRef<'_>, allow_empty: bool) -> Result<(), DecodeError> {
    let Some(saw_item) = validate_list_value(value.as_bytes()) else {
        return Err(invalid_token(name).at_value(0));
    };
    if !saw_item && !allow_empty {
        return Err(invalid_syntax(name));
    }
    Ok(())
}

/// Recognizes a short field line that is known to be well formed.
///
/// The lines carried by these headers repeat heavily, and matching one whole
/// settles it without a scan; anything else falls through to one.
#[expect(clippy::inline_always, reason = "measured: folds into the caller's dispatch")]
#[inline(always)]
fn common_list_line(bytes: &[u8]) -> Option<bool> {
    match bytes {
        b"*"
        | b"GET"
        | b"PUT"
        | b"POST"
        | b"HEAD"
        | b"PATCH"
        | b"DELETE"
        | b"OPTIONS"
        | b"GET, POST"
        | b"GET, HEAD"
        | b"content-type"
        | b"authorization"
        | b"content-type, x-request-id"
        | b"x-request-id, content-type"
        | b"etag, x-request-id"
        | b"content-type, authorization"
        | b"authorization, content-type"
        | b"content-type, x-requested-with" => Some(true),
        b"" => Some(false),
        _ => None,
    }
}

/// Shortest field line the shared list scanner pays for itself on.
///
/// The shared finder and token validator process long runs a vector block at
/// a time; below one block, the scalar state machine avoids their call cost.
#[cfg(test)]
const LIST_SCAN_MIN_LEN: usize = 16;

/// Checks one field line a byte at a time.
///
/// This scan carries field lines shorter than one vector block, so it is
/// deliberately small: whole blocks are the shared scanner's business.
#[expect(clippy::inline_always, reason = "measured: keeps the short-line scan in the caller")]
#[inline(always)]
#[cfg(test)]
fn scan_list_value(bytes: &[u8]) -> Option<bool> {
    let mut saw_item = false;
    let mut index = 0_usize;
    while index < bytes.len() {
        index = skip_ows(bytes, index);
        let start = index;
        while byte_class(bytes, index) & CLASS_TOKEN != 0 {
            index += 1;
        }
        saw_item |= index > start;
        index = skip_ows(bytes, index);
        match bytes.get(index) {
            None => break,
            Some(&b',') => index += 1,
            Some(_) => return None,
        }
    }
    Some(saw_item)
}

#[cfg(test)]
fn skip_ows(bytes: &[u8], mut index: usize) -> usize {
    while byte_class(bytes, index) & CLASS_OWS != 0 {
        index += 1;
    }
    index
}

fn validate_list_item(name: &'static FieldName, item: &[u8]) -> Result<(), DecodeError> {
    if item.is_empty() || !all_token_bytes(item) {
        return Err(invalid_token(name));
    }
    Ok(())
}

#[expect(clippy::inline_always, reason = "preserves pre-split inlining in hot CORS method iteration")]
#[inline(always)]
pub(super) fn method_ref(bytes: &[u8]) -> Option<MethodView<'_>> {
    if let Some(method) = common_method(bytes) {
        return Some(MethodView::from_validated(method.as_bytes()));
    }
    if bytes.is_empty() || !all_token_bytes(bytes) {
        return None;
    }
    Some(MethodView::from_validated(bytes))
}

#[inline]
pub(super) fn method_ref_validated(bytes: &[u8]) -> MethodView<'_> {
    MethodView::from_validated(bytes)
}

#[inline]
pub(super) fn header_name_ref_validated(bytes: &[u8]) -> FieldNameView<'_> {
    FieldNameView::from_validated(bytes)
}

#[cfg(test)]
fn header_name_ref(bytes: &[u8]) -> Option<FieldNameView<'_>> {
    if let Some(name) = common_header_name(bytes) {
        return Some(FieldNameView::from_validated(name.as_bytes()));
    }
    if bytes.is_empty() || !all_token_bytes(bytes) {
        return None;
    }
    Some(FieldNameView::from_validated(bytes))
}

#[expect(clippy::inline_always, reason = "preserves pre-split inlining in hot CORS list iteration")]
#[inline(always)]
fn all_token_bytes(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| BYTE_CLASS[usize::from(*byte)] & CLASS_TOKEN != 0)
}

pub(super) fn untrimmed_range(bytes: &[u8]) -> Option<Range<usize>> {
    let first = *bytes.first()?;
    let last = bytes[bytes.len() - 1];
    match (first, last) {
        (b' ' | b'\t', _) | (_, b' ' | b'\t') => None,
        _ => Some(0..bytes.len()),
    }
}

pub(super) fn trimmed_range(bytes: &[u8]) -> Range<usize> {
    if bytes.first().is_some_and(|byte| !matches!(byte, b' ' | b'\t')) && bytes.last().is_some_and(|byte| !matches!(byte, b' ' | b'\t')) {
        return 0..bytes.len();
    }
    let start = bytes.iter().position(|byte| !matches!(byte, b' ' | b'\t')).unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(start, |index| index + 1);
    start..end
}

#[cold]
pub(super) fn invalid_syntax(name: &'static FieldName) -> DecodeError {
    DecodeError::new(name, DecodeErrorKind::InvalidSyntax)
}

#[cold]
pub(super) fn invalid_token(name: &'static FieldName) -> DecodeError {
    DecodeError::new(name, DecodeErrorKind::InvalidToken)
}

#[cold]
pub(super) fn invalid_number(name: &'static FieldName) -> DecodeError {
    DecodeError::new(name, DecodeErrorKind::InvalidNumber)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::hash_map::DefaultHasher;

    use super::super::test_map::TestMap;
    use super::*;
    use crate::headers::{
        AccessControlAllowHeaders, AccessControlAllowHeadersOwned, AccessControlExposeHeaders, AccessControlExposeHeadersOwned,
        AccessControlRequestHeaders, AccessControlRequestHeadersOwned,
    };
    use crate::sink::FieldSink;

    /// Checks that the short lines settled from the table reach the verdict
    /// the scan would have reached.
    #[test]
    fn common_short_lines_match_the_scan() {
        const ALPHABET: &[u8] = b"GETPOS, *-\t\"";

        let known: &[&[u8]] = &[
            b"*",
            b"GET",
            b"PUT",
            b"POST",
            b"HEAD",
            b"PATCH",
            b"DELETE",
            b"OPTIONS",
            b"GET, POST",
            b"GET, HEAD",
            b"content-type",
            b"authorization",
            b"",
        ];
        for line in known {
            assert_eq!(common_list_line(line), scan_list_value(line), "table disagrees on {line:?}");
            assert!(line.len() < LIST_SCAN_MIN_LEN, "{line:?} is not a short line");
        }

        let mut line = Vec::new();
        for first in ALPHABET {
            for second in ALPHABET {
                for third in ALPHABET {
                    line.clear();
                    line.extend_from_slice(&[*first, *second, *third]);
                    if let Some(members) = common_list_line(&line) {
                        assert_eq!(Some(members), scan_list_value(&line), "table disagrees on {line:?}");
                    }
                }
            }
        }
    }

    /// Checks that handing long field lines to the shared scanner keeps the
    /// verdict the local scan would have reached.
    #[test]
    fn long_list_lines_scan_the_same_either_way() {
        /// Bytes a member is spelled with.
        const TOKEN: &[u8] = b"abcXYZ019-_.!";
        /// Bytes that leave the grammar, mutated into otherwise sound lines.
        const FOREIGN: &[u8] = b"\"@\x7f\r ,\t";

        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut line = Vec::with_capacity(256);
        let mut accepted = 0_usize;
        for _ in 0..20_000 {
            let length = LIST_SCAN_MIN_LEN + usize::try_from(next() % 160).unwrap_or_default();
            line.clear();
            while line.len() < length {
                if !line.is_empty() {
                    line.push(b',');
                }
                for _ in 0..next() % 3 {
                    line.push(if next() % 2 == 0 { b' ' } else { b'\t' });
                }
                for _ in 0..next() % 9 {
                    let index = usize::try_from(next() % TOKEN.len() as u64).unwrap_or_default();
                    line.push(TOKEN[index]);
                }
            }
            if next() % 3 == 0 {
                let at = usize::try_from(next() % line.len() as u64).unwrap_or_default();
                let index = usize::try_from(next() % FOREIGN.len() as u64).unwrap_or_default();
                line[at] = FOREIGN[index];
            }
            accepted += usize::from(validate_list_value(&line).is_some());
            assert_eq!(validate_list_value(&line), scan_list_value(&line));
        }
        assert!(
            accepted > 1_000,
            "the generated lines must exercise the accepting path, not only the fallback"
        );
    }

    #[test]
    fn byte_classes_and_view_conversions_cover_shared_value_helpers() {
        let table = BYTE_CLASS;
        assert_ne!(table[usize::from(b'A')] & CLASS_TOKEN, 0);
        assert_ne!(table[usize::from(b' ')] & CLASS_OWS, 0);
        assert_ne!(table[usize::from(b'9')] & CLASS_DIGIT, 0);
        assert_ne!(table[usize::from(b'a')] & CLASS_DOMAIN, 0);
        assert_ne!(table[usize::from(b'f')] & CLASS_HEX, 0);
        assert_ne!(table[usize::from(b'+')] & CLASS_SCHEME, 0);

        let method = MethodView::new("CUSTOM").unwrap();
        assert_eq!(method.as_str(), "CUSTOM");
        assert_eq!(method.as_bytes(), b"CUSTOM");
        #[cfg(feature = "http")]
        assert_eq!(method.try_to_method().expect("HTTP method").as_str(), "CUSTOM");

        let name = FieldNameView::new("X-Trace-Id").unwrap();
        assert_eq!(name.as_str(), "X-Trace-Id");
        assert_eq!(name.as_bytes(), b"X-Trace-Id");
        assert!(name.eq_ignore_ascii_case("x-trace-id"));
        assert_eq!(name.try_to_field_name().expect("native header name").as_str(), "x-trace-id");
        #[cfg(feature = "http")]
        assert_eq!(name.try_to_http_header_name().expect("HTTP header name").as_str(), "x-trace-id");
    }

    #[test]
    fn cors_list_storage_covers_one_many_equality_hashing_and_errors() {
        let one = CorsList::from_field_values(
            &FieldName::AccessControlAllowHeaders,
            vec![FieldValue::from_static("content-type")],
            true,
        )
        .expect("one field line");
        assert_eq!(one.value_count(), 1);
        assert_eq!(one.field_values().count(), 1);
        assert_eq!(one.clone().into_field_values().len(), 1);
        assert_eq!(one.clone().into_encoded().len(), 1);
        assert!(matches!(
            CorsList::from_field_values(
                &FieldName::AccessControlRequestHeaders,
                vec![FieldValue::from_static("")],
                false,
            ),
            Err(error) if error.kind() == DecodeErrorKind::InvalidSyntax
        ));

        let many = CorsList::from_field_values(
            &FieldName::AccessControlAllowHeaders,
            vec![FieldValue::from_static("content-type"), FieldValue::from_static("x-trace-id")],
            true,
        )
        .expect("several field lines");
        assert_eq!(many.value_count(), 2);
        assert_eq!(many.clone().into_field_values().len(), 2);
        assert!(many == many.clone());
        assert!(one != many);

        let mut first_hash = DefaultHasher::new();
        many.hash(&mut first_hash);
        let mut second_hash = DefaultHasher::new();
        many.clone().hash(&mut second_hash);
        assert_eq!(first_hash.finish(), second_hash.finish());
        let mut helper_hash = DefaultHasher::new();
        hash_cors_list(&many, &mut helper_hash);
        assert_eq!(first_hash.finish(), helper_hash.finish());
        assert_eq!(many.clone().into_encoded().len(), 2);

        assert_eq!(
            AccessControlRequestHeadersOwned::from_header_names(Vec::<String>::new())
                .expect_err("request list must not be empty")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            AccessControlAllowHeadersOwned::from_header_names(["valid", "bad name"])
                .expect_err("invalid token")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        let single = AccessControlAllowHeadersOwned::from_header_names(["content-type"]).expect("valid single-item list");
        assert_eq!(single.iter().next().expect("field name").as_str(), "content-type");
        let owned = AccessControlAllowHeadersOwned::from_header_names(vec![String::from("content-type")]).expect("valid owned item list");
        assert_eq!(owned.iter().next().expect("field name").as_str(), "content-type");
        assert_eq!(
            CorsList::from_field_values(&FieldName::AccessControlAllowHeaders, Vec::new(), true,)
                .err()
                .expect("a present header needs a field line")
                .kind(),
            DecodeErrorKind::MissingValue
        );
        assert_eq!(CorsList::wildcard().field_values().next().expect("line"), "*");
        assert_eq!(CorsList::empty().field_values().next().expect("line"), "");
    }

    #[test]
    fn allow_and_expose_header_lists_cover_wildcard_and_repeated_paths() {
        let allow =
            AccessControlAllowHeadersOwned::from_header_names(["content-type", "X-Trace-Id", "content-type"]).expect("header names");
        assert_eq!(allow.len(), 3);
        assert!(!allow.is_empty());
        assert_eq!(
            allow.iter().map(FieldNameView::as_str).collect::<Vec<_>>(),
            ["content-type", "X-Trace-Id", "content-type"]
        );
        assert_eq!(allow.field_values().count(), 1);
        assert!(format!("{allow:?}").contains("header_name_count"));

        let wildcard = AccessControlAllowHeadersOwned::wildcard();
        assert!(wildcard.contains_wildcard());
        assert!(wildcard.is_wildcard());
        let mixed = AccessControlAllowHeadersOwned::from_header_names(["*", "x-trace-id"]).expect("mixed wildcard list");
        assert!(mixed.contains_wildcard());
        assert!(!mixed.is_wildcard());
        assert!(AccessControlAllowHeadersOwned::empty().is_empty());
        assert!(AccessControlExposeHeadersOwned::empty().is_empty());
        let expose_wildcard = AccessControlExposeHeadersOwned::wildcard();
        assert!(expose_wildcard.contains_wildcard());
        assert!(expose_wildcard.is_wildcard());
        assert!(format!("{expose_wildcard:?}").contains("header_name_count"));
        let expose_mixed = AccessControlExposeHeadersOwned::from_header_names(["*", "x-trace-id"]).expect("mixed expose list");
        assert!(expose_mixed.contains_wildcard());
        assert!(!expose_mixed.is_wildcard());

        let repeated = TestMap::new(
            &FieldName::AccessControlExposeHeaders,
            vec![FieldValue::from_static("content-type"), FieldValue::from_static("*, x-trace-id")],
        );
        let view = AccessControlExposeHeaders::view(&repeated).expect("valid view").expect("present");
        assert_eq!(view.len(), 3);
        assert!(!view.is_empty());
        assert!(view.contains_wildcard());
        assert!(!view.is_wildcard());
        assert_eq!(view.field_values().count(), 2);
        assert!(format!("{view:?}").contains("header_name_count"));

        let expose_wildcard_map = TestMap::new(&FieldName::AccessControlExposeHeaders, vec![FieldValue::from_static("*")]);
        let expose_wildcard_view = AccessControlExposeHeaders::view(&expose_wildcard_map)
            .expect("valid wildcard view")
            .expect("present wildcard view");
        assert!(expose_wildcard_view.is_wildcard());

        let allow_wildcard_map = TestMap::new(&FieldName::AccessControlAllowHeaders, vec![FieldValue::from_static("*")]);
        let allow_wildcard_view = AccessControlAllowHeaders::view(&allow_wildcard_map)
            .expect("valid wildcard view")
            .expect("present wildcard view");
        assert!(allow_wildcard_view.is_wildcard());

        let owned = AccessControlExposeHeaders::owned(&repeated).expect("valid owned").expect("present");
        assert_eq!(owned.field_values().count(), 2);
        assert_eq!(owned.clone().into_field_values().len(), 2);

        let mut sink = TestMap::new(&FieldName::Accept, Vec::new());
        AccessControlExposeHeaders::insert(&mut sink, owned).expect("insert list");
        assert_eq!(sink.name, &FieldName::AccessControlExposeHeaders);
        assert_eq!(sink.values.len(), 2);
    }

    #[test]
    fn request_and_allow_header_lists_cover_generated_conversion_paths() {
        let required = AccessControlRequestHeadersOwned::try_from(vec![
            FieldValue::from_static("content-type"),
            FieldValue::from_static("x-trace-id"),
        ])
        .expect("required repeated list");
        assert_eq!(required.len(), 2);
        let from_one = AccessControlRequestHeadersOwned::try_from(FieldValue::from_static("content-type")).expect("one required name");
        assert_eq!(from_one.len(), 1);
        assert!(!from_one.is_empty());
        assert_eq!(from_one.field_values().count(), 1);
        assert!(format!("{from_one:?}").contains("header_name_count"));
        assert_eq!(from_one.clone().into_field_values().len(), 1);
        assert_eq!(
            AccessControlRequestHeaders::owned(&TestMap::new(
                &FieldName::AccessControlRequestHeaders,
                vec![FieldValue::from_static("content-type")],
            ))
            .expect("one-line owned list")
            .expect("present")
            .len(),
            1
        );

        let request_source = TestMap::new(
            &FieldName::AccessControlRequestHeaders,
            vec![FieldValue::from_static("content-type, x-trace-id")],
        );
        let request_view = AccessControlRequestHeaders::view(&request_source)
            .expect("request view")
            .expect("present");
        assert_eq!(request_view.len(), 2);
        assert!(!request_view.is_empty());
        assert_eq!(request_view.iter().count(), 2);
        assert_eq!(request_view.field_values().count(), 1);
        assert!(format!("{request_view:?}").contains("header_name_count"));
        let mut request_sink = TestMap::new(&FieldName::Accept, Vec::new());
        AccessControlRequestHeaders::insert(&mut request_sink, from_one).expect("insert request headers");
        assert_eq!(request_sink.name, &FieldName::AccessControlRequestHeaders);

        let allow_source = TestMap::new(
            &FieldName::AccessControlAllowHeaders,
            vec![FieldValue::from_static("*, content-type")],
        );
        let allow_view = AccessControlAllowHeaders::view(&allow_source)
            .expect("allow view")
            .expect("present");
        assert_eq!(allow_view.len(), 2);
        assert!(!allow_view.is_empty());
        assert!(allow_view.contains_wildcard());
        assert!(!allow_view.is_wildcard());
        assert_eq!(allow_view.field_values().count(), 1);
        assert!(format!("{allow_view:?}").contains("header_name_count"));

        let allow_from_field = AccessControlAllowHeadersOwned::try_from(FieldValue::from_static("content-type")).expect("allow field");
        assert_eq!(allow_from_field.len(), 1);
        let allow_from_fields =
            AccessControlAllowHeadersOwned::try_from(vec![FieldValue::from_static("content-type"), FieldValue::from_static("x-trace-id")])
                .expect("allow fields");
        assert_eq!(allow_from_fields.clone().into_field_values().len(), 2);
        let mut allow_sink = TestMap::new(&FieldName::Accept, Vec::new());
        AccessControlAllowHeaders::insert(&mut allow_sink, allow_from_fields).expect("insert allow headers");

        let expose_from_field = AccessControlExposeHeadersOwned::try_from(FieldValue::from_static("content-type")).expect("expose field");
        assert_eq!(expose_from_field.len(), 1);
        let expose_from_fields =
            AccessControlExposeHeadersOwned::try_from(vec![FieldValue::from_static("content-type"), FieldValue::from_static("x-trace-id")])
                .expect("expose fields");
        assert_eq!(expose_from_fields.len(), 2);

        let absent = TestMap::new(&FieldName::Accept, Vec::new());
        assert!(AccessControlAllowHeaders::view(&absent).expect("absent").is_none());
        assert!(AccessControlAllowHeaders::owned(&absent).expect("absent").is_none());
    }

    #[test]
    fn header_name_lists_report_empty_and_later_field_errors() {
        let error = AccessControlRequestHeadersOwned::from_header_names(Vec::<String>::new()).expect_err("required list");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);

        let empty = TestMap::new(&FieldName::AccessControlRequestHeaders, vec![FieldValue::from_static("")]);
        assert_eq!(
            AccessControlRequestHeaders::view(&empty).expect_err("empty required list").kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            AccessControlRequestHeaders::owned(&empty).expect_err("empty required list").kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let bad_later = TestMap::new(
            &FieldName::AccessControlRequestHeaders,
            vec![FieldValue::from_static("content-type"), FieldValue::from_static("bad name")],
        );
        let error = AccessControlRequestHeaders::view(&bad_later).expect_err("bad second field line");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidToken);
        assert_eq!(error.value_index(), Some(1));
        let error = AccessControlRequestHeaders::owned(&bad_later).expect_err("bad second field line");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidToken);
        assert_eq!(error.value_index(), Some(1));

        let bad_first = TestMap::new(&FieldName::AccessControlRequestHeaders, vec![FieldValue::from_static("bad name")]);
        assert_eq!(
            AccessControlRequestHeaders::view(&bad_first)
                .expect_err("bad first field line")
                .value_index(),
            Some(0)
        );
        assert_eq!(
            AccessControlRequestHeaders::owned(&bad_first)
                .expect_err("bad first field line")
                .value_index(),
            Some(0)
        );

        let repeated_empty = TestMap::new(
            &FieldName::AccessControlRequestHeaders,
            vec![FieldValue::from_static(""), FieldValue::from_static(" , ")],
        );
        assert_eq!(
            AccessControlRequestHeaders::owned(&repeated_empty)
                .expect_err("empty repeated required list")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn private_token_helpers_cover_common_fallback_and_range_paths() {
        for method in [
            b"GET".as_slice(),
            b"PUT",
            b"HEAD",
            b"POST",
            b"PATCH",
            b"TRACE",
            b"DELETE",
            b"CONNECT",
            b"OPTIONS",
        ] {
            assert_eq!(method_ref(method).expect("registered method").as_bytes(), method);
        }
        assert_eq!(method_ref(b"CUSTOM").expect("extension").as_str(), "CUSTOM");
        assert!(method_ref(b"").is_none());
        assert!(method_ref(b"bad method").is_none());
        assert!(method_ref(&[0xff]).is_none());

        for name in [
            b"content-type".as_slice(),
            b"authorization",
            b"x-request-id",
            b"etag",
            b"origin",
            b"accept",
            b"x-requested-with",
            b"content-length",
            b"cache-control",
        ] {
            assert_eq!(header_name_ref(name).expect("common name").as_bytes(), name);
        }
        assert_eq!(header_name_ref(b"x-custom").expect("extension name").as_str(), "x-custom");
        assert!(header_name_ref(b"").is_none());
        assert!(header_name_ref(b"bad name").is_none());
        assert!(header_name_ref(&[0xff]).is_none());

        assert_eq!(untrimmed_range(b"token"), Some(0..5));
        assert_eq!(untrimmed_range(b" token"), None);
        assert_eq!(untrimmed_range(b"token "), None);
        assert_eq!(untrimmed_range(b""), None);
        assert_eq!(trimmed_range(b"\t token \t"), 2..7);
        assert_eq!(trimmed_range(b" \t "), 3..3);

        assert_eq!(invalid_syntax(&FieldName::Accept).kind(), DecodeErrorKind::InvalidSyntax);
        assert_eq!(invalid_token(&FieldName::Accept).kind(), DecodeErrorKind::InvalidToken);
        assert_eq!(invalid_number(&FieldName::Accept).kind(), DecodeErrorKind::InvalidNumber);
        assert_eq!(validate_list_value(b"                    "), Some(false));

        let mut sink = TestMap::new(&FieldName::AccessControlAllowHeaders, vec![FieldValue::from_static("content-type")]);
        sink.remove_values(&FieldName::Accept);
        assert_eq!(sink.values.len(), 1);
        sink.remove_values(&FieldName::AccessControlAllowHeaders);
        assert!(sink.values.is_empty());
    }
}
