// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::MAX_CUSTOM_LIST_ITEMS;
use crate::{DecodeError, DecodeErrorKind, FieldName};

pub(crate) fn update_list_item_count(
    name: &'static FieldName,
    bytes: &[u8],
    delimiter: u8,
    skip_empty: bool,
    backslash_escapes: bool,
    item_count: &mut usize,
) -> Result<(), DecodeError> {
    let limit = || DecodeError::new(name, DecodeErrorKind::SourceLimitExceeded);
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
                *item_count = item_count.checked_add(1).ok_or_else(limit)?;
                if *item_count > MAX_CUSTOM_LIST_ITEMS {
                    return Err(limit());
                }
            }
            start = position + 1;
        }
    }
    let item = crate::validate::trim_ows(&bytes[start..]);
    if !skip_empty || !item.is_empty() {
        *item_count = item_count.checked_add(1).ok_or_else(limit)?;
        if *item_count > MAX_CUSTOM_LIST_ITEMS {
            return Err(limit());
        }
    }
    Ok(())
}
