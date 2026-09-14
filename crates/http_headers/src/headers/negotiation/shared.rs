// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::hash::{Hash, Hasher};
use std::slice;

use http_headers_simd::{EmptyMembers, TokenListScan};

use crate::source::{FieldLines, MAX_CUSTOM_LIST_ITEMS};
use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef, validate};

/// The preserved field lines of a list header.
///
/// A repeated list header is rare, so the sole line is held on its own rather
/// than behind the length and capacity a growable buffer would carry.
#[derive(Clone, Debug)]
pub(super) enum ListValues {
    One(FieldValue),
    Many(Vec<FieldValue>),
}

impl ListValues {
    /// Borrows the field lines as one contiguous run.
    #[inline]
    fn as_slice(&self) -> &[FieldValue] {
        match self {
            Self::One(line) => slice::from_ref(line),
            Self::Many(lines) => lines,
        }
    }

    #[inline]
    pub(super) fn len(&self) -> usize {
        self.as_slice().len()
    }

    #[inline]
    pub(super) fn iter(&self) -> slice::Iter<'_, FieldValue> {
        self.as_slice().iter()
    }
}

impl Eq for ListValues {}

impl PartialEq for ListValues {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Hash for ListValues {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

/// Copies the preserved field lines, without allocating for a single line.
#[expect(
    clippy::inline_always,
    reason = "specializing per call site folds the validator into the field line walk"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
pub(super) fn clone_checked_values(
    values: &FieldLines<'_>,
    mut validate_value: impl FnMut(usize, FieldValueRef<'_>) -> Result<(), DecodeError>,
) -> Result<ListValues, DecodeError> {
    let mut lines = values.repeated_owned()?;
    let (first, first_owned) = lines.next().expect("untyped values always contain a field line");
    validate_value(0, first)?;
    let Some((second, second_owned)) = lines.next() else {
        return Ok(ListValues::One(first_owned));
    };
    validate_value(1, second)?;
    clone_remaining_values(lines, first_owned, second_owned, validate_value)
}

/// Copies the field lines after the second, which only repeated headers have.
///
/// Keeping the growable buffer out of line leaves the single-line path, which
/// is the overwhelmingly common one, free of its register pressure.
#[inline(never)]
fn clone_remaining_values<'a>(
    lines: impl Iterator<Item = (FieldValueRef<'a>, FieldValue)>,
    first: FieldValue,
    second: FieldValue,
    mut validate_value: impl FnMut(usize, FieldValueRef<'_>) -> Result<(), DecodeError>,
) -> Result<ListValues, DecodeError> {
    let mut copied = vec![first, second];
    for (index, (line, owned)) in (2_usize..).zip(lines) {
        validate_value(index, line)?;
        copied.push(owned);
    }
    Ok(ListValues::Many(copied))
}

macro_rules! decode_owned_list {
    (quoted, $values:expr, $name:expr, $mode:expr, $strict:expr, $relaxed:expr) => {
        super::shared::checked_owned_values($values, $name, $mode, $strict, $relaxed, super::shared::decode_quoted_owned_list)
    };
    (token, $values:expr, $name:expr, $mode:expr, $strict:expr, $relaxed:expr) => {
        super::shared::checked_owned_values($values, $name, $mode, $strict, $relaxed, super::shared::decode_token_owned_list)
    };
}

/// Validates one already-delimited list member.
pub(super) type ItemValidator = fn(&[u8]) -> Result<(), DecodeError>;

#[expect(
    clippy::inline_always,
    reason = "specializing per call site folds the header name and item validator into the field line walk"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
pub(super) fn decode_quoted_owned_list(
    name: &'static FieldName,
    values: &FieldLines<'_>,
    validator: ItemValidator,
) -> Result<ListValues, DecodeError> {
    clone_checked_values(values, |index, value| {
        check_quoted_value(name, value, validator).map_err(|error| {
            if error.kind() == DecodeErrorKind::UnterminatedQuote {
                error.at_value(index)
            } else {
                error
            }
        })
    })
}

#[expect(
    clippy::inline_always,
    reason = "specializing per call site folds the header name and item validator into the field line walk"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
pub(super) fn decode_token_owned_list(
    name: &'static FieldName,
    values: &FieldLines<'_>,
    validator: ItemValidator,
) -> Result<ListValues, DecodeError> {
    clone_checked_values(values, |index, value| {
        if scan_token_list(value.as_bytes()) {
            Ok(())
        } else {
            Err(token_list_error(name, value.as_bytes(), validator, Some(index)))
        }
    })
}

pub(super) type ValuesValidator =
    for<'a> fn(&'static FieldName, &FieldLines<'a>, ItemValidator) -> Result<Option<FieldValueRef<'a>>, DecodeError>;

pub(super) type OwnedValuesDecoder = fn(&'static FieldName, &FieldLines<'_>, ItemValidator) -> Result<ListValues, DecodeError>;

fn validate_custom_values(values: &FieldLines<'_>, name: &'static FieldName, validator: ItemValidator) -> Result<bool, DecodeError> {
    if !values.has_custom_source_limits() {
        return Ok(false);
    }
    values.validate_custom_source_bounds()?;

    let mut item_count = 0_usize;
    for value in values.repeated() {
        for item in QuotedItems::comma(value.as_bytes(), name) {
            item_count = item_count.checked_add(1).ok_or_else(|| invalid_syntax(name))?;
            if item_count > MAX_CUSTOM_LIST_ITEMS {
                return Err(invalid_syntax(name));
            }
            validator(item?)?;
        }
    }
    Ok(true)
}

#[expect(
    clippy::inline_always,
    reason = "specializing per call site turns the decoder and validator pointers into direct calls"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
pub(super) fn checked_view_values<'a>(
    values: Option<FieldLines<'a>>,
    name: &'static FieldName,
    mode: crate::DecodeMode,
    strict: ItemValidator,
    relaxed: ItemValidator,
    validate_values: ValuesValidator,
) -> Result<Option<FieldLines<'a>>, DecodeError> {
    let Some(values) = values else {
        return Ok(None);
    };
    if validate_custom_values(
        &values,
        name,
        match mode {
            crate::DecodeMode::Strict => strict,
            crate::DecodeMode::Relaxed => relaxed,
        },
    )? {
        return Ok(Some(values));
    }
    values.validate_list_item_limit(b',', true)?;
    match mode {
        crate::DecodeMode::Strict => validate_values(name, &values, strict),
        crate::DecodeMode::Relaxed => validate_values(name, &values, relaxed),
    }
    .map(|_single| Some(values))
}

#[expect(
    clippy::inline_always,
    reason = "specializing per call site turns the decoder and validator pointers into direct calls"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
pub(super) fn checked_owned_values(
    values: Option<FieldLines<'_>>,
    name: &'static FieldName,
    mode: crate::DecodeMode,
    strict: ItemValidator,
    relaxed: ItemValidator,
    decode_values: OwnedValuesDecoder,
) -> Result<Option<ListValues>, DecodeError> {
    let Some(values) = values else {
        return Ok(None);
    };
    if validate_custom_values(
        &values,
        name,
        match mode {
            crate::DecodeMode::Strict => strict,
            crate::DecodeMode::Relaxed => relaxed,
        },
    )? {
        return clone_checked_values(&values, |_index, _value| Ok(())).map(Some);
    }
    values.validate_list_item_limit(b',', true)?;
    match mode {
        crate::DecodeMode::Strict => decode_values(name, &values, strict),
        crate::DecodeMode::Relaxed => decode_values(name, &values, relaxed),
    }
    .map(Some)
}

pub(super) fn encoded_list(values: ListValues) -> crate::sink::EncodedValues {
    match values {
        ListValues::One(line) => crate::sink::EncodedValues::single(line),
        ListValues::Many(lines) => crate::sink::EncodedValues::from_vec(lines),
    }
}

macro_rules! owned_list_items {
    (quoted, $self:expr, $name:expr) => {
        $self
            .values
            .iter()
            .flat_map(|value| super::shared::QuotedItems::comma(value.as_bytes(), $name))
            .filter_map(Result::ok)
    };
    (token, $self:expr, $name:expr) => {
        $self
            .values
            .iter()
            .flat_map(|value| value.as_bytes().split(|byte| *byte == b','))
            .map(validate::trim_ows)
            .filter(|item| !item.is_empty())
    };
}

macro_rules! borrowed_list_items {
    (quoted, $self:expr) => {
        $self.values.comma_items().filter_map(Result::ok)
    };
    (token, $self:expr) => {
        $self
            .values
            .repeated()
            .flat_map(|value| value.as_bytes().split(|byte| *byte == b','))
            .map(validate::trim_ows)
            .filter(|item| !item.is_empty())
    };
}

macro_rules! list_header {
    (
        $descriptor:ident,
        $owned:ident,
        $view:ident,
        $header_name:literal,
        $specification:literal,
        $name:expr,
        $validator:ident,
        $relaxed_validator:ident,
        $check_values:ident,
        $check_value:ident,
        $item_style:ident
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

        impl Clone for $owned {
            fn clone(&self) -> Self {
                Self {
                    values: self.values.clone(),
                }
            }
        }

        impl std::fmt::Debug for $owned {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($owned))
                    .field("value_count", &self.values.len())
                    .finish()
            }
        }

        impl Eq for $owned {}

        impl PartialEq for $owned {
            fn eq(&self, other: &Self) -> bool {
                self.values == other.values
            }
        }

        impl std::hash::Hash for $owned {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                self.values.hash(state);
            }
        }

        impl std::fmt::Debug for $view<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($view))
                    .field("value_count", &self.values.len())
                    .finish()
            }
        }

        impl $owned {
            /// Iterates raw list members in wire order.
            ///
            /// Empty RFC list members are ignored.
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::headers::AcceptEncodingOwned;
            ///
            /// let value = AcceptEncodingOwned::try_from("gzip, br")?;
            /// let items: Vec<&[u8]> = value.items().collect();
            /// assert_eq!(items, vec![b"gzip".as_slice(), b"br".as_slice()]);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn items(&self) -> impl Iterator<Item = &[u8]> {
                super::shared::owned_list_items!($item_style, self, $name)
            }

            /// Iterates the preserved field lines.
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::headers::AllowOwned;
            ///
            /// let value = AllowOwned::try_from("GET, HEAD")?;
            /// let mut values = value.values();
            /// assert_eq!(values.len(), 1);
            /// assert_eq!(
            ///     values.next().expect("one field line").as_bytes(),
            ///     b"GET, HEAD"
            /// );
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn values(&self) -> impl ExactSizeIterator<Item = FieldValueRef<'_>> {
                self.values.iter().map(FieldValue::as_field_value_ref)
            }
        }

        impl<'a> $view<'a> {
            /// Iterates raw list members in wire order without allocating.
            ///
            /// Empty RFC list members are ignored.
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::headers::{AcceptEncoding, AcceptEncodingView};
            /// use http_headers::source::{FieldLines, FieldSource};
            /// use http_headers::{Field, FieldName};
            ///
            /// struct Source;
            ///
            /// impl FieldSource for Source {
            ///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
            ///         (name == &FieldName::AcceptEncoding).then(|| FieldLines::single(name, b"gzip, br"))
            ///     }
            /// }
            ///
            /// let value: AcceptEncodingView<'_> = AcceptEncoding::view(&Source)?.expect("header is present");
            /// let items = value.items().collect::<Vec<_>>();
            /// assert_eq!(items, vec![b"gzip".as_slice(), b"br".as_slice()]);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn items(&self) -> impl Iterator<Item = &'a [u8]> + '_ {
                super::shared::borrowed_list_items!($item_style, self)
            }

            /// Iterates the original field lines.
            /// # Examples
            ///
            /// ```rust
            /// use http_headers::headers::{Allow, AllowView};
            /// use http_headers::source::{FieldLines, FieldSource};
            /// use http_headers::{Field, FieldName};
            ///
            /// struct Source;
            ///
            /// impl FieldSource for Source {
            ///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
            ///         (name == &FieldName::Allow).then(|| FieldLines::single(name, b"GET, HEAD"))
            ///     }
            /// }
            ///
            /// let value: AllowView<'_> = Allow::view(&Source)?.expect("header is present");
            /// let values: Vec<&[u8]> = value.values().map(|line| line.as_bytes()).collect();
            /// assert_eq!(values, vec![b"GET, HEAD".as_slice()]);
            /// # Ok::<(), http_headers::DecodeError>(())
            /// ```
            pub fn values(&self) -> impl Iterator<Item = FieldValueRef<'a>> + '_ {
                self.values.repeated()
            }
        }

        impl Field for $descriptor {
            type View<'a> = $view<'a>;
            type Owned = $owned;

            fn name() -> &'static FieldName {
                $name
            }

            #[inline]
            fn view_with<S>(source: &S, mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
            where
                S: FieldSource + ?Sized,
            {
                super::shared::checked_view_values(
                    source.lines(Self::name()),
                    $name,
                    mode,
                    $validator,
                    $relaxed_validator,
                    $check_values,
                )
                .map(|values| values.map(|values| $view { values }))
            }

            #[inline]
            fn owned_with<S>(source: &S, mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
            where
                S: FieldSource + ?Sized,
            {
                super::shared::decode_owned_list!(
                    $item_style,
                    source.lines(Self::name()),
                    $name,
                    mode,
                    $validator,
                    $relaxed_validator
                )
                .map(|values| values.map(|values| $owned { values }))
            }

            fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
            where
                S: FieldSink + ?Sized,
            {
                sink.set_values(Self::name(), super::shared::encoded_list(value.values))
            }
        }

        impl TryFrom<&str> for $owned {
            type Error = DecodeError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                let value = FieldValue::from_str(value).map_err(|_invalid| super::shared::invalid_syntax($name))?;
                Self::try_from(value)
            }
        }

        impl TryFrom<String> for $owned {
            type Error = DecodeError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                let value = FieldValue::try_from(value).map_err(|_invalid| super::shared::invalid_syntax($name))?;
                Self::try_from(value)
            }
        }

        impl TryFrom<FieldValue> for $owned {
            type Error = DecodeError;

            fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
                $check_value($name, value.as_field_value_ref(), $validator)?;
                Ok(Self {
                    values: super::shared::ListValues::One(value),
                })
            }
        }
    };
}

/// Validates every member of a list whose members may contain quoted strings.
///
/// The sole field line is never reported, because the delimited iteration this
/// performs does not reveal the field line boundaries it crosses.
#[expect(
    clippy::inline_always,
    reason = "specializing per call site folds the header name and item validator into the scan"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
pub(super) fn check_quoted_values<'a>(
    name: &'static FieldName,
    values: &FieldLines<'a>,
    validator: ItemValidator,
) -> Result<Option<FieldValueRef<'a>>, DecodeError> {
    let mut lines = values.repeated();
    let first = lines.next().expect("untyped values always contain a field line");
    let single = lines.next().is_none();
    check_quoted_value(name, first, validator)?;
    if single {
        return Ok(Some(first));
    }
    check_repeated_quoted_values(name, values, validator).map(|()| None)
}

/// Scans the field lines after the first one, which only repeated headers have.
///
/// A repeated negotiation header is rare, so keeping this cold and out of line
/// leaves the single-line path a short straight run.
#[cold]
#[inline(never)]
fn check_repeated_quoted_values(name: &'static FieldName, values: &FieldLines<'_>, validator: ItemValidator) -> Result<(), DecodeError> {
    for line in values.repeated().skip(1) {
        check_quoted_value(name, line, validator)?;
    }
    Ok(())
}

#[expect(
    clippy::inline_always,
    reason = "specializing per call site folds the header name and item validator into the scan"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
pub(super) fn check_quoted_value(name: &'static FieldName, value: FieldValueRef<'_>, validator: ItemValidator) -> Result<(), DecodeError> {
    if is_well_known_negotiation_line(name, value.as_bytes()) {
        return Ok(());
    }
    if recognizes_whole_line(name, value.as_bytes()) {
        return Ok(());
    }
    if try_plain_items(value.as_bytes(), b',', true, validator)? {
        return Ok(());
    }
    check_quoted_members(name, value.as_bytes(), validator)
}

/// Validates members of a line that carries quoted syntax.
///
/// Quoting is rare in negotiation values, so this carries the quote and escape
/// state that [`try_plain_items`] deliberately leaves out.
#[cold]
#[inline(never)]
fn check_quoted_members(name: &'static FieldName, bytes: &[u8], validator: ItemValidator) -> Result<(), DecodeError> {
    for item in QuotedItems::comma(bytes, name) {
        validator(item?)?;
    }
    Ok(())
}

/// Recognizes a whole negotiation line without splitting it into members.
///
/// The content negotiation headers each carry quality parameters, which makes
/// their member grammar rich enough to be worth a dedicated whole-line pass;
/// every other header falls straight through. A `false` answer always means
/// "not recognized", never "malformed", so the general parser still produces
/// every diagnostic.
fn recognizes_whole_line(name: &'static FieldName, bytes: &[u8]) -> bool {
    match *name {
        FieldName::Accept => super::accept_scan::scan_accept_line(bytes),
        FieldName::AcceptEncoding => super::weighted_token_scan::scan_accept_encoding_line(bytes),
        FieldName::AcceptLanguage => super::weighted_token_scan::scan_accept_language_line(bytes),
        _ => false,
    }
}

/// The `Accept` lines recognized without walking the member grammar.
pub(super) const WELL_KNOWN_ACCEPT: &[&[u8]] = &[b"*/*", b"application/json", b"text/html"];

/// The `Accept-Encoding` lines recognized without walking the member grammar.
pub(super) const WELL_KNOWN_ACCEPT_ENCODING: &[&[u8]] = &[b"gzip", b"br", b"identity"];

/// The `Accept-Language` lines recognized without walking the member grammar.
pub(super) const WELL_KNOWN_ACCEPT_LANGUAGE: &[&[u8]] = &[b"en", b"en-US", b"*"];

/// Recognizes negotiation lines whose whole text is a single common token.
///
/// A client that states one preference sends the same handful of lines over
/// and over, and matching the whole line settles them without walking the
/// member grammar. Lines carrying several members or quality parameters vary
/// too much between clients to be worth a literal, so they are parsed.
///
/// Every recognized line also satisfies the grammar it skips, which the tests
/// beside each header's validator check.
fn is_well_known_negotiation_line(name: &'static FieldName, bytes: &[u8]) -> bool {
    let recognized = if name == &FieldName::Accept {
        WELL_KNOWN_ACCEPT
    } else if name == &FieldName::AcceptEncoding {
        WELL_KNOWN_ACCEPT_ENCODING
    } else if name == &FieldName::AcceptLanguage {
        WELL_KNOWN_ACCEPT_LANGUAGE
    } else {
        return false;
    };
    recognized.contains(&bytes)
}

/// Validates delimiter-separated items until quoted syntax requires fallback.
///
/// Common negotiation values contain no quoted strings. Keeping that path to
/// delimiter checks and trimming avoids carrying quote and escape state
/// through every byte; encountering either marker leaves validation to
/// [`QuotedItems`] from the beginning.
#[expect(
    clippy::inline_always,
    reason = "specializing per call site folds the item validator into the delimiter scan"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
pub(super) fn try_plain_items(
    bytes: &[u8],
    delimiter: u8,
    skip_empty: bool,
    mut validator: impl FnMut(&[u8]) -> Result<(), DecodeError>,
) -> Result<bool, DecodeError> {
    let mut start = 0;
    for (position, byte) in bytes.iter().copied().enumerate() {
        if matches!(byte, b'"' | b'\\') {
            return Ok(false);
        }
        if byte == delimiter {
            let item = validate::trim_ows(&bytes[start..position]);
            if !skip_empty || !item.is_empty() {
                validator(item)?;
            }
            start = position + 1;
        }
    }
    let item = validate::trim_ows(&bytes[start..]);
    if !skip_empty || !item.is_empty() {
        return validator(item).map(|()| true);
    }
    Ok(true)
}

#[expect(
    clippy::inline_always,
    reason = "specializing per call site folds the header name and item validator into the scan"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
pub(super) fn check_token_values<'a>(
    name: &'static FieldName,
    values: &FieldLines<'a>,
    validator: ItemValidator,
) -> Result<Option<FieldValueRef<'a>>, DecodeError> {
    let mut lines = values.repeated();
    let first = lines.next().expect("untyped values always contain a field line");
    let single = lines.next().is_none();
    if !is_well_known_token_list(first.as_bytes()) {
        return check_unusual_token_values(name, values, first, single, validator);
    }
    if single {
        return Ok(Some(first));
    }
    check_repeated_token_values(name, values, validator).map(|()| None)
}

/// Finishes a decode whose first field line is not one of the well-known ones.
///
/// Taking the whole tail out of line, rather than just the scan, keeps the
/// recognized-line path free of the register pressure a call site imposes.
#[inline(never)]
fn check_unusual_token_values<'a>(
    name: &'static FieldName,
    values: &FieldLines<'a>,
    first: FieldValueRef<'a>,
    single: bool,
    validator: ItemValidator,
) -> Result<Option<FieldValueRef<'a>>, DecodeError> {
    if !scan_token_list_general(first.as_bytes()) {
        return Err(token_list_error(name, first.as_bytes(), validator, Some(0)));
    }
    if single {
        return Ok(Some(first));
    }
    check_repeated_token_values(name, values, validator).map(|()| None)
}

/// Scans the field lines after the first one, which only repeated headers have.
///
/// A repeated list header is rare, so keeping this cold and out of line, and
/// rebuilding the walk from `values` rather than taking a half-consumed one,
/// leaves the single-line path a short straight run.
#[cold]
#[inline(never)]
fn check_repeated_token_values(name: &'static FieldName, values: &FieldLines<'_>, validator: ItemValidator) -> Result<(), DecodeError> {
    for (offset, line) in values.repeated().enumerate().skip(1) {
        if !scan_token_list(line.as_bytes()) {
            return Err(token_list_error(name, line.as_bytes(), validator, Some(offset)));
        }
    }
    Ok(())
}

#[expect(
    clippy::inline_always,
    reason = "specializing per call site folds the header name and item validator into the scan"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
pub(super) fn check_token_value(name: &'static FieldName, value: FieldValueRef<'_>, validator: ItemValidator) -> Result<(), DecodeError> {
    if scan_token_list(value.as_bytes()) {
        Ok(())
    } else {
        Err(token_list_error(name, value.as_bytes(), validator, None))
    }
}

/// Reads the eight bytes at `offset`, or zero when they are not all present.
#[expect(clippy::inline_always, reason = "the callers pass constant offsets that fold the bounds test away")]
#[cfg_attr(not(coverage_nightly), inline(always))]
fn word(bytes: &[u8], offset: usize) -> u64 {
    let mut chunk = [0_u8; 8];
    if let Some(source) = bytes.get(offset..offset + 8) {
        chunk.copy_from_slice(source);
    }
    u64::from_le_bytes(chunk)
}

/// Returns whether `bytes` equals `expected`, comparing eight bytes at a time.
///
/// The final word overlaps the one before it, so any length is covered by
/// `len / 8` rounded up comparisons rather than one comparison per byte.
#[expect(
    clippy::inline_always,
    reason = "expected is always a literal, which folds the loop and the words away"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
fn equals_word_at_a_time(bytes: &[u8], expected: &[u8]) -> bool {
    if bytes.len() != expected.len() {
        return false;
    }
    if bytes.len() < 8 {
        return bytes == expected;
    }
    let mut offset = 0;
    while offset + 8 < bytes.len() {
        if word(bytes, offset) != word(expected, offset) {
            return false;
        }
        offset += 8;
    }
    match bytes.len() - offset {
        // One or two trailing bytes are cheaper read on their own than as a
        // word overlapping the one before them.
        1 => bytes.get(offset) == expected.get(offset),
        2 => half_word(bytes, offset) == half_word(expected, offset),
        _ => {
            let last = bytes.len() - 8;
            word(bytes, last) == word(expected, last)
        }
    }
}

/// Reads the two bytes at `offset`, or zero when they are not both present.
#[expect(clippy::inline_always, reason = "the callers pass constant offsets that fold the bounds test away")]
#[cfg_attr(not(coverage_nightly), inline(always))]
fn half_word(bytes: &[u8], offset: usize) -> u16 {
    let mut chunk = [0_u8; 2];
    if let Some(source) = bytes.get(offset..offset + 2) {
        chunk.copy_from_slice(source);
    }
    u16::from_le_bytes(chunk)
}

/// The field lines short enough that byte comparison is already cheap.
///
/// This mirrors [`is_well_known_token_list`] so the tests can prove every
/// accepted line is valid.
#[cfg(test)]
const SHORT_WELL_KNOWN_TOKEN_LISTS: &[&[u8]] = &[
    b"*", b"GET", b"PUT", b"HEAD", b"POST", b"PATCH", b"DELETE", b"OPTIONS", b"accept", b"cookie", b"origin",
];

/// The field lines compared eight bytes at a time, grouped by length.
///
/// This mirrors [`is_well_known_token_list`] so the tests can prove every
/// accepted line is valid.
#[cfg(test)]
const LONG_WELL_KNOWN_TOKEN_LISTS: &[&[u8]] = &[
    b"GET, HEAD",
    b"GET, POST",
    b"HEAD, GET",
    b"user-agent",
    b"authorization",
    b"GET, HEAD, POST",
    b"accept-encoding",
    b"accept-language",
    b"GET, HEAD, OPTIONS",
    b"GET, POST, OPTIONS",
    b"OPTIONS, GET, HEAD",
    b"accept, accept-encoding",
    b"accept-encoding, origin",
    b"origin, accept-encoding",
    b"accept-encoding, accept-language",
];

/// Returns whether a field line is one of the well-known valid token lists.
///
/// These lines carry the overwhelming majority of real `Allow` and `Vary`
/// traffic. Recognizing one costs a load and a compare per eight bytes, where
/// the general scan costs several instructions per byte, and every line the
/// table accepts is a valid bare token list, so a hit may skip the scan.
#[expect(
    clippy::inline_always,
    reason = "specializing per call site folds the table into a switch on the line length"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
fn is_well_known_token_list(bytes: &[u8]) -> bool {
    if bytes.len() < 8 {
        return matches!(
            bytes,
            b"*" | b"GET" | b"PUT" | b"HEAD" | b"POST" | b"PATCH" | b"DELETE" | b"OPTIONS" | b"accept" | b"cookie" | b"origin"
        );
    }
    match bytes.len() {
        9 => {
            equals_word_at_a_time(bytes, b"GET, POST")
                || equals_word_at_a_time(bytes, b"GET, HEAD")
                || equals_word_at_a_time(bytes, b"HEAD, GET")
        }
        10 => equals_word_at_a_time(bytes, b"user-agent"),
        13 => equals_word_at_a_time(bytes, b"authorization"),
        15 => {
            equals_word_at_a_time(bytes, b"accept-encoding")
                || equals_word_at_a_time(bytes, b"accept-language")
                || equals_word_at_a_time(bytes, b"GET, HEAD, POST")
        }
        18 => {
            equals_word_at_a_time(bytes, b"GET, HEAD, OPTIONS")
                || equals_word_at_a_time(bytes, b"GET, POST, OPTIONS")
                || equals_word_at_a_time(bytes, b"OPTIONS, GET, HEAD")
        }
        23 => {
            equals_word_at_a_time(bytes, b"accept-encoding, origin")
                || equals_word_at_a_time(bytes, b"origin, accept-encoding")
                || equals_word_at_a_time(bytes, b"accept, accept-encoding")
        }
        32 => equals_word_at_a_time(bytes, b"accept-encoding, accept-language"),
        _ => false,
    }
}

/// Returns whether a field line is a comma-delimited list of bare tokens.
///
/// Optional whitespace around members and empty members are ignored, so this
/// accepts exactly the lines whose members are all valid tokens. Rejected
/// lines are re-scanned by [`token_list_error`] to describe the failure.
#[inline]
pub(super) fn scan_token_list(bytes: &[u8]) -> bool {
    is_well_known_token_list(bytes) || scan_token_list_general(bytes)
}

/// Returns whether a field line is a comma-delimited list of bare tokens.
///
/// This classifies every byte, so it accepts far more than the well-known
/// table and costs far more to do it. Lines long enough to be worth it are
/// folded a vector register at a time by [`http_headers_simd`].
fn scan_token_list_general(bytes: &[u8]) -> bool {
    http_headers_simd::scan_token_list(bytes, EmptyMembers::Skip) != TokenListScan::Rejected
}

/// Describes why a field line is not a list of bare tokens.
///
/// A line only reaches this function once [`scan_token_list`] has rejected it,
/// so the delimiter-aware scan it performs always fails as well.
#[cold]
pub(super) fn token_list_error(
    name: &'static FieldName,
    bytes: &[u8],
    validator: ItemValidator,
    value_index: Option<usize>,
) -> DecodeError {
    for item in QuotedItems::comma(bytes, name) {
        match item {
            Ok(item) => {
                if let Err(error) = validator(item) {
                    return error;
                }
            }
            Err(error) => {
                return match value_index {
                    Some(index) => error.at_value(index),
                    None => error,
                };
            }
        }
    }
    invalid(name, DecodeErrorKind::InvalidToken)
}

#[expect(
    clippy::inline_always,
    reason = "specializing per call site folds the member grammar into the caller's item loop"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
pub(super) fn validate_weighted_token(
    bytes: &[u8],
    name: &'static FieldName,
    item_validator: fn(&[u8]) -> bool,
    relaxed: bool,
) -> Result<(), DecodeError> {
    // A member without a parameter or quoting is just its own token, so the
    // segment walk below would find one segment and validate exactly this.
    if !carries_parameter_syntax(bytes) {
        return if item_validator(validate::trim_ows(bytes)) {
            Ok(())
        } else {
            Err(invalid(name, DecodeErrorKind::InvalidToken))
        };
    }
    if validate_weighted_token_plain(bytes, name, item_validator, relaxed)? {
        return Ok(());
    }
    validate_weighted_token_quoted(bytes, name, item_validator, relaxed)
}

/// Reports whether a list member carries parameter or quoting syntax.
#[inline]
fn carries_parameter_syntax(bytes: &[u8]) -> bool {
    bytes.iter().any(|byte| matches!(byte, b';' | b'"' | b'\\'))
}

#[expect(
    clippy::inline_always,
    reason = "specializing per call site folds the member grammar into the caller's item loop"
)]
#[cfg_attr(not(coverage_nightly), inline(always))]
fn validate_weighted_token_plain(
    bytes: &[u8],
    name: &'static FieldName,
    item_validator: fn(&[u8]) -> bool,
    relaxed: bool,
) -> Result<bool, DecodeError> {
    let mut segment = 0_u8;
    try_plain_items(bytes, b';', false, |bytes| {
        segment = segment.saturating_add(1);
        match segment {
            1 if item_validator(bytes) => Ok(()),
            1 => Err(invalid(name, DecodeErrorKind::InvalidToken)),
            2 if bytes.len() >= 2 && bytes[0].eq_ignore_ascii_case(&b'q') && bytes[1] == b'=' => {
                validate_quality(&bytes[2..], name, relaxed)
            }
            2 => {
                let (parameter, value, compact) = parse_parameter(bytes, false, name)?;
                if (!relaxed && !compact) || !validate::eq_ignore_ascii_case(parameter, b"q") {
                    return Err(invalid_syntax(name));
                }
                validate_quality(value.expect("required parameters always contain a value"), name, relaxed)
            }
            _ => Err(invalid_syntax(name)),
        }
    })
}

#[cold]
#[inline(never)]
fn validate_weighted_token_quoted(
    bytes: &[u8],
    name: &'static FieldName,
    item_validator: fn(&[u8]) -> bool,
    relaxed: bool,
) -> Result<(), DecodeError> {
    let mut segments = QuotedItems::semicolon(bytes, name);
    let item = segments.next().expect("semicolon iteration always yields a first item")?;
    if !item_validator(item) {
        return Err(invalid(name, DecodeErrorKind::InvalidToken));
    }
    if let Some(weight) = segments.next() {
        let (parameter, value, compact) = parse_parameter(weight?, false, name)?;
        if (!relaxed && !compact) || !validate::eq_ignore_ascii_case(parameter, b"q") {
            return Err(invalid_syntax(name));
        }
        let quality = value.expect("required parameters always contain a value");
        validate_quality(quality, name, relaxed)?;
    }
    if let Some(extra) = segments.next() {
        extra?;
        return Err(invalid_syntax(name));
    }
    Ok(())
}

pub(super) type ParsedParameter<'a> = (&'a [u8], Option<&'a [u8]>, bool);

pub(super) fn parse_parameter<'a>(
    bytes: &'a [u8],
    value_optional: bool,
    header: &'static FieldName,
) -> Result<ParsedParameter<'a>, DecodeError> {
    let equals = bytes.iter().position(|byte| *byte == b'=');
    let (name, value, compact) = if let Some(equals) = equals {
        let raw_name = &bytes[..equals];
        let raw_value = &bytes[equals + 1..];
        let name = validate::trim_ows(raw_name);
        let value = validate::trim_ows(raw_value);
        if !valid_parameter_value(value) {
            return Err(invalid_syntax(header));
        }
        (name, Some(value), raw_name.len() == name.len() && raw_value.len() == value.len())
    } else if value_optional {
        (validate::trim_ows(bytes), None, true)
    } else {
        return Err(invalid_syntax(header));
    };
    if !validate::token(name) {
        return Err(invalid(header, DecodeErrorKind::InvalidToken));
    }
    Ok((name, value, compact))
}

fn valid_parameter_value(bytes: &[u8]) -> bool {
    validate::token(bytes) || valid_quoted_string(bytes)
}

fn valid_quoted_string(bytes: &[u8]) -> bool {
    if bytes.len() < 2 || bytes.first() != Some(&b'"') || bytes.last() != Some(&b'"') {
        return false;
    }
    let mut escaped = false;
    for byte in bytes[1..bytes.len() - 1].iter().copied() {
        if escaped {
            if !matches!(byte, b'\t' | b' '..=b'~' | 0x80..=0xff) {
                return false;
            }
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if !matches!(byte, b'\t' | b' ' | b'!' | b'#'..=b'[' | b']'..=b'~' | 0x80..=0xff) {
            return false;
        }
    }
    !escaped
}

fn validate_qvalue(bytes: &[u8], header: &'static FieldName) -> Result<(), DecodeError> {
    let valid = match bytes {
        [b'0' | b'1'] => true,
        [whole @ (b'0' | b'1'), b'.', fraction @ ..] if fraction.len() <= 3 => fraction
            .iter()
            .all(|byte| byte.is_ascii_digit() && (*whole == b'0' || *byte == b'0')),
        _ => false,
    };
    if valid { Ok(()) } else { Err(invalid_syntax(header)) }
}

pub(super) fn validate_quality(bytes: &[u8], header: &'static FieldName, relaxed: bool) -> Result<(), DecodeError> {
    if relaxed {
        validate_qvalue_relaxed(bytes, header)
    } else {
        validate_qvalue(bytes, header)
    }
}

fn validate_qvalue_relaxed(bytes: &[u8], header: &'static FieldName) -> Result<(), DecodeError> {
    let bytes = validate::trim_ows(bytes);
    let valid = match bytes {
        [b'0' | b'1'] => true,
        [whole @ (b'0' | b'1'), b'.', fraction @ ..] => fraction
            .iter()
            .all(|byte| byte.is_ascii_digit() && (*whole == b'0' || *byte == b'0')),
        [b'.', fraction @ ..] if !fraction.is_empty() => fraction.iter().all(u8::is_ascii_digit),
        _ => false,
    };
    if valid { Ok(()) } else { Err(invalid_syntax(header)) }
}

#[derive(Debug)]
pub(super) struct QuotedItems<'a> {
    bytes: &'a [u8],
    header: &'static FieldName,
    delimiter: u8,
    position: usize,
    start: usize,
    finished: bool,
    skip_empty: bool,
}

impl<'a> QuotedItems<'a> {
    pub(super) const fn comma(bytes: &'a [u8], header: &'static FieldName) -> Self {
        Self::new(bytes, header, b',', true)
    }

    pub(super) const fn semicolon(bytes: &'a [u8], header: &'static FieldName) -> Self {
        Self::new(bytes, header, b';', false)
    }

    const fn new(bytes: &'a [u8], header: &'static FieldName, delimiter: u8, skip_empty: bool) -> Self {
        Self {
            bytes,
            header,
            delimiter,
            position: 0,
            start: 0,
            finished: false,
            skip_empty,
        }
    }
}

impl<'a> Iterator for QuotedItems<'a> {
    type Item = Result<&'a [u8], DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
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
                } else if !quoted && byte == self.delimiter {
                    let item = validate::trim_ows(&self.bytes[self.start..self.position]);
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
                return Some(Err(invalid(self.header, DecodeErrorKind::UnterminatedQuote)));
            }
            let item = validate::trim_ows(&self.bytes[self.start..]);
            if !self.skip_empty || !item.is_empty() {
                return Some(Ok(item));
            }
        }
    }
}

/// Builds a decode failure.
///
/// Every call sits on a path a well-formed field line never takes, so marking
/// it cold keeps the construction out of the straight-line decode.
#[cold]
pub(super) fn invalid(header: &'static FieldName, kind: DecodeErrorKind) -> DecodeError {
    DecodeError::new(header, kind)
}

pub(super) fn invalid_syntax(header: &'static FieldName) -> DecodeError {
    invalid(header, DecodeErrorKind::InvalidSyntax)
}

pub(super) use borrowed_list_items;
pub(super) use decode_owned_list;
pub(super) use list_header;
pub(super) use owned_list_items;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod weighted_token_tests {
    // These tests pin the parameter-free fast path to the general segment walk.
    use super::{DecodeError, validate_weighted_token, validate_weighted_token_plain, validate_weighted_token_quoted};
    use crate::{FieldName, validate};

    /// Validates a member without the parameter-free fast path.
    fn reference(bytes: &[u8], name: &'static FieldName, item_validator: fn(&[u8]) -> bool, relaxed: bool) -> Result<(), DecodeError> {
        if validate_weighted_token_plain(bytes, name, item_validator, relaxed)? {
            return Ok(());
        }
        validate_weighted_token_quoted(bytes, name, item_validator, relaxed)
    }

    fn members() -> Vec<Vec<u8>> {
        let mut members: Vec<Vec<u8>> = [
            "",
            " ",
            "  ",
            "\t",
            "gzip",
            " gzip ",
            "\tbr\t",
            "en_US",
            "*",
            "identity",
            "a;",
            ";",
            ";q=0.5",
            "gzip;q=0.5",
            "gzip ; q=0.5",
            "gzip;;q=0.5",
            "gzip;q=",
            "\"x\"",
            "\"gzip\"",
            "a\\b",
            "gzip;q=\"0.5\"",
            "gzip;\"q\"=0.5",
            "gzip,br",
            "text/html",
            "gzip;q=0.5;extra",
            "\\",
            "\"",
            "\";\"",
            "gzip\u{7f}",
        ]
        .iter()
        .map(|member| member.as_bytes().to_vec())
        .collect();

        members.extend((0_u16..=255).map(|byte| vec![u8::try_from(byte).unwrap_or(0)]));
        members.extend((0_u16..=255).map(|byte| vec![b'g', u8::try_from(byte).unwrap_or(0), b'z']));
        members
    }

    #[test]
    fn fast_path_matches_the_general_segment_walk() {
        let validators: [fn(&[u8]) -> bool; 2] = [validate::token, |bytes| !bytes.is_empty()];

        for member in members() {
            for item_validator in validators {
                for relaxed in [false, true] {
                    let fast = validate_weighted_token(&member, &FieldName::Accept, item_validator, relaxed);
                    let slow = reference(&member, &FieldName::Accept, item_validator, relaxed);

                    assert_eq!(
                        format!("{fast:?}"),
                        format!("{slow:?}"),
                        "member {:?} (relaxed: {relaxed}) must validate identically",
                        String::from_utf8_lossy(&member)
                    );
                }
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod token_list_tests {
    use super::recognizes_whole_line;

    #[test]
    fn only_the_negotiation_headers_are_recognized_whole() {
        assert!(recognizes_whole_line(&FieldName::Accept, b"text/html;q=0.9"));
        assert!(recognizes_whole_line(&FieldName::AcceptEncoding, b"gzip;q=0.9"));
        assert!(recognizes_whole_line(&FieldName::AcceptLanguage, b"en-US;q=0.9"));

        for name in [&FieldName::Vary, &FieldName::Allow, &FieldName::Server] {
            assert!(
                !recognizes_whole_line(name, b"text/html;q=0.9"),
                "{name} has no whole-line recognizer"
            );
        }
    }

    // These tests exercise private well-known tables and token-list scanners.
    use super::{
        LONG_WELL_KNOWN_TOKEN_LISTS, QuotedItems, SHORT_WELL_KNOWN_TOKEN_LISTS, is_well_known_token_list, scan_token_list,
        scan_token_list_general,
    };
    use crate::{FieldName, validate};

    /// Accepts exactly the lines the delimiter-aware fallback accepts.
    fn delimited_scan(bytes: &[u8]) -> bool {
        QuotedItems::comma(bytes, &FieldName::Allow).all(|item| match item {
            Ok(item) => validate::token(item),
            Err(_error) => false,
        })
    }

    fn well_known_lines() -> impl Iterator<Item = &'static [u8]> {
        SHORT_WELL_KNOWN_TOKEN_LISTS.iter().chain(LONG_WELL_KNOWN_TOKEN_LISTS).copied()
    }

    #[test]
    fn every_well_known_line_is_recognized_and_valid() {
        for line in well_known_lines() {
            let shown = line.escape_ascii().to_string();
            assert!(is_well_known_token_list(line), "{shown}");
            assert!(scan_token_list_general(line), "{shown}");
            assert!(delimited_scan(line), "{shown}");
        }
    }

    #[cfg(test)]
    #[expect(
        clippy::assertions_on_result_states,
        reason = "the tests classify many parser outcomes without needing their success values"
    )]
    mod behavior_tests {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        use super::super::{
            ListValues, QuotedItems, check_quoted_value, check_quoted_values, check_token_value, check_token_values, clone_checked_values,
            equals_word_at_a_time, half_word, is_well_known_negotiation_line, parse_parameter, token_list_error, try_plain_items,
            valid_quoted_string, validate_quality, validate_weighted_token, validate_weighted_token_quoted, word,
        };
        use crate::headers::{
            Accept, AcceptEncoding, AcceptEncodingOwned, AcceptLanguage, AcceptLanguageOwned, AcceptOwned, Allow, AllowOwned, Vary,
            VaryOwned,
        };
        use crate::sink::{EncodedValues, FieldSink, InsertError};
        use crate::source::{FieldLines, FieldSource};
        use crate::{DecodeError, DecodeErrorKind, DecodeMode, Field, FieldName, FieldValue, validate};

        #[derive(Default)]
        struct Store {
            name: Option<&'static FieldName>,
            values: Vec<FieldValue>,
        }

        impl Store {
            fn new(name: &'static FieldName, values: &[&'static str]) -> Self {
                Self {
                    name: Some(name),
                    values: values.iter().copied().map(FieldValue::from_static).collect(),
                }
            }
        }

        impl FieldSource for Store {
            fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
                (self.name == Some(name))
                    .then(|| FieldLines::from_slice(name, &self.values))
                    .flatten()
            }
        }

        impl FieldSink for Store {
            fn set_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
                self.name = Some(name);
                self.values = values.into_iter().collect();
                Ok(())
            }

            fn append_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
                if self.name == Some(name) {
                    self.values.extend(values);
                } else {
                    self.name = Some(name);
                    self.values = values.into_iter().collect();
                }
                Ok(())
            }

            fn remove_values(&mut self, name: &'static FieldName) {
                if self.name == Some(name) {
                    self.name = None;
                    self.values.clear();
                }
            }
        }

        fn validate_token(bytes: &[u8]) -> Result<(), DecodeError> {
            if validate::token(bytes) {
                Ok(())
            } else {
                Err(DecodeError::new(&FieldName::Accept, DecodeErrorKind::InvalidToken))
            }
        }

        #[test]
        fn generated_list_headers_preserve_single_and_repeated_lines() {
            let single = AcceptOwned::try_from(String::from("text/plain;level=\"one\", text/html")).expect("quoted media parameters");
            assert_eq!(
                single.items().collect::<Vec<_>>(),
                [b"text/plain;level=\"one\"".as_slice(), b"text/html"]
            );
            assert_eq!(single.values().len(), 1);
            assert!(format!("{single:?}").contains("value_count: 1"));
            assert_eq!(single, single.clone());
            let mut hasher = DefaultHasher::new();
            single.hash(&mut hasher);
            assert_ne!(hasher.finish(), 0);

            let source = Store::new(&FieldName::Accept, &["text/plain;level=\"one\"", "application/json", "text/html"]);
            let view = Accept::view(&source).expect("valid repeated list").expect("header present");
            assert_eq!(view.items().count(), 3);
            assert_eq!(view.values().count(), 3);
            assert!(format!("{view:?}").contains("value_count: 3"));

            let owned = Accept::owned(&source).expect("valid repeated list").expect("header present");
            assert_eq!(owned.values().len(), 3);
            let mut sink = Store::default();
            Accept::insert(&mut sink, owned).expect("insert repeated values");
            assert_eq!(sink.values.len(), 3);

            let mut sink = Store::default();
            Accept::insert(&mut sink, single).expect("insert one value");
            assert_eq!(sink.values.len(), 1);
            sink.remove_values(&FieldName::Accept);
            assert!(sink.values.is_empty());
            assert_eq!(sink.name, None);
            assert!(Accept::view(&Store::default()).expect("absent source").is_none());
            assert!(Accept::owned(&Store::default()).expect("absent source").is_none());
        }

        #[test]
        fn generated_token_lists_validate_views_owned_values_and_conversions() {
            let source = Store::new(&FieldName::Allow, &["GET, HEAD", "POST"]);
            let view = Allow::view(&source).expect("valid methods").expect("header present");
            assert_eq!(view.items().collect::<Vec<_>>(), [b"GET".as_slice(), b"HEAD", b"POST"]);
            let owned = Allow::owned(&source).expect("valid methods").expect("header present");
            assert_eq!(owned.items().count(), 3);

            let mut sink = Store::default();
            Allow::insert(&mut sink, owned).expect("insert token list");
            assert_eq!(sink.values.len(), 2);

            assert_eq!(
                AllowOwned::try_from(String::from("bad method"))
                    .expect_err("spaces are not tokens")
                    .kind(),
                DecodeErrorKind::InvalidToken
            );
            assert_eq!(
                VaryOwned::try_from(String::from("line\nbreak"))
                    .expect_err("invalid field value")
                    .kind(),
                DecodeErrorKind::InvalidSyntax
            );

            let relaxed = Store::new(&FieldName::Accept, &["text/plain; q = .75"]);
            assert!(Accept::view_with(&relaxed, DecodeMode::Relaxed).expect("relaxed quality").is_some());
            assert!(
                Accept::owned_with(&relaxed, DecodeMode::Relaxed)
                    .expect("relaxed quality")
                    .is_some()
            );
        }

        #[test]
        fn generated_owned_error_and_borrowed_conversion_paths_are_preserved() {
            for source in [
                Store::new(&FieldName::Accept, &["text/plain", "\"unterminated"]),
                Store::new(&FieldName::Accept, &["text/plain", "invalid"]),
            ] {
                assert!(Accept::owned(&source).is_err());
            }
            let invalid_allow = Store::new(&FieldName::Allow, &["GET", "bad method"]);
            assert!(Allow::owned(&invalid_allow).is_err());

            assert!(AcceptOwned::try_from("text/plain").is_ok());
            assert!(AcceptEncodingOwned::try_from("gzip").is_ok());
            assert!(AcceptLanguageOwned::try_from("en").is_ok());
            assert!(AllowOwned::try_from("GET").is_ok());
            assert!(VaryOwned::try_from("accept").is_ok());

            assert!(AcceptOwned::try_from("line\nbreak").is_err());
            assert!(AcceptEncodingOwned::try_from("line\nbreak").is_err());
            assert!(AcceptLanguageOwned::try_from("line\nbreak").is_err());
            assert!(AllowOwned::try_from("line\nbreak").is_err());
            assert!(VaryOwned::try_from("line\nbreak").is_err());
            assert!(AcceptOwned::try_from(String::from("line\nbreak")).is_err());
            assert!(AcceptEncodingOwned::try_from(String::from("line\nbreak")).is_err());
            assert!(AcceptLanguageOwned::try_from(String::from("line\nbreak")).is_err());
            assert!(AllowOwned::try_from(String::from("line\nbreak")).is_err());
        }

        #[test]
        fn every_generated_list_type_exercises_its_owned_and_view_apis() {
            macro_rules! exercise {
                ($header:ty, $owned:ty, $name:expr, $line:expr, $items:expr) => {{
                    let source = Store::new($name, $line);
                    let view = <$header>::view(&source).expect("valid view").expect("present view");
                    assert_eq!(view.items().collect::<Vec<_>>(), $items);
                    assert_eq!(view.values().count(), $line.len());
                    assert!(!format!("{view:?}").is_empty());

                    let owned = <$header>::owned(&source)
                        .expect("valid owned value")
                        .expect("present owned value");
                    assert_eq!(owned.items().collect::<Vec<_>>(), $items);
                    assert_eq!(owned.values().len(), $line.len());
                    assert_eq!(owned, owned.clone());
                    assert!(!format!("{owned:?}").is_empty());
                    let mut hasher = DefaultHasher::new();
                    owned.hash(&mut hasher);
                    assert_ne!(hasher.finish(), 0);

                    assert!(<$owned>::try_from(String::from($line[0])).is_ok());
                    let mut sink = Store::default();
                    <$header>::insert(&mut sink, owned).expect("insert generated list");
                    assert_eq!(sink.values.len(), $line.len());
                }};
            }

            exercise!(
                AcceptEncoding,
                AcceptEncodingOwned,
                &FieldName::AcceptEncoding,
                &["gzip", "br"],
                [b"gzip".as_slice(), b"br".as_slice()]
            );
            exercise!(
                AcceptLanguage,
                AcceptLanguageOwned,
                &FieldName::AcceptLanguage,
                &["en", "fr"],
                [b"en".as_slice(), b"fr".as_slice()]
            );
            exercise!(
                Allow,
                AllowOwned,
                &FieldName::Allow,
                &["TRACE", "PATCH"],
                [b"TRACE".as_slice(), b"PATCH".as_slice()]
            );
            exercise!(
                Vary,
                VaryOwned,
                &FieldName::Vary,
                &["accept", "origin"],
                [b"accept".as_slice(), b"origin".as_slice()]
            );
        }

        #[test]
        fn checked_copy_and_quoted_line_validation_cover_line_boundaries() {
            let values = [
                FieldValue::from_static("alpha"),
                FieldValue::from_static("beta"),
                FieldValue::from_static("gamma"),
            ];
            let borrowed = FieldLines::from_slice(&FieldName::Accept, &values).expect("nonempty values");
            let mut indices = Vec::new();
            let copied = clone_checked_values(&borrowed, |index, value| {
                indices.push(index);
                validate_token(value.as_bytes())
            })
            .expect("all lines are tokens");
            assert_eq!(indices, [0, 1, 2]);
            assert!(matches!(copied, ListValues::Many(ref lines) if lines.len() == 3));

            let one = [FieldValue::from_static("alpha")];
            let borrowed = FieldLines::from_slice(&FieldName::Accept, &one).expect("one value");
            assert!(matches!(
                clone_checked_values(&borrowed, |_index, _value| Ok(())).expect("valid line"),
                ListValues::One(_)
            ));

            assert_eq!(
                check_quoted_values(&FieldName::Accept, &borrowed, validate_token)
                    .expect("one quoted line")
                    .expect("single line")
                    .as_bytes(),
                b"alpha"
            );
            let invalid_first = FieldLines::single(&FieldName::Accept, b"bad token");
            assert!(check_quoted_values(&FieldName::Accept, &invalid_first, validate_token).is_err());
            assert!(
                check_quoted_values(
                    &FieldName::Accept,
                    &FieldLines::from_slice(&FieldName::Accept, &values).expect("three values"),
                    validate_token,
                )
                .expect("three valid lines")
                .is_none()
            );
            let invalid_third = [
                FieldValue::from_static("alpha"),
                FieldValue::from_static("beta"),
                FieldValue::from_static("bad token"),
            ];
            assert!(
                check_quoted_values(
                    &FieldName::Accept,
                    &FieldLines::from_slice(&FieldName::Accept, &invalid_third).expect("three values"),
                    validate_token,
                )
                .is_err()
            );
            assert_eq!(
                check_quoted_value(
                    &FieldName::Accept,
                    FieldValue::from_static("\"unterminated").as_field_value_ref(),
                    validate_token,
                )
                .expect_err("unterminated quote")
                .kind(),
                DecodeErrorKind::UnterminatedQuote
            );

            let borrowed_all = FieldLines::from_slice(&FieldName::Accept, &values).expect("three values");
            for failing_index in 0..3 {
                let error = clone_checked_values(&borrowed_all, |index, _value| {
                    if index == failing_index {
                        Err(DecodeError::new(&FieldName::Accept, DecodeErrorKind::InvalidSyntax))
                    } else {
                        Ok(())
                    }
                })
                .expect_err("the selected line fails validation");
                assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
            }

            let invalid_lines = [FieldValue::from_static("alpha"), FieldValue::from_static("\"unterminated")];
            let invalid = FieldLines::from_slice(&FieldName::Accept, &invalid_lines).expect("two field lines");
            assert_eq!(
                check_quoted_values(&FieldName::Accept, &invalid, validate_token)
                    .expect_err("second line has an unterminated quote")
                    .kind(),
                DecodeErrorKind::UnterminatedQuote
            );
        }

        #[test]
        fn token_list_private_paths_cover_unusual_and_invalid_lines() {
            assert!(
                check_quoted_value(
                    &FieldName::Accept,
                    FieldValue::from_static("\"quoted\"").as_field_value_ref(),
                    validate_token,
                )
                .is_err()
            );
            let token_lines = [FieldValue::from_static("GET"), FieldValue::from_static("bad method")];
            let tokens = FieldLines::from_slice(&FieldName::Allow, &token_lines).expect("two token lines");
            assert_eq!(
                check_token_values(&FieldName::Allow, &tokens, validate_token)
                    .expect_err("second line is invalid")
                    .kind(),
                DecodeErrorKind::InvalidToken
            );

            let unusual = FieldLines::single(&FieldName::Allow, b"TRACE");
            assert_eq!(
                check_token_values(&FieldName::Allow, &unusual, validate_token)
                    .expect("valid uncommon token list")
                    .expect("single line")
                    .as_bytes(),
                b"TRACE"
            );
            let unusual_lines = [FieldValue::from_static("TRACE"), FieldValue::from_static("PATCH")];
            let unusual_repeated = FieldLines::from_slice(&FieldName::Allow, &unusual_lines).expect("two lines");
            assert!(
                check_token_values(&FieldName::Allow, &unusual_repeated, validate_token)
                    .expect("valid repeated uncommon token list")
                    .is_none()
            );
            let invalid_unusual = FieldLines::single(&FieldName::Allow, b"bad method");
            assert!(check_token_values(&FieldName::Allow, &invalid_unusual, validate_token).is_err());
            assert_eq!(
                check_token_value(
                    &FieldName::Allow,
                    FieldValue::from_static("bad method").as_field_value_ref(),
                    validate_token,
                )
                .expect_err("single invalid token line")
                .kind(),
                DecodeErrorKind::InvalidToken
            );
            assert!(
                check_token_value(
                    &FieldName::Allow,
                    FieldValue::from_static("TRACE").as_field_value_ref(),
                    validate_token,
                )
                .is_ok()
            );
        }

        #[test]
        fn weighted_tokens_parameters_and_qvalues_cover_strict_and_relaxed_grammar() {
            for valid in [b"gzip".as_slice(), b"gzip;q=0.5"] {
                assert!(
                    validate_weighted_token(valid, &FieldName::AcceptEncoding, validate::token, false).is_ok(),
                    "{valid:?}"
                );
            }
            assert!(validate_weighted_token(b"gzip; q = .1234", &FieldName::AcceptEncoding, validate::token, true,).is_ok());
            for invalid in [
                b"".as_slice(),
                b"bad token",
                b"gzip;level=1",
                b"gzip;q=0.5;level=1",
                b"gzip;q=\"unterminated",
            ] {
                assert!(
                    validate_weighted_token(invalid, &FieldName::AcceptEncoding, validate::token, false,).is_err(),
                    "{invalid:?}"
                );
            }

            assert_eq!(
                parse_parameter(b" name = \"a b\" ", false, &FieldName::Accept).expect("quoted parameter"),
                (b"name".as_slice(), Some(b"\"a b\"".as_slice()), false)
            );
            assert_eq!(
                parse_parameter(b"flag", true, &FieldName::Accept).expect("optional value"),
                (b"flag".as_slice(), None, true)
            );
            assert!(parse_parameter(b"flag", false, &FieldName::Accept).is_err());
            assert!(parse_parameter(b"bad name=x", false, &FieldName::Accept).is_err());
            assert!(parse_parameter(b"name=\"bad\\\x01\"", false, &FieldName::Accept).is_err());

            for valid in [b"\"\"".as_slice(), b"\"a b\"", b"\"a\\\"b\"", b"\"\xff\""] {
                assert!(valid_quoted_string(valid), "{valid:?}");
            }
            assert!(valid_quoted_string(b"\"a\\\xff\""));
            for invalid in [
                b"".as_slice(),
                b"token",
                b"\"unterminated",
                b"\"bad\\\x01\"",
                b"\"bad\x01\"",
                b"\"bad\\\"",
            ] {
                assert!(!valid_quoted_string(invalid), "{invalid:?}");
            }

            for valid in [b"0".as_slice(), b"1", b"0.123", b"1.000"] {
                assert!(validate_quality(valid, &FieldName::Accept, false).is_ok());
            }
            for invalid in [b"2".as_slice(), b"1.001", b"0.1234", b".5"] {
                assert!(validate_quality(invalid, &FieldName::Accept, false).is_err());
            }
            for valid in [b" .5 ".as_slice(), b"0.1234", b"1.0000"] {
                assert!(validate_quality(valid, &FieldName::Accept, true).is_ok());
            }
            assert!(validate_quality(b"1.0001", &FieldName::Accept, true).is_err());
            assert!(validate_quality(b"2", &FieldName::Accept, true).is_err());
            assert!(validate_quality(b"1", &FieldName::Accept, true).is_ok());
            assert!(validate_weighted_token(b"gzip;bad", &FieldName::AcceptEncoding, validate::token, false,).is_err());
        }

        #[test]
        fn direct_quoted_weight_and_plain_list_paths_cover_fallbacks() {
            validate_weighted_token_quoted(b"gzip;q=0.5", &FieldName::AcceptEncoding, validate::token, false)
                .expect("direct quoted-path validation");
            for invalid in [
                b"bad token;q=0.5".as_slice(),
                b"gzip;level=1",
                b"gzip;bad",
                b"gzip;q=2",
                b"gzip;q=0.5;extra=1",
                b"gzip;q=\"unterminated",
                b"gzip;q=0.5;\"unterminated",
                b"\"unterminated",
            ] {
                assert!(
                    validate_weighted_token_quoted(invalid, &FieldName::AcceptEncoding, validate::token, false,).is_err(),
                    "{invalid:?}"
                );
            }

            assert!(is_well_known_negotiation_line(&FieldName::AcceptEncoding, b"gzip"));
            assert!(is_well_known_negotiation_line(&FieldName::AcceptLanguage, b"en-US"));
            // Multi-member lines vary too much between clients to be worth a
            // literal, so they reach the member grammar instead.
            for line in [&b"gzip, br"[..], b"gzip, br;q=0.8"] {
                assert!(!is_well_known_negotiation_line(&FieldName::AcceptEncoding, line));
            }
            assert!(!is_well_known_negotiation_line(&FieldName::AcceptLanguage, b"en, en-US;q=0.8"));
            assert!(!is_well_known_negotiation_line(&FieldName::Vary, b"accept"));

            let mut items = Vec::new();
            assert!(
                try_plain_items(b", alpha, , beta", b',', true, |item| {
                    items.push(item.to_vec());
                    Ok(())
                })
                .expect("plain list")
            );
            assert_eq!(items, [b"alpha".to_vec(), b"beta".to_vec()]);
            assert!(!try_plain_items(b"alpha,\"beta\"", b',', true, |_item| Ok(())).expect("quoted fallback"));
            assert!(try_plain_items(b"alpha,", b',', true, validate_token).expect("trailing empty item"));
            assert!(try_plain_items(b"bad method,GET", b',', true, validate_token).is_err());
            validate_weighted_token_quoted(b"gzip", &FieldName::AcceptEncoding, validate::token, false)
                .expect("quoted parser without a weight");
        }

        #[test]
        fn quoted_iterator_and_low_level_word_helpers_cover_edge_shapes() {
            let items = QuotedItems::comma(b", alpha, \"beta,gamma\",,", &FieldName::Accept)
                .collect::<Result<Vec<_>, _>>()
                .expect("quoted commas stay within an item");
            assert_eq!(items, [b"alpha".as_slice(), b"\"beta,gamma\""]);

            let items = QuotedItems::semicolon(b"alpha;\"beta\\\"gamma\";", &FieldName::Accept)
                .collect::<Result<Vec<_>, _>>()
                .expect("escaped quote");
            assert_eq!(items, [b"alpha".as_slice(), b"\"beta\\\"gamma\"", b""]);
            assert_eq!(
                QuotedItems::comma(b"\"unfinished", &FieldName::Accept)
                    .next()
                    .expect("one error")
                    .expect_err("quote is unfinished")
                    .kind(),
                DecodeErrorKind::UnterminatedQuote
            );

            assert_eq!(word(b"abcdefgh", 0), u64::from_le_bytes(*b"abcdefgh"));
            assert_eq!(word(b"short", 0), 0);
            assert_eq!(half_word(b"ab", 0), u16::from_le_bytes(*b"ab"));
            assert_eq!(half_word(b"a", 0), 0);
            assert!(equals_word_at_a_time(b"short", b"short"));
            assert!(!equals_word_at_a_time(b"short", b"longer"));
            assert!(equals_word_at_a_time(b"123456789", b"123456789"));
            assert!(!equals_word_at_a_time(b"123456789", b"123456780"));
            assert!(equals_word_at_a_time(b"1234567890", b"1234567890"));
            assert!(equals_word_at_a_time(b"12345678901", b"12345678901"));

            assert_eq!(
                token_list_error(&FieldName::Allow, b"alpha,\"unfinished", validate_token, Some(2),).kind(),
                DecodeErrorKind::UnterminatedQuote
            );
            assert_eq!(
                token_list_error(&FieldName::Allow, b"\"unfinished", validate_token, None,).kind(),
                DecodeErrorKind::UnterminatedQuote
            );
            assert_eq!(
                token_list_error(&FieldName::Allow, b"bad token", validate_token, None).kind(),
                DecodeErrorKind::InvalidToken
            );
            assert_eq!(
                token_list_error(&FieldName::Allow, b"GET", validate_token, None).kind(),
                DecodeErrorKind::InvalidToken
            );
        }
    }

    #[test]
    fn the_well_known_table_never_accepts_an_invalid_line() {
        let mut mutated = Vec::new();
        for line in well_known_lines() {
            for position in 0..line.len() {
                for replacement in 0..=u8::MAX {
                    mutated.clear();
                    mutated.extend_from_slice(line);
                    mutated[position] = replacement;
                    assert!(
                        !is_well_known_token_list(&mutated) || scan_token_list_general(&mutated),
                        "{:?}",
                        mutated.escape_ascii().to_string()
                    );
                }
            }
            for position in 0..line.len() {
                mutated.clear();
                mutated.extend_from_slice(line);
                mutated.remove(position);
                assert!(
                    !is_well_known_token_list(&mutated) || scan_token_list_general(&mutated),
                    "{:?}",
                    mutated.escape_ascii().to_string()
                );
            }
        }
    }

    #[test]
    fn fast_scan_agrees_with_the_delimited_scan() {
        let alphabet: &[u8] = b"a,; \t\"\\!\x00\x80";
        for length in 0..=4 {
            let mut counters = vec![0_usize; length];
            let mut input = vec![0_u8; length];
            loop {
                for (slot, counter) in input.iter_mut().zip(counters.iter()) {
                    *slot = alphabet[*counter];
                }
                assert_eq!(
                    scan_token_list(&input),
                    delimited_scan(&input),
                    "{:?}",
                    input.escape_ascii().to_string()
                );
                let mut position = 0;
                loop {
                    if position == counters.len() {
                        break;
                    }
                    counters[position] += 1;
                    if counters[position] < alphabet.len() {
                        break;
                    }
                    counters[position] = 0;
                    position += 1;
                }
                if position == counters.len() {
                    break;
                }
            }
        }

        for value in [
            &b"GET, POST"[..],
            b"accept-encoding, origin",
            b"a-very-long-token-name, another-token, third",
            b"abcdefgh",
            b"abcdefg h",
            b"abcdefgh,",
            b"abcdefghi",
            b"abcdefgh ,i",
            b"abc\x7fdefgh",
            b"abcdefg\x7f",
        ] {
            assert_eq!(
                scan_token_list(value),
                delimited_scan(value),
                "{:?}",
                value.escape_ascii().to_string()
            );
        }
    }

    #[test]
    fn fast_scan_agrees_on_lines_long_enough_to_vectorize() {
        let base: &[u8] = b"alpha, beta, gamma, delta, epsilon, zeta, eta, theta, iota, kappa, mu";
        let alphabet: &[u8] = b"a,; \t\"\\!\x00\x80\x7f";
        let mut mutated = Vec::new();
        for length in 0..=base.len() {
            let line = &base[..length];
            assert_eq!(scan_token_list(line), delimited_scan(line), "{:?}", line.escape_ascii().to_string());
            for position in 0..length {
                for replacement in alphabet {
                    mutated.clear();
                    mutated.extend_from_slice(line);
                    mutated[position] = *replacement;
                    assert_eq!(
                        scan_token_list(&mutated),
                        delimited_scan(&mutated),
                        "{:?}",
                        mutated.escape_ascii().to_string()
                    );
                }
            }
        }

        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        for _ in 0..200_000 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let length = usize::try_from(state >> 57).expect("a 7-bit length fits a usize");
            mutated.clear();
            for _ in 0..length {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let pick = usize::try_from(state >> 40).expect("a 24-bit index fits a usize");
                mutated.push(alphabet[pick % alphabet.len()]);
            }
            assert_eq!(
                scan_token_list(&mutated),
                delimited_scan(&mutated),
                "{:?}",
                mutated.escape_ascii().to_string()
            );
        }
    }
}
