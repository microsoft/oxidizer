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
        // #[gamma::skip(logical.and_remove_left, tag = "equivalent", reason = "When backslash escapes are disabled, the escaped state is never consumed because its only reader is separately gated by backslash_escapes")]
        let allow_quoted_escape = backslash_escapes && quoted;
        if
        // #[gamma::skip(logical.and_remove_left, tag = "equivalent", reason = "escaped is set only when backslash_escapes is true, so escaped already implies the removed flag")]
        backslash_escapes && escaped {
            // #[gamma::skip(assign_value.default, tag = "equivalent", reason = "false is exactly bool::default(); the assignment still executes")]
            escaped = false;
        } else if allow_quoted_escape && byte == b'\\' {
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

#[cfg(test)]
mod tests {
    use super::update_list_item_count;
    use crate::{DecodeErrorKind, FieldName};

    #[test]
    fn gamma_round2_list_budget_counts_escaped_quotes_and_empty_members_exactly() {
        for (wire, escaped, skip_empty, expected) in [
            (b"\"a\\\",b\",c".as_slice(), true, true, 2),
            (b"\"a\\\",b\",c", false, true, 2),
            (b"\"a\\\",b,c", true, true, 1),
            (b"\"a\\\",b,c", false, true, 3),
            (b"a\\\",b,c", true, true, 1),
            (b",a,,", true, true, 1),
            (b",a,,", true, false, 4),
            (b"\"a\\\\\",b,c", true, true, 3),
            (b"\"ax\",b,c", true, true, 3),
        ] {
            let mut count = 0;
            update_list_item_count(&FieldName::Vary, wire, b',', skip_empty, escaped, &mut count).unwrap();
            assert_eq!(count, expected, "{wire:?}, escapes={escaped}, skip={skip_empty}");
        }
        let mut count = 1023;
        update_list_item_count(&FieldName::Vary, b"a,", b',', true, true, &mut count).unwrap();
        assert_eq!(count, 1024);
        assert_eq!(
            update_list_item_count(&FieldName::Vary, b"b,c", b',', true, true, &mut count)
                .unwrap_err()
                .kind(),
            DecodeErrorKind::SourceLimitExceeded
        );
        assert_eq!(count, 1025);
    }
}
