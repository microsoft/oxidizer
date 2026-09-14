// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Definitions involved in reading fields from field containers.
//!
//! A container implements [`FieldSource`] to expose the field lines stored
//! under a field name, and every typed decode in this crate starts there.
//! [`FieldLines`] is what a source hands back: the one-or-more raw field
//! lines for a single name, borrowed from whatever storage the container
//! uses. [`FieldLinesIter`] walks those lines, and [`DelimitedItems`] walks the
//! comma- or semicolon-separated items within them.
//!
//! The `http` cargo feature implements [`FieldSource`] for
//! `http::HeaderMap`. Implement it yourself only when integrating a different
//! container; reading a known header needs nothing from this module. Owned
//! decodes from custom sources accept at most [`MAX_CUSTOM_FIELD_BYTES`] total
//! bytes and [`MAX_CUSTOM_FIELD_LINES`] lines for one name. Delimited parsing
//! accepts at most [`MAX_CUSTOM_LIST_ITEMS`] items per decode. The validated
//! `http::HeaderMap` adapter is exempt from these custom-source budgets.

mod delimited_items;
mod field_lines;
mod field_source;

#[doc(inline)]
pub use delimited_items::DelimitedItems;
#[doc(inline)]
pub use field_lines::{FieldLines, FieldLinesIter, MAX_CUSTOM_FIELD_BYTES, MAX_CUSTOM_FIELD_LINES, MAX_CUSTOM_LIST_ITEMS};
#[doc(inline)]
pub use field_source::FieldSource;

use crate::{DecodeError, DecodeErrorKind, FieldName};

pub(crate) fn update_list_item_count(
    name: &'static FieldName,
    bytes: &[u8],
    delimiter: u8,
    skip_empty: bool,
    backslash_escapes: bool,
    item_count: &mut usize,
) -> Result<(), DecodeError> {
    let invalid = || DecodeError::new(name, DecodeErrorKind::InvalidSyntax);
    let mut start = 0_usize;
    let mut quoted = false;
    let mut escaped = false;
    for (position, byte) in bytes.iter().copied().enumerate() {
        if backslash_escapes && escaped {
            escaped = false;
        } else if backslash_escapes && quoted && byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            quoted = !quoted;
        } else if !quoted && byte == delimiter {
            let item = crate::validate::trim_ows(&bytes[start..position]);
            if !skip_empty || !item.is_empty() {
                *item_count = item_count.checked_add(1).ok_or_else(invalid)?;
                if *item_count > MAX_CUSTOM_LIST_ITEMS {
                    return Err(invalid());
                }
            }
            start = position + 1;
        }
    }
    let item = crate::validate::trim_ows(&bytes[start..]);
    if !skip_empty || !item.is_empty() {
        *item_count = item_count.checked_add(1).ok_or_else(invalid)?;
        if *item_count > MAX_CUSTOM_LIST_ITEMS {
            return Err(invalid());
        }
    }
    Ok(())
}
