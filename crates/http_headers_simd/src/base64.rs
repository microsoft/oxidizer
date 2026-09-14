// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Scalar base64 alphabet classification used as the SIMD oracle.

/// Returns whether one byte is a standard base64 alphabet character.
///
/// Padding is deliberately excluded: callers know where `=` may appear and
/// check it positionally.
pub(super) fn is_base64_alphabet_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/'
}

/// Runs the whole check without any architecture-specific acceleration.
pub(super) fn all_base64_alphabet(bytes: &[u8]) -> bool {
    bytes.iter().copied().all(is_base64_alphabet_byte)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn alphabet_accepts_data_but_not_padding_or_url_safe_symbols() {
        assert!(all_base64_alphabet(
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        ));
        assert!(all_base64_alphabet(b""));
        for byte in [b'=', b'-', b'_', b' ', b'\n', 0x80] {
            assert!(!is_base64_alphabet_byte(byte), "{byte:#04x}");
            assert!(!all_base64_alphabet(&[b'A', byte]), "{byte:#04x}");
        }
    }
}
