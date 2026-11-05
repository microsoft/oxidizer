// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{base64, uri};

#[inline]
pub(crate) fn all_base64_alphabet(bytes: &[u8]) -> bool {
    base64::all_base64_alphabet(bytes)
}

#[inline]
pub(super) fn simple_uri_tail(bytes: &[u8]) -> bool {
    uri::simple_uri_tail(bytes, 0)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[test]
    fn gamma_nov6_portable_base64_preserves_both_answers_and_every_input_byte() {
        let valid = [b'A'; 64];
        assert!(super::all_base64_alphabet(&valid));
        assert!(super::all_base64_alphabet(b"+/019AZaz"));
        for lane in 0..valid.len() {
            let mut invalid = valid;
            invalid[lane] = b'!';
            assert!(!super::all_base64_alphabet(&invalid), "{lane}");
        }
    }

    #[test]
    fn gamma_nov6_portable_uri_tail_preserves_both_answers_and_every_input_byte() {
        let mut valid = [b'a'; 64];
        valid[0] = b'/';
        assert!(super::simple_uri_tail(&valid));
        assert!(super::simple_uri_tail(b"/path?query=value#fragment"));
        for lane in 0..valid.len() {
            let mut invalid = valid;
            invalid[lane] = b' ';
            assert!(!super::simple_uri_tail(&invalid), "{lane}");
        }
    }
}
