// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod redactor;

pub mod simple_redactor;

#[cfg(feature = "xxh3")]
pub mod xxh3_redactor;

#[cfg(feature = "rapidhash")]
pub mod rapidhash_redactor;

pub use redactor::Redactor;

#[cfg(any(feature = "xxh3", feature = "rapidhash"))]
#[inline]
pub fn u64_to_hex_array<const N: usize>(mut value: u64) -> [u8; N] {
    const HEX_LOWER_CHARS: &[u8; 16] = b"0123456789abcdef";

    let mut buffer = [0u8; N];
    for e in buffer.iter_mut().rev() {
        *e = HEX_LOWER_CHARS[(value & 0x0f) as usize];
        value >>= 4;
    }

    buffer
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use data_privacy_macros::taxonomy;

    use super::*;

    #[cfg(any(feature = "xxh3", feature = "rapidhash"))]
    #[test]
    fn test_u64_to_hex_array() {
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

    struct TestRedactor;

    impl Redactor for TestRedactor {
        fn redact(&self, _data_class: &crate::DataClass, value: &str, output: &mut dyn Write) -> std::fmt::Result {
            write!(output, "{value}tomato")
        }
    }

    #[taxonomy(test)]
    enum TestTaxonomy {
        Sensitive,
    }

    #[test]
    fn test_exact_len_default_behavior() {
        let redactor = TestRedactor;
        let mut output_buffer = String::new();
        _ = redactor.redact(&TestTaxonomy::Sensitive.data_class(), "test_value", &mut output_buffer);

        assert_eq!(output_buffer, "test_valuetomato");
    }
}
