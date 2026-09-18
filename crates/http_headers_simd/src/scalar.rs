// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Portable scalar validators and reference implementations.

// RFC 9110 `tchar`, indexed by the upper two bits of each byte.
const TOKEN_BITMAP: [u64; 4] = [0x03ff_6cfa_0000_0000, 0x57ff_ffff_c7ff_fffe, 0, 0];
// RFC 9110 `token68` data characters, indexed by the upper two bits.
const TOKEN68_BITMAP: [u64; 4] = [0x03ff_e800_0000_0000, 0x47ff_fffe_87ff_fffe, 0, 0];

pub(super) fn is_token(bytes: &[u8]) -> bool {
    !bytes.is_empty() && all_token_bytes(bytes)
}

pub(super) fn all_token_bytes(bytes: &[u8]) -> bool {
    bytes.iter().copied().all(is_token_byte)
}

pub(super) fn is_token68(bytes: &[u8]) -> bool {
    token68_tail(bytes, false)
}

pub(super) fn token68_tail(bytes: &[u8], mut has_data: bool) -> bool {
    for (index, &byte) in bytes.iter().enumerate() {
        if !is_token68_data_byte(byte) {
            return has_data && byte == b'=' && bytes[index + 1..].iter().all(|byte| *byte == b'=');
        }
        has_data = true;
    }
    has_data
}

fn is_token68_data_byte(byte: u8) -> bool {
    let word = TOKEN68_BITMAP[usize::from(byte >> 6)];
    word & (1_u64 << (byte & 63)) != 0
}

fn is_token_byte(byte: u8) -> bool {
    let word = TOKEN_BITMAP[usize::from(byte >> 6)];
    word & (1_u64 << (byte & 63)) != 0
}

pub(super) fn is_field_value(bytes: &[u8]) -> bool {
    bytes.iter().copied().all(|byte| byte == b'\t' || byte >= b' ' && byte != 0x7f)
}

pub(super) fn eq_ignore_ascii_case(left: &[u8], right: &[u8]) -> bool {
    left.eq_ignore_ascii_case(right)
}

pub(super) fn find_interesting(bytes: &[u8]) -> Option<usize> {
    bytes
        .iter()
        .position(|byte| matches!(byte, b',' | b';' | b'"' | b'\\' | b' ' | b'\t'))
}

pub(super) fn find_either(bytes: &[u8], first: u8, second: u8) -> Option<usize> {
    bytes.iter().position(|byte| *byte == first || *byte == second)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn token68_requires_data_before_contiguous_padding() {
        for valid in [&b"a"[..], b"a=", b"a==", b"AZ09-._~+/=="] {
            assert!(is_token68(valid), "{valid:?}");
        }
        for invalid in [&b""[..], b"=", b"==", b"a=a", b"a= ="] {
            assert!(!is_token68(invalid), "{invalid:?}");
        }
    }

    #[test]
    fn scalar_helpers_handle_offsets_and_non_ascii_bytes() {
        assert!(all_token_bytes(b"AZaz09!#$%&'*+-.^_`|~"));
        assert!(!all_token_bytes(b"token()"));
        assert!(is_field_value(&[b'\t', b' ', b'~', 0x80, 0xff]));
        assert!(!is_field_value(&[0x7f]));
        assert!(eq_ignore_ascii_case(b"HeAdEr", b"hEaDeR"));
        assert!(!eq_ignore_ascii_case(&[0x80], &[0x81]));
        assert_eq!(find_interesting(b"token;parameter"), Some(5));
        assert_eq!(find_interesting(b"token"), None);
    }
}
