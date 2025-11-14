// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{DataClass, Redactor};
use core::fmt::{Result, Write};
use xxhash_rust::xxh3::xxh3_64_with_secret;

const REDACTED_LEN: usize = 16;

/// A redactor that replaces the original string with the xxH3 hash of the string.
#[expect(
    non_camel_case_types,
    reason = "Just following the naming conventions of xxHash, silly as they are"
)]
#[derive(Clone, Debug)]
pub struct xxH3Redactor {
    secret: Box<[u8]>,
}

const MIN_SECRET_LENGTH: usize = 136;
const MAX_SECRET_LENGTH: usize = 256;

impl xxH3Redactor {
    /// Creates a new instance with a custom secret.
    ///
    /// The secret must be at least 136 bytes long and at most 256 bytes long, with
    /// a length of 192 being recommended.
    ///
    /// # Panics
    ///
    /// Panics if the secret is not within the specified length range.
    #[must_use]
    pub fn with_secret(secret: impl AsRef<[u8]>) -> Self {
        assert!(
            secret.as_ref().len() >= MIN_SECRET_LENGTH && secret.as_ref().len() <= MAX_SECRET_LENGTH,
            "Secret must be between {MIN_SECRET_LENGTH} and {MAX_SECRET_LENGTH} bytes long"
        );

        Self {
            secret: Box::from(secret.as_ref()),
        }
    }
}

impl Redactor for xxH3Redactor {
    fn redact(&self, _: &DataClass, value: &str, output: &mut dyn Write) -> Result {
        let hash = xxh3_64_with_secret(value.as_bytes(), &self.secret);
        let buffer = u64_to_hex_array(hash);

        // SAFETY: The buffer is guaranteed to be valid UTF-8 because it only contains hex digits.
        write!(output, "{}", unsafe { core::str::from_utf8_unchecked(&buffer) })
    }
}

#[inline]
fn u64_to_hex_array(mut value: u64) -> [u8; 16] {
    static HEX_LOWER_CHARS: &[u8; 16] = b"0123456789abcdef";

    let mut buffer = [0u8; REDACTED_LEN];
    for e in buffer.iter_mut().rev() {
        *e = HEX_LOWER_CHARS[(value & 0x0f) as usize];
        value >>= 4;
    }

    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_test_redactor() -> xxH3Redactor {
        let mut secret: Vec<u8> = vec![0; 192];
        for i in 0u8..192u8 {
            secret[i as usize] = i;
        }

        xxH3Redactor::with_secret(secret)
    }

    #[test]
    fn test_with_secret_creates_redactor_with_custom_secret() {
        let custom_secret = vec![42; 190];
        let redactor = xxH3Redactor::with_secret(custom_secret.clone());
        assert_eq!(redactor.secret.as_ref(), &custom_secret);
    }

    #[test]
    fn test_redact_produces_consistent_output() {
        let redactor = get_test_redactor();
        let data_class = DataClass::new("test_taxonomy", "test_class");
        let input = "sensitive_data";

        let mut output1 = String::new();
        let mut output2 = String::new();

        redactor.redact(&data_class, input, &mut output1).unwrap();
        redactor.redact(&data_class, input, &mut output2).unwrap();

        assert_eq!(output1, output2);
        assert_eq!(output1.len(), REDACTED_LEN);
    }

    #[test]
    fn test_redact_output_is_hex_string() {
        let redactor = get_test_redactor();
        let data_class = DataClass::new("test_taxonomy", "test_class");
        let input = "test_input";

        let mut output = String::new();
        redactor.redact(&data_class, input, &mut output).unwrap();

        assert_eq!(output.len(), REDACTED_LEN);
        assert!(output.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(output.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    }

    #[test]
    fn test_different_inputs_produce_different_outputs() {
        let redactor = get_test_redactor();
        let data_class = DataClass::new("test_taxonomy", "test_class");

        let mut output1 = String::new();
        let mut output2 = String::new();

        redactor.redact(&data_class, "input1", &mut output1).unwrap();
        redactor.redact(&data_class, "input2", &mut output2).unwrap();

        assert_ne!(output1, output2);
    }

    #[test]
    fn test_different_secrets_produce_different_outputs() {
        let redactor1 = get_test_redactor();
        let custom_secret = vec![0x95u8; 136];
        let redactor2 = xxH3Redactor::with_secret(&custom_secret);
        let data_class = DataClass::new("test_taxonomy", "test_class");
        let input = "same_input";

        let mut output1 = String::new();
        let mut output2 = String::new();

        redactor1.redact(&data_class, input, &mut output1).unwrap();
        redactor2.redact(&data_class, input, &mut output2).unwrap();

        assert_ne!(output1, output2);
    }

    #[test]
    fn test_empty_string_input() {
        let redactor = get_test_redactor();
        let data_class = DataClass::new("test_taxonomy", "test_class");

        let mut output = String::new();
        redactor.redact(&data_class, "", &mut output).unwrap();

        assert_eq!(output.len(), REDACTED_LEN);
        assert!(output.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_unicode_input() {
        let redactor = get_test_redactor();
        let data_class = DataClass::new("test_taxonomy", "test_class");
        let input = "こんにちは世界"; // "Hello World" in Japanese

        let mut output = String::new();
        redactor.redact(&data_class, input, &mut output).unwrap();

        assert_eq!(output.len(), REDACTED_LEN);
        assert!(output.chars().all(|c| c.is_ascii_hexdigit()));
    }

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

    #[test]
    fn test_clone_produces_identical_redactor() {
        // Create a custom secret that's at least 136 bytes (xxHash minimum)
        let custom_secret = vec![0x33u8; 136];
        let original = xxH3Redactor::with_secret(&custom_secret);
        let cloned = original.clone();

        assert_eq!(original.secret, cloned.secret);

        let data_class = DataClass::new("test_taxonomy", "test_class");
        let input = "test_input";

        let mut output1 = String::new();
        let mut output2 = String::new();

        original.redact(&data_class, input, &mut output1).unwrap();
        cloned.redact(&data_class, input, &mut output2).unwrap();

        assert_eq!(output1, output2);
    }

    #[test]
    fn test_custom_secret_edge_cases() {
        // Test with minimum viable secret (136 bytes for xxHash)
        let small_secret = vec![0x11u8; 136];
        let redactor = xxH3Redactor::with_secret(&small_secret);
        assert_eq!(redactor.secret.len(), 136);

        // Test with larger secret
        let large_secret = vec![0u8; 256];
        let redactor = xxH3Redactor::with_secret(&large_secret);
        assert_eq!(redactor.secret.len(), 256);
    }

    #[test]
    fn test_data_class_does_not_affect_output() {
        let redactor = get_test_redactor();
        let data_class1 = DataClass::new("test_taxonomy", "class1");
        let data_class2 = DataClass::new("test_taxonomy", "class2");
        let input = "test_input";

        let mut output1 = String::new();
        let mut output2 = String::new();

        redactor.redact(&data_class1, input, &mut output1).unwrap();
        redactor.redact(&data_class2, input, &mut output2).unwrap();

        // The data_class parameter is ignored in the redaction process
        assert_eq!(output1, output2);
    }
}
