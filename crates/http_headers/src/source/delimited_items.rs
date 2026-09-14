// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Delimiter-aware iteration over the items within field lines.

use std::fmt;

use crate::source::{FieldLines, FieldLinesIter, MAX_CUSTOM_LIST_ITEMS};
use crate::{DecodeError, DecodeErrorKind, FieldName};

/// A borrowing iterator over delimited items across multiple field lines.
///
/// Each item is yielded as the raw bytes between delimiters, with optional
/// whitespace already trimmed from both ends. Delimiters inside quoted strings
/// are treated as part of the item, and backslash escapes inside quoted
/// strings are honored within each physical field line. A quoted string or
/// escape cannot span field lines because every yielded item borrows one
/// contiguous line. Items are bytes rather than `&str` because a field value
/// may contain obs-text, which need not be UTF-8.
///
/// Each item is returned as a `Result`; an unterminated quoted string produces
/// [`DecodeErrorKind::UnterminatedQuote`] with the affected field-line index.
/// Custom sources are also rejected before yielding an item when their byte or
/// line budget is exceeded, and after [`MAX_CUSTOM_LIST_ITEMS`] parsed items.
///
/// # Examples
///
/// ```rust
/// use http_headers::FieldName;
/// use http_headers::source::{DelimitedItems, FieldLines};
///
/// let lines = FieldLines::single(&FieldName::Vary, b" accept , origin ");
/// let mut items: DelimitedItems<'_> = lines.comma_items();
/// assert_eq!(items.next().transpose()?, Some(b"accept".as_slice()));
/// assert_eq!(items.next().transpose()?, Some(b"origin".as_slice()));
/// assert!(items.next().is_none());
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct DelimitedItems<'a> {
    name: &'static FieldName,
    values: FieldLinesIter<'a>,
    delimiter: u8,
    skip_empty: bool,
    current: Option<&'a [u8]>,
    start: usize,
    position: usize,
    value_index: usize,
    item_count: usize,
    pending_error: Option<DecodeError>,
    limited: bool,
    finished: bool,
}

impl fmt::Debug for DelimitedItems<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DelimitedItems")
            .field("name", &self.name)
            .field("delimiter", &self.delimiter)
            .field("value_index", &self.value_index)
            .finish_non_exhaustive()
    }
}

impl<'a> DelimitedItems<'a> {
    pub(super) fn new(values: &FieldLines<'a>, delimiter: u8) -> Self {
        Self {
            name: values.name(),
            values: values.repeated(),
            delimiter,
            skip_empty: delimiter == b',',
            current: None,
            start: 0,
            position: 0,
            value_index: 0,
            item_count: 0,
            pending_error: values.validate_custom_source().err(),
            limited: values.has_custom_source_limits(),
            finished: false,
        }
    }

    fn item(&mut self, item: &'a [u8]) -> Result<&'a [u8], DecodeError> {
        if self.limited && self.item_count == MAX_CUSTOM_LIST_ITEMS {
            self.finished = true;
            return Err(DecodeError::new(self.name, DecodeErrorKind::InvalidSyntax));
        }
        if self.limited {
            self.item_count += 1;
        }
        Ok(item)
    }
}

fn trim_ows(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|byte| !matches!(byte, b' ' | b'\t')).unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

#[inline]
fn find_delimiter_or_quote(bytes: &[u8], delimiter: u8) -> Option<usize> {
    http_headers_simd::find_either(bytes, delimiter, b'"')
}

impl<'a> Iterator for DelimitedItems<'a> {
    type Item = Result<&'a [u8], DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        if let Some(error) = self.pending_error.take() {
            self.finished = true;
            return Some(Err(error));
        }

        loop {
            let bytes = if let Some(bytes) = self.current {
                bytes
            } else {
                let value = self.values.next()?;
                let bytes = value.as_bytes();
                self.current = Some(bytes);
                self.start = 0;
                self.position = 0;
                bytes
            };

            let mut quoted = false;
            let mut escaped = false;
            while self.position < bytes.len() {
                if !quoted && let Some(skip) = find_delimiter_or_quote(&bytes[self.position..], self.delimiter) {
                    self.position += skip;
                } else if !quoted {
                    self.position = bytes.len();
                    break;
                }

                let byte = bytes[self.position];
                if escaped {
                    escaped = false;
                } else if quoted && byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quoted = !quoted;
                } else if !quoted && byte == self.delimiter {
                    let item = trim_ows(&bytes[self.start..self.position]);
                    self.position += 1;
                    self.start = self.position;
                    if self.skip_empty && item.is_empty() {
                        continue;
                    }
                    return Some(self.item(item));
                }
                self.position += 1;
            }

            let index = self.value_index;
            self.value_index += 1;
            let item = trim_ows(&bytes[self.start..]);
            self.current = None;
            if quoted || escaped {
                return Some(Err(DecodeError::new(self.name, DecodeErrorKind::UnterminatedQuote).at_value(index)));
            }

            if !self.skip_empty || !item.is_empty() {
                return Some(self.item(item));
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::source::FieldLines;
    use crate::{DecodeErrorKind, FieldName, FieldValue};

    #[test]
    fn delimited_iteration_handles_ows_empty_items_quotes_escapes_and_errors() {
        let stored = [
            FieldValue::from_static(" , alpha, \"bravo,charlie\" ,, "),
            FieldValue::from_static("delta"),
        ];
        let values = FieldLines::from_slice(&FieldName::Vary, &stored).expect("values");
        let mut items = values.comma_items();
        assert_eq!(
            format!("{items:?}"),
            "DelimitedItems { name: \"vary\", delimiter: 44, value_index: 0, .. }"
        );
        assert_eq!(
            items.by_ref().map(|item| item.expect("valid item")).collect::<Vec<_>>(),
            [b"alpha".as_slice(), b"\"bravo,charlie\"".as_slice(), b"delta".as_slice(),]
        );
        assert!(items.next().is_none());

        let semicolons = FieldLines::single(&FieldName::ContentType, b" alpha ; ; \"b;\\\"c\" ; ");
        assert_eq!(
            semicolons
                .semicolon_items()
                .map(|item| item.expect("valid item"))
                .collect::<Vec<_>>(),
            [b"alpha".as_slice(), b"".as_slice(), b"\"b;\\\"c\"".as_slice(), b"".as_slice(),]
        );

        let unterminated = FieldLines::single(&FieldName::Vary, b"alpha,\"bravo");
        let error = unterminated
            .comma_items()
            .nth(1)
            .expect("error item")
            .expect_err("unterminated quote fails");
        assert_eq!(error.kind(), DecodeErrorKind::UnterminatedQuote);
        assert_eq!(error.value_index(), Some(0));

        let escaped = FieldLines::single(&FieldName::Vary, b"\"alpha\\");
        assert_eq!(
            escaped
                .comma_items()
                .next()
                .expect("error item")
                .expect_err("trailing escape fails")
                .kind(),
            DecodeErrorKind::UnterminatedQuote
        );

        let split_quote = [FieldValue::from_static("\"alpha"), FieldValue::from_static("bravo\"")];
        let values = FieldLines::from_slice(&FieldName::Vary, &split_quote).expect("values");
        let error = values
            .comma_items()
            .next()
            .expect("error item")
            .expect_err("quoted strings cannot span physical lines");
        assert_eq!(error.kind(), DecodeErrorKind::UnterminatedQuote);
        assert_eq!(error.value_index(), Some(0));

        let obs_text = [FieldValue::from_bytes(b"\xff").expect("obs-text is a valid field value")];
        let values = FieldLines::from_slice(&FieldName::Vary, &obs_text).expect("values");
        assert_eq!(
            values.comma_items().next().expect("one item").expect("obs-text item is yielded"),
            b"\xff".as_slice()
        );
    }
}
