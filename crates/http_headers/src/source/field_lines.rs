// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Field lines supplied by a field source for one field name.

use std::iter::FusedIterator;
use std::{fmt, mem, slice};

use crate::source::{DelimitedItems, update_list_item_count};
use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef};

/// Maximum total field-value bytes accepted from a custom [`FieldSource`](crate::source::FieldSource).
///
/// The limit is enforced before typed decoding accepts custom-source bytes.
/// The validated `http::HeaderMap` adapter is exempt, so callers using it must
/// enforce suitable aggregate header limits at the transport or server layer.
/// Exceeding this budget returns [`DecodeErrorKind::SourceLimitExceeded`].
pub const MAX_CUSTOM_FIELD_BYTES: usize = 64 * 1024;

/// Maximum field lines accepted for one name from a custom [`FieldSource`](crate::source::FieldSource).
///
/// The limit is enforced before typed decoding accepts custom-source lines.
/// The validated `http::HeaderMap` adapter is exempt, so callers using it must
/// enforce suitable aggregate header limits at the transport or server layer.
/// Exceeding this budget returns [`DecodeErrorKind::SourceLimitExceeded`].
pub const MAX_CUSTOM_FIELD_LINES: usize = 128;

/// Maximum parsed list items accepted during one custom-source field decode.
///
/// [`DelimitedItems`] and specialized list parsers enforce this limit before
/// accepting a further item. The validated `http::HeaderMap` adapter is exempt,
/// so callers using it must enforce suitable aggregate header limits at the
/// transport or server layer.
/// Exceeding this budget returns [`DecodeErrorKind::SourceLimitExceeded`].
pub const MAX_CUSTOM_LIST_ITEMS: usize = 1_024;

/// The storage a [`FieldLines`] iterates.
///
/// The set of representations is closed so that every decoder dispatches
/// statically: a field source hands out one of these shapes, and no decoding
/// path ever goes through a trait object.
enum Repr<'a> {
    /// Exactly one field line, borrowed from anywhere.
    Single(&'a [u8]),
    /// One [`FieldValue`] per field line, stored contiguously.
    Slice(&'a [FieldValue]),
    /// Field lines borrowing arbitrary backing storage.
    Borrowed(&'a [FieldValueRef<'a>]),
    /// The field lines an `http::HeaderMap` stores for one name.
    #[cfg(feature = "http")]
    Http(http::header::GetAll<'a, http::HeaderValue>),
}

/// The raw field lines stored under one field name.
///
/// A [`crate::source::FieldSource`] returns this type for a present field. An
/// instance always contains at least one field line; absence is represented by
/// `None`. Line order is preserved, a zero-length line remains a present line,
/// and the lines can be iterated repeatedly.
///
/// The associated name is a static descriptor, independent of the lifetime of
/// the field bytes. Locally constructed runtime names cannot be passed to the
/// constructors; see [`crate::source::FieldSource`].
///
/// # Examples
///
/// ```rust
/// use http_headers::FieldName;
/// use http_headers::source::FieldLines;
///
/// let lines = FieldLines::single(&FieldName::UserAgent, b"client/1");
/// assert_eq!(lines.len(), 1);
/// assert_eq!(lines.exactly_one()?.as_bytes(), b"client/1");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct FieldLines<'a> {
    name: &'static FieldName,
    repr: Repr<'a>,
}

const _: [(); mem::size_of::<FieldLines<'static>>()] = [(); mem::size_of::<Option<FieldLines<'static>>>()];

impl fmt::Debug for FieldLines<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FieldLines")
            .field("name", &self.name)
            .field("line_count", &self.len())
            .finish()
    }
}

impl<'a> FieldLines<'a> {
    /// Creates field lines holding exactly one line.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldName;
    /// use http_headers::source::FieldLines;
    ///
    /// assert_eq!(
    ///     FieldLines::single(&FieldName::Accept, b"text/html").len(),
    ///     1
    /// );
    /// ```
    #[must_use]
    #[inline]
    pub const fn single(name: &'static FieldName, value: &'a [u8]) -> Self {
        Self {
            name,
            repr: Repr::Single(value),
        }
    }

    /// Creates field lines over a contiguous slice of [`FieldValue`]s.
    ///
    /// Returns `None` when the slice contains no field lines.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::source::FieldLines;
    /// use http_headers::{FieldName, FieldValue};
    ///
    /// let stored = [FieldValue::from_static("text/html")];
    /// let lines = FieldLines::from_slice(&FieldName::Accept, &stored);
    /// assert_eq!(lines.map(|lines| lines.len()), Some(1));
    /// ```
    #[must_use]
    #[inline]
    pub const fn from_slice(name: &'static FieldName, values: &'a [FieldValue]) -> Option<Self> {
        if values.is_empty() {
            None
        } else {
            Some(Self {
                name,
                repr: Repr::Slice(values),
            })
        }
    }

    /// Creates field lines from borrowed lines, one per line.
    ///
    /// Sensitivity markers on the supplied lines are preserved. Returns
    /// `None` when the slice contains no field lines.
    #[must_use]
    #[inline]
    pub const fn from_borrowed(name: &'static FieldName, values: &'a [FieldValueRef<'a>]) -> Option<Self> {
        if values.is_empty() {
            None
        } else {
            Some(Self {
                name,
                repr: Repr::Borrowed(values),
            })
        }
    }

    /// Creates field lines over what an `http::HeaderMap` stores.
    ///
    /// Returns `None` when the map contains no field line for the name.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "http")]
    /// # fn main() {
    /// use http_headers::FieldName;
    /// use http_headers::source::FieldLines;
    ///
    /// let mut map = http::HeaderMap::new();
    /// map.append(
    ///     http::header::ACCEPT,
    ///     http::HeaderValue::from_static("text/html"),
    /// );
    /// let lines = FieldLines::from_http(&FieldName::Accept, map.get_all(http::header::ACCEPT));
    /// assert_eq!(lines.map(|lines| lines.len()), Some(1));
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    #[cfg(feature = "http")]
    #[must_use]
    #[inline]
    pub fn from_http(name: &'static FieldName, values: http::header::GetAll<'a, http::HeaderValue>) -> Option<Self> {
        if values.iter().next().is_none() {
            None
        } else {
            Some(Self {
                name,
                repr: Repr::Http(values),
            })
        }
    }

    /// Returns the associated field name.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldName;
    /// use http_headers::source::FieldLines;
    ///
    /// let lines = FieldLines::single(&FieldName::Accept, b"");
    /// assert_eq!(lines.name().as_str(), "accept");
    /// ```
    #[must_use]
    #[inline]
    pub const fn name(&self) -> &'static FieldName {
        self.name
    }

    /// Returns the value of the only field line, or a cardinality error.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no field line or more than one.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldName;
    /// use http_headers::source::FieldLines;
    ///
    /// let lines = FieldLines::single(&FieldName::UserAgent, b"client/1");
    /// assert_eq!(lines.exactly_one()?.as_bytes(), b"client/1");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[expect(
        clippy::inline_always,
        reason = "typed singleton decoding must eliminate the known source representation and intermediate result"
    )]
    #[inline(always)]
    pub fn exactly_one(self) -> Result<FieldValueRef<'a>, DecodeError> {
        // Specialized per representation rather than routed through
        // `repeated()`: the cardinality of a slice is already known, and
        // `Repr::Single` is by construction exactly one line.
        let multiple = || DecodeError::new(self.name, DecodeErrorKind::UnexpectedMultipleValues);
        let first = match &self.repr {
            Repr::Single(bytes) => return Ok(FieldValueRef::new(bytes)),
            Repr::Slice(values) => match values {
                [only] => Some(only.as_field_value_ref()),
                [] => None,
                _ => return Err(multiple()),
            },
            Repr::Borrowed(values) => match values {
                [only] => Some(*only),
                [] => None,
                _ => return Err(multiple()),
            },
            #[cfg(feature = "http")]
            Repr::Http(values) => {
                let mut lines = values.iter();
                let first = lines.next().map(FieldValueRef::from);
                if first.is_some() && lines.next().is_some() {
                    return Err(multiple());
                }
                first
            }
        };
        first.ok_or_else(|| DecodeError::new(self.name, DecodeErrorKind::MissingValue))
    }

    /// Returns the value of the only field line as a representation-aware
    /// owned value, or a cardinality error.
    ///
    /// This never builds an intermediate borrowed [`FieldValueRef`]: each
    /// [`Repr`] variant is specialized directly to the cheapest owned value
    /// it can produce. Short lines are stored inline, so the common case
    /// allocates nothing at all; longer ones reuse the source's shared
    /// storage where it exists — [`Repr::Slice`] clones the stored
    /// [`FieldValue`] — and copy otherwise. [`Repr::Single`] and
    /// [`Repr::Borrowed`] are also the only representations that revalidate,
    /// because they are the only ones whose bytes did not arrive inside an
    /// already-validated value.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no field line, if there is more than one,
    /// if a source handed out bytes that are not a valid field value, or if a
    /// custom source exceeds [`MAX_CUSTOM_FIELD_BYTES`] or
    /// [`MAX_CUSTOM_FIELD_LINES`] (reported as [`DecodeErrorKind::SourceLimitExceeded`]).
    #[inline]
    pub(crate) fn exactly_one_owned(&self) -> Result<FieldValue, DecodeError> {
        self.validate_custom_source()?;
        match &self.repr {
            Repr::Single(bytes) => Ok(FieldValueRef::new(bytes).to_validated_field_value()),
            Repr::Slice(values) => match values {
                [value] => Ok(value.clone()),
                _ => Err(DecodeError::new(self.name, DecodeErrorKind::UnexpectedMultipleValues)),
            },
            Repr::Borrowed(values) => match values {
                [value] => Ok(value.to_validated_field_value()),
                _ => Err(DecodeError::new(self.name, DecodeErrorKind::UnexpectedMultipleValues)),
            },
            #[cfg(feature = "http")]
            Repr::Http(values) => {
                let mut values = values.iter();
                let first = values
                    .next()
                    .ok_or_else(|| DecodeError::new(self.name, DecodeErrorKind::MissingValue))?;
                if values.next().is_some() {
                    return Err(DecodeError::new(self.name, DecodeErrorKind::UnexpectedMultipleValues));
                }
                Ok(FieldValue::from(first))
            }
        }
    }

    /// Iterates the raw field lines in insertion order.
    ///
    /// This is equivalent to [`Self::repeated`] and iteration over `&FieldLines`.
    /// Line boundaries and sensitivity markers are preserved without parsing
    /// or validating the bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_headers::FieldName;
    /// use http_headers::source::FieldLines;
    ///
    /// let lines = FieldLines::single(&FieldName::SetCookie, b"a=1");
    /// assert_eq!(lines.iter().next().unwrap().as_bytes(), b"a=1");
    /// assert_eq!(lines.iter().count(), lines.repeated().count());
    /// ```
    #[must_use]
    #[inline]
    pub fn iter(&self) -> FieldLinesIter<'a> {
        self.repeated()
    }

    /// Reiterates the field lines in insertion order.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldName;
    /// use http_headers::source::FieldLines;
    ///
    /// let lines = FieldLines::single(&FieldName::SetCookie, b"a=1");
    /// assert_eq!(lines.repeated().count(), 1);
    /// for line in &lines {
    ///     assert_eq!(line.as_bytes(), b"a=1");
    /// }
    /// ```
    #[must_use]
    #[inline]
    pub fn repeated(&self) -> FieldLinesIter<'a> {
        FieldLinesIter {
            repr: match &self.repr {
                Repr::Single(value) => LinesRepr::Single(Some(value)),
                Repr::Slice(values) => LinesRepr::Slice(values.iter()),
                Repr::Borrowed(values) => LinesRepr::Borrowed(values.iter()),
                #[cfg(feature = "http")]
                Repr::Http(values) => LinesRepr::Http(values.iter()),
            },
        }
    }

    /// Reiterates the field lines in insertion order, pairing each borrowed
    /// view with a representation-aware owned clone.
    ///
    /// This is what an owned decoder should walk instead of calling
    /// [`FieldValueRef::to_field_value`] on the output of [`Self::repeated`]:
    /// it keeps [`Repr::Slice`] from paying for a byte copy that a cheap
    /// clone would have avoided.
    ///
    /// # Errors
    ///
    /// Returns an invalid-syntax error when raw single or borrowed lines
    /// contain bytes that cannot form an HTTP field value. Returns
    /// [`DecodeErrorKind::SourceLimitExceeded`] when a custom source exceeds
    /// [`MAX_CUSTOM_FIELD_BYTES`] or [`MAX_CUSTOM_FIELD_LINES`].
    #[inline]
    #[cfg(any(
        test,
        feature = "headers-cache-control",
        feature = "headers-conditional",
        feature = "headers-cors",
        feature = "headers-negotiation",
        feature = "headers-range",
        feature = "headers-security",
        feature = "headers-set-cookie",
        feature = "headers-websocket",
    ))]
    pub(crate) fn repeated_owned(&self) -> Result<FieldLinesOwnedIter<'a>, DecodeError> {
        self.validate_custom_source()?;
        Ok(FieldLinesOwnedIter {
            repr: match &self.repr {
                Repr::Single(value) => OwnedLinesRepr::Single(Some(value)),
                Repr::Slice(values) => OwnedLinesRepr::Slice(values.iter()),
                Repr::Borrowed(values) => OwnedLinesRepr::Borrowed(values.iter()),
                #[cfg(feature = "http")]
                Repr::Http(values) => OwnedLinesRepr::Http(values.iter()),
            },
        })
    }

    /// Returns the number of field lines, which is always at least one.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldName;
    /// use http_headers::source::FieldLines;
    ///
    /// assert_eq!(FieldLines::single(&FieldName::SetCookie, b"a=1").len(), 1);
    /// ```
    #[must_use]
    #[inline]
    #[expect(
        clippy::len_without_is_empty,
        reason = "FieldLines is non-empty by construction; absence is represented by Option"
    )]
    pub fn len(&self) -> usize {
        match &self.repr {
            Repr::Single(_) => 1,
            Repr::Slice(values) => values.len(),
            Repr::Borrowed(values) => values.len(),
            #[cfg(feature = "http")]
            Repr::Http(values) => values.iter().count(),
        }
    }

    /// Iterates comma-delimited items across all field lines.
    ///
    /// Items are yielded in order across field-line boundaries without
    /// materializing a combined value. Each physical line is scanned
    /// independently, so a quoted string or escape cannot span lines.
    ///
    /// This follows the HTTP list-rule convention: empty items produced by
    /// consecutive or trailing commas are silently skipped rather than
    /// yielded.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldName;
    /// use http_headers::source::FieldLines;
    ///
    /// let lines = FieldLines::single(&FieldName::Vary, b"accept, , origin");
    /// assert_eq!(lines.comma_items().count(), 2);
    /// ```
    #[must_use]
    #[inline]
    pub fn comma_items(&self) -> DelimitedItems<'a> {
        DelimitedItems::new(self, b',')
    }

    /// Iterates comma members after source preflight has already succeeded.
    ///
    /// The caller must have established that `validate_custom_source()` succeeds
    /// for these exact retained lines, not for another call to `FieldSource::lines`.
    /// Only source bounds and field-value preflight are omitted: quoting and
    /// item limits remain checked.
    /// Debug builds recheck the caller's precondition.
    #[cfg(any(test, feature = "headers-negotiation"))]
    pub(crate) fn validated_comma_items(&self) -> DelimitedItems<'a> {
        #[cfg(debug_assertions)]
        self.validate_custom_source()
            .expect("the caller must retain the exact lines whose source preflight is known to succeed");
        DelimitedItems::from_validated_source(self, b',')
    }

    /// Iterates semicolon-delimited items across all field lines.
    ///
    /// Unlike [`Self::comma_items`], empty items are yielded rather than
    /// skipped, allowing the caller to decide whether they are valid for the
    /// field being parsed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldName;
    /// use http_headers::source::FieldLines;
    ///
    /// let lines = FieldLines::single(&FieldName::ContentType, b"text/html; charset=utf-8");
    /// assert_eq!(lines.semicolon_items().count(), 2);
    /// ```
    #[must_use]
    pub fn semicolon_items(&self) -> DelimitedItems<'a> {
        DelimitedItems::new(self, b';')
    }

    #[cfg(feature = "http")]
    pub(crate) const fn has_custom_source_limits(&self) -> bool {
        !matches!(&self.repr, Repr::Http(_))
    }

    #[cfg(not(feature = "http"))]
    pub(crate) const fn has_custom_source_limits(&self) -> bool {
        matches!(&self.repr, Repr::Single(_) | Repr::Slice(_) | Repr::Borrowed(_))
    }

    #[inline]
    pub(crate) fn validate_custom_source_bounds(&self) -> Result<(), DecodeError> {
        if !self.has_custom_source_limits() {
            return Ok(());
        }
        self.validate_bounded_source()
    }

    fn validate_bounded_source(&self) -> Result<(), DecodeError> {
        let limit = || DecodeError::new(self.name, DecodeErrorKind::SourceLimitExceeded);
        if self.len() > MAX_CUSTOM_FIELD_LINES {
            return Err(limit());
        }

        let validate_bytes = matches!(&self.repr, Repr::Single(_) | Repr::Borrowed(_));
        let mut total_bytes = 0_usize;
        for value in self.repeated() {
            let bytes = value.as_bytes();
            total_bytes = total_bytes.checked_add(bytes.len()).ok_or_else(limit)?;
            if total_bytes > MAX_CUSTOM_FIELD_BYTES {
                return Err(limit());
            }
            if validate_bytes && !crate::validate::field_value(bytes) {
                return Err(DecodeError::new(self.name, DecodeErrorKind::InvalidSyntax));
            }
        }
        Ok(())
    }

    #[inline]
    pub(crate) fn validate_custom_source(&self) -> Result<(), DecodeError> {
        self.validate_custom_source_bounds()
    }

    pub(crate) fn validate_list_item_limit(&self, delimiter: u8, skip_empty: bool) -> Result<(), DecodeError> {
        self.validate_list_item_limit_with(delimiter, skip_empty, true)
    }

    #[cfg(any(test, feature = "headers-conditional"))]
    pub(crate) fn validate_entity_tag_item_limit(&self) -> Result<(), DecodeError> {
        self.validate_list_item_limit_with(b',', true, false)
    }

    #[inline]
    fn validate_list_item_limit_with(&self, delimiter: u8, skip_empty: bool, backslash_escapes: bool) -> Result<(), DecodeError> {
        if !self.has_custom_source_limits() {
            return Ok(());
        }
        self.validate_bounded_list(delimiter, skip_empty, backslash_escapes)
    }

    fn validate_bounded_list(&self, delimiter: u8, skip_empty: bool, backslash_escapes: bool) -> Result<(), DecodeError> {
        let limit = || DecodeError::new(self.name, DecodeErrorKind::SourceLimitExceeded);
        if self.len() > MAX_CUSTOM_FIELD_LINES {
            return Err(limit());
        }

        let validate_bytes = matches!(&self.repr, Repr::Single(_) | Repr::Borrowed(_));
        let mut total_bytes = 0_usize;
        let mut item_count = 0_usize;
        for value in self.repeated() {
            let bytes = value.as_bytes();
            total_bytes = total_bytes.checked_add(bytes.len()).ok_or_else(limit)?;
            if total_bytes > MAX_CUSTOM_FIELD_BYTES {
                return Err(limit());
            }
            if validate_bytes && !crate::validate::field_value(bytes) {
                return Err(DecodeError::new(self.name, DecodeErrorKind::InvalidSyntax));
            }
            update_list_item_count(self.name, bytes, delimiter, skip_empty, backslash_escapes, &mut item_count)?;
        }
        Ok(())
    }
}

impl<'a> IntoIterator for &FieldLines<'a> {
    type Item = FieldValueRef<'a>;
    type IntoIter = FieldLinesIter<'a>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// The storage a [`FieldLinesIter`] iterator walks.
enum LinesRepr<'a> {
    /// At most one remaining field line.
    Single(Option<&'a [u8]>),
    /// Remaining [`FieldValue`] lines.
    Slice(slice::Iter<'a, FieldValue>),
    /// Remaining field lines borrowing arbitrary backing storage.
    Borrowed(slice::Iter<'a, FieldValueRef<'a>>),
    /// Remaining `http` field lines.
    #[cfg(feature = "http")]
    Http(http::header::ValueIter<'a, http::HeaderValue>),
}

/// An iterator over the field lines stored under one field name.
///
/// Yields one [`FieldValueRef`] per field line, in insertion order: the
/// `field-value` each line carries, never the comma-joined combination of
/// them. Field-line boundaries are preserved.
///
/// # Examples
///
/// ```rust
/// use http_headers::FieldName;
/// use http_headers::source::{FieldLines, FieldLinesIter};
///
/// let lines = FieldLines::single(&FieldName::SetCookie, b"a=1");
/// let mut iter: FieldLinesIter<'_> = lines.repeated();
/// assert_eq!(iter.size_hint(), (1, Some(1)));
/// assert_eq!(
///     iter.next().map(|line| line.as_bytes()),
///     Some(b"a=1".as_slice())
/// );
/// assert!(iter.next().is_none());
/// ```
pub struct FieldLinesIter<'a> {
    repr: LinesRepr<'a>,
}

impl fmt::Debug for FieldLinesIter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FieldLinesIter").finish_non_exhaustive()
    }
}

/// Re-borrows a stored value for [`LinesRepr::Slice`].
///
/// Kept out of line so that the three remaining arms stay small enough for
/// `next` to inline into a decoder's field-line loop. Inlining the whole
/// `FieldValue` representation here instead costs every other source a call
/// per line, which outweighs the call this leaves on the slice path.
#[inline(never)]
fn slice_line(value: &FieldValue) -> FieldValueRef<'_> {
    value.as_field_value_ref()
}

impl<'a> Iterator for FieldLinesIter<'a> {
    type Item = FieldValueRef<'a>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.repr {
            LinesRepr::Single(value) => value.take().map(FieldValueRef::new),
            LinesRepr::Slice(values) => values.next().map(slice_line),
            LinesRepr::Borrowed(values) => values.next().copied(),
            #[cfg(feature = "http")]
            LinesRepr::Http(values) => values.next().map(FieldValueRef::from),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.repr {
            LinesRepr::Single(value) => {
                let length = usize::from(value.is_some());
                (length, Some(length))
            }
            LinesRepr::Slice(values) => values.size_hint(),
            LinesRepr::Borrowed(values) => values.size_hint(),
            #[cfg(feature = "http")]
            LinesRepr::Http(values) => values.size_hint(),
        }
    }
}

impl FusedIterator for FieldLinesIter<'_> {}

/// The storage a [`FieldLinesOwnedIter`] iterator walks.
#[cfg(any(
    test,
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-cors",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-set-cookie",
    feature = "headers-websocket",
))]
enum OwnedLinesRepr<'a> {
    /// At most one remaining field line.
    Single(Option<&'a [u8]>),
    /// Remaining [`FieldValue`] lines.
    Slice(slice::Iter<'a, FieldValue>),
    /// Remaining field lines borrowing arbitrary backing storage.
    Borrowed(slice::Iter<'a, FieldValueRef<'a>>),
    /// Remaining `http` field lines.
    #[cfg(feature = "http")]
    Http(http::header::ValueIter<'a, http::HeaderValue>),
}

/// An iterator pairing each field line stored under one field name with a
/// representation-aware owned clone.
///
/// The borrowed half of each item is what a decoder validates or parses; the
/// owned half is what it stores. Cloning the owned half costs no byte copy for
/// [`Repr::Slice`], which already holds shareable storage. Every other
/// representation copies: [`Repr::Single`] and [`Repr::Borrowed`] have only
/// bytes to work from, and with the `http` feature [`Repr::Http`] can reach
/// `HeaderValue` only through `as_bytes`, since `http` keeps the `Bytes`
/// behind it private.
#[cfg(any(
    test,
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-cors",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-set-cookie",
    feature = "headers-websocket",
))]
pub(crate) struct FieldLinesOwnedIter<'a> {
    repr: OwnedLinesRepr<'a>,
}

#[cfg(any(
    test,
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-cors",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-set-cookie",
    feature = "headers-websocket",
))]
impl fmt::Debug for FieldLinesOwnedIter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FieldLinesOwnedIter").finish_non_exhaustive()
    }
}

#[cfg(any(
    test,
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-cors",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-set-cookie",
    feature = "headers-websocket",
))]
impl<'a> Iterator for FieldLinesOwnedIter<'a> {
    type Item = (FieldValueRef<'a>, FieldValue);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.repr {
            OwnedLinesRepr::Single(value) => value.take().map(|bytes| {
                let value = FieldValueRef::new(bytes);
                (value, value.to_validated_field_value())
            }),
            OwnedLinesRepr::Slice(values) => values.next().map(|value| (value.as_field_value_ref(), value.clone())),
            OwnedLinesRepr::Borrowed(values) => values.next().copied().map(|value| (value, value.to_validated_field_value())),
            #[cfg(feature = "http")]
            OwnedLinesRepr::Http(values) => values.next().map(|value| (FieldValueRef::from(value), FieldValue::from(value))),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.repr {
            OwnedLinesRepr::Single(value) => {
                let length = usize::from(value.is_some());
                (length, Some(length))
            }
            OwnedLinesRepr::Slice(values) => values.size_hint(),
            OwnedLinesRepr::Borrowed(values) => values.size_hint(),
            #[cfg(feature = "http")]
            OwnedLinesRepr::Http(values) => values.size_hint(),
        }
    }
}

#[cfg(any(
    test,
    feature = "headers-cache-control",
    feature = "headers-conditional",
    feature = "headers-cors",
    feature = "headers-negotiation",
    feature = "headers-range",
    feature = "headers-security",
    feature = "headers-set-cookie",
    feature = "headers-websocket",
))]
impl FusedIterator for FieldLinesOwnedIter<'_> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{FieldLines, Repr, update_list_item_count};
    use crate::headers::SetCookie;
    use crate::source::FieldSource;
    use crate::{DecodeErrorKind, Field, FieldName, FieldValue, FieldValueRef};

    #[test]
    fn set_cookie_handles_an_internal_empty_field_lines_representation() {
        struct EmptySource;

        impl FieldSource for EmptySource {
            fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
                Some(FieldLines {
                    name,
                    repr: Repr::Slice(&[]),
                })
            }
        }

        let source = EmptySource;
        let view = <SetCookie as Field>::view(&source).unwrap().unwrap();
        assert_eq!(view.len(), 0);
        assert_eq!(view.iter().count(), 0);
        let owned = <SetCookie as Field>::owned(&source).unwrap().unwrap();
        assert_eq!(owned.len(), 0);
        assert!(owned.is_empty());
        assert_eq!(owned.iter().count(), 0);
    }

    #[test]
    fn exactly_one_specializes_every_representation() {
        assert_eq!(
            FieldLines::single(&FieldName::Accept, b"text/html")
                .exactly_one()
                .expect("a single line is exactly one value")
                .as_bytes(),
            b"text/html"
        );

        let borrowed = [FieldValueRef::new(b"text/html"), FieldValueRef::new(b"application/json")];
        assert_eq!(
            FieldLines::from_borrowed(&FieldName::Accept, &borrowed)
                .expect("a non-empty slice makes a set")
                .exactly_one()
                .expect_err("two borrowed lines are not exactly one")
                .kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );

        let empty = FieldLines {
            name: &FieldName::Accept,
            repr: Repr::Borrowed(&[]),
        };
        assert_eq!(
            empty
                .exactly_one()
                .expect_err("internal empty borrowed representation reports missing")
                .kind(),
            DecodeErrorKind::MissingValue
        );
    }

    #[test]
    fn every_storage_representation_preserves_cardinality_order_and_owned_values() {
        assert!(FieldLines::from_slice(&FieldName::SetCookie, &[]).is_none());
        assert!(FieldLines::from_borrowed(&FieldName::SetCookie, &[]).is_none());

        let single = FieldLines::single(&FieldName::SetCookie, b"a=1");
        assert_eq!(single.name(), &FieldName::SetCookie);
        assert_eq!(single.len(), 1);
        assert_eq!(format!("{single:?}"), "FieldLines { name: \"set-cookie\", line_count: 1 }");
        let mut lines = single.repeated();
        assert_eq!(lines.size_hint(), (1, Some(1)));
        assert_eq!(format!("{lines:?}"), "FieldLinesIter { .. }");
        assert_eq!(lines.next().expect("single line"), "a=1");
        assert!(lines.next().is_none());
        assert_eq!(single.exactly_one_owned().expect("single owned value").as_bytes(), b"a=1");
        let mut owned_lines = single.repeated_owned().expect("valid raw line");
        assert_eq!(owned_lines.size_hint(), (1, Some(1)));
        assert_eq!(format!("{owned_lines:?}"), "FieldLinesOwnedIter { .. }");
        let (borrowed, owned) = owned_lines.next().expect("single owned line");
        assert_eq!(borrowed, "a=1");
        assert_eq!(owned, "a=1");

        let stored = [FieldValue::from_static("a=1"), FieldValue::from_static("b=2")];
        let slice = FieldLines::from_slice(&FieldName::SetCookie, &stored).expect("non-empty slice");
        assert_eq!(slice.len(), 2);
        assert_eq!(
            slice.repeated().map(FieldValueRef::as_bytes).collect::<Vec<_>>(),
            [b"a=1".as_slice(), b"b=2".as_slice()]
        );
        assert_eq!(
            slice
                .repeated_owned()
                .expect("stored values are valid")
                .map(|(borrowed, owned)| (borrowed.as_bytes(), owned))
                .collect::<Vec<_>>(),
            [
                (b"a=1".as_slice(), FieldValue::from_static("a=1")),
                (b"b=2".as_slice(), FieldValue::from_static("b=2")),
            ]
        );
        assert_eq!(
            slice.exactly_one_owned().expect_err("multiple owned values fail").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        let slice = FieldLines::from_slice(&FieldName::SetCookie, &stored).expect("non-empty slice");
        assert_eq!(
            slice.exactly_one().expect_err("multiple values fail").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );

        let refs = [FieldValueRef::new(b"a=1"), FieldValueRef::new(b"b=2")];
        let borrowed = FieldLines::from_borrowed(&FieldName::SetCookie, &refs).expect("non-empty refs");
        assert_eq!(borrowed.len(), 2);
        let mut borrowed_lines = borrowed.repeated();
        assert_eq!(borrowed_lines.size_hint(), (2, Some(2)));
        assert_eq!(borrowed_lines.by_ref().count(), 2);
        let mut borrowed_owned = borrowed.repeated_owned().expect("valid borrowed lines");
        assert_eq!(borrowed_owned.size_hint(), (2, Some(2)));
        assert_eq!(borrowed_owned.by_ref().count(), 2);
        assert_eq!(
            borrowed.exactly_one_owned().expect_err("multiple borrowed values fail").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        let one_ref = [FieldValueRef::new(b"a=1")];
        let one_borrowed = FieldLines::from_borrowed(&FieldName::SetCookie, &one_ref).expect("one ref");
        assert_eq!(one_borrowed.exactly_one_owned().expect("one borrowed owned value"), "a=1");

        let empty = FieldLines {
            name: &FieldName::SetCookie,
            repr: Repr::Slice(&[]),
        };
        assert_eq!(
            empty
                .exactly_one()
                .expect_err("internal empty representation reports missing")
                .kind(),
            DecodeErrorKind::MissingValue
        );
    }

    #[test]
    fn unvalidated_source_bytes_report_a_decode_error_instead_of_panicking() {
        let single = FieldLines::single(&FieldName::SetCookie, b"bad\nvalue");
        assert_eq!(
            single
                .exactly_one_owned()
                .expect_err("a source must not hand out invalid bytes")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let refs = [FieldValueRef::new(b"bad\nvalue")];
        let borrowed = FieldLines::from_borrowed(&FieldName::SetCookie, &refs).expect("one ref");
        assert_eq!(
            borrowed
                .exactly_one_owned()
                .expect_err("a source must not hand out invalid bytes")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        assert!(single.validate_custom_source_bounds().is_err());
        assert!(borrowed.validate_custom_source_bounds().is_err());
    }

    #[test]
    fn list_budget_and_counter_overflow_rejections_are_admission_errors() {
        for initial in [1_024, usize::MAX] {
            for bytes in [b"one,two".as_slice(), b"one"] {
                let mut item_count = initial;
                assert_eq!(
                    update_list_item_count(&FieldName::Vary, bytes, b',', true, true, &mut item_count)
                        .unwrap_err()
                        .kind(),
                    DecodeErrorKind::SourceLimitExceeded
                );
            }
        }
    }

    #[test]
    fn borrowed_and_owned_iteration_carry_sensitivity_from_stored_values() {
        let stored = [FieldValue::from_static("credential").with_sensitive(true)];
        let values = FieldLines::from_slice(&FieldName::Authorization, &stored).expect("non-empty slice");

        let borrowed = values.repeated().next().expect("one line");
        assert!(borrowed.is_sensitive());
        assert_eq!(format!("{borrowed:?}"), "FieldValueRef(Sensitive)");

        let (borrowed, owned) = values.repeated_owned().expect("stored value is valid").next().expect("one line");
        assert!(borrowed.is_sensitive());
        assert!(owned.is_sensitive());
        assert!(values.exactly_one_owned().expect("one value").is_sensitive());
    }

    #[cfg(feature = "http")]
    #[test]
    fn http_storage_representation_handles_empty_single_and_multiple_values() {
        let mut map = http::HeaderMap::new();
        assert!(FieldLines::from_http(&FieldName::SetCookie, map.get_all(http::header::SET_COOKIE)).is_none());
        let empty = FieldLines {
            name: &FieldName::SetCookie,
            repr: Repr::Http(map.get_all(http::header::SET_COOKIE)),
        };
        assert_eq!(
            empty
                .exactly_one_owned()
                .expect_err("internal empty http representation reports missing")
                .kind(),
            DecodeErrorKind::MissingValue
        );

        map.append(http::header::SET_COOKIE, http::HeaderValue::from_static("a=1"));
        let one = FieldLines::from_http(&FieldName::SetCookie, map.get_all(http::header::SET_COOKIE)).expect("one http value");
        assert_eq!(one.len(), 1);
        let mut lines = one.repeated();
        assert_eq!(lines.size_hint(), (1, Some(1)));
        assert_eq!(lines.next().expect("line"), "a=1");
        assert_eq!(one.exactly_one_owned().expect("one owned http value").as_bytes(), b"a=1");
        let mut owned_lines = one.repeated_owned().expect("http values are valid");
        assert_eq!(owned_lines.size_hint(), (1, Some(1)));
        assert_eq!(owned_lines.next().expect("owned line").1, "a=1");

        map.append(http::header::SET_COOKIE, http::HeaderValue::from_static("b=2"));
        let multiple = FieldLines::from_http(&FieldName::SetCookie, map.get_all(http::header::SET_COOKIE)).expect("multiple http values");
        assert_eq!(multiple.len(), 2);
        assert_eq!(multiple.repeated().count(), 2);
        assert_eq!(multiple.repeated_owned().expect("http values are valid").count(), 2);
        assert_eq!(
            multiple.exactly_one_owned().expect_err("multiple http values fail").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
    }

    #[cfg(feature = "http")]
    #[test]
    fn http_storage_carries_sensitivity_into_borrowed_and_owned_values() {
        let mut map = http::HeaderMap::new();
        let mut value = http::HeaderValue::from_static("Bearer credential");
        value.set_sensitive(true);
        map.append(http::header::AUTHORIZATION, value);

        let values = FieldLines::from_http(&FieldName::Authorization, map.get_all(http::header::AUTHORIZATION)).expect("one http value");

        let borrowed = values.repeated().next().expect("one line");
        assert!(borrowed.is_sensitive());
        let rendered = format!("{borrowed:?}");
        assert!(
            !rendered.contains("credential"),
            "a borrowed view must never render credential bytes: {rendered}"
        );

        let (borrowed, owned) = values.repeated_owned().expect("http values are valid").next().expect("one line");
        assert!(borrowed.is_sensitive());
        assert!(owned.is_sensitive());
        assert!(values.exactly_one_owned().expect("one value").is_sensitive());
    }
}
