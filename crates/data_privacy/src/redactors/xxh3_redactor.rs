// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A redactor based on `xxh3`.

use core::fmt::Write;

use xxhash_rust::xxh3::xxh3_64_with_secret;

use crate::{DataClass, Redactor};

/// The length of the redacted output in hex digits.
pub const REDACTED_LEN: usize = 16;

/// A redactor that replaces the original string with the xxH3 hash of the string.
#[expect(
    non_camel_case_types,
    reason = "Just following the naming conventions of xxHash, silly as they are"
)]
#[derive(Clone, Debug, Eq, PartialEq)]
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
    fn redact(&self, _: &DataClass, value: &str, output: &mut dyn Write) -> core::fmt::Result {
        let hash = xxh3_64_with_secret(value.as_bytes(), &self.secret);
        let buffer = crate::redactors::u64_to_hex_array::<REDACTED_LEN>(hash);

        // SAFETY: The buffer is guaranteed to be valid UTF-8 because it only contains hex digits.
        write!(output, "{}", unsafe { core::str::from_utf8_unchecked(&buffer) })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_with_secret_creates_redactor_with_custom_secret() {
        let custom_secret = vec![42; 190];
        let redactor = crate::redactors::xxh3_redactor::xxH3Redactor::with_secret(custom_secret.clone());
        assert_eq!(redactor.secret.as_ref(), &custom_secret);
    }

    #[test]
    fn test_custom_secret_edge_cases() {
        // Test with minimum viable secret (136 bytes for xxHash)
        let small_secret = vec![0x11u8; 136];
        let redactor = crate::redactors::xxh3_redactor::xxH3Redactor::with_secret(&small_secret);
        assert_eq!(redactor.secret.len(), 136);

        // Test with larger secret
        let large_secret = vec![0u8; 256];
        let redactor = crate::redactors::xxh3_redactor::xxH3Redactor::with_secret(&large_secret);
        assert_eq!(redactor.secret.len(), 256);
    }
}
