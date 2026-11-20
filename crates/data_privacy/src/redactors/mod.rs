// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

pub mod simple_redactor;

#[cfg(feature = "xxh3")]
pub mod xxh3_redactor;

#[cfg(feature = "rapidhash")]
pub mod rapidhash_redactor;

#[cfg(any(feature = "xxh3", feature = "rapidhash"))]
#[inline]
pub(crate) fn u64_to_hex_array<const N: usize>(mut value: u64) -> [u8; N] {
    const HEX_LOWER_CHARS: &[u8; 16] = b"0123456789abcdef";

    // #[gamma::skip(literal.int_increment, tag = "equivalent", reason = "The loop overwrites every buffer byte before returning it, including when N is zero.")]
    let mut buffer = [0u8; N];
    for e in buffer.iter_mut().rev() {
        *e = HEX_LOWER_CHARS[(value & 0x0f) as usize];
        value >>= 4;
    }

    buffer
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
// Hash redactors cannot select the helper's exact 0 and u64::MAX inputs.
// Keep its byte-array assertions here rather than substituting hash-output checks.
mod tests {
    #[cfg(any(feature = "xxh3", feature = "rapidhash"))]
    #[test]
    fn test_u64_to_hex_array() {
        use super::u64_to_hex_array;

        let result = u64_to_hex_array(0x1234_5678_9abc_def0);
        let expected = b"123456789abcdef0";
        assert_eq!(result, *expected);

        let result = u64_to_hex_array(0);
        let expected = b"0000000000000000";
        assert_eq!(result, *expected);

        let result = u64_to_hex_array(u64::MAX);
        let expected = b"ffffffffffffffff";
        assert_eq!(result, *expected);
    }
}
