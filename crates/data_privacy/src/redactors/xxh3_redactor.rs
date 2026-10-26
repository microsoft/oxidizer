// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A keyed cryptographic redactor with a legacy `xxh3` API.

use core::fmt::{self, Write};

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::{DataClass, Redactor};

/// The length of the redacted output in hex digits.
pub const REDACTED_LEN: usize = 64;

/// A redactor that replaces the original string with its HMAC-SHA-256 value.
///
/// The type and feature names remain unchanged for source compatibility.
/// This redactor no longer uses xxHash, which is not a cryptographic hash.
/// Outputs now contain 64 lowercase hex digits instead of 16.
/// Previously stored xxHash outputs cannot be correlated with the new outputs.
///
/// # Security
///
/// Generate the secret with a cryptographically secure random number generator.
/// Keep it private. Anyone with the secret can test guesses against redacted values.
/// Use the same secret only where correlation between values is intended.
#[expect(
    non_camel_case_types,
    reason = "Just following the naming conventions of xxHash, silly as they are"
)]
#[derive(Clone, Eq, PartialEq)]
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
    fn redacts(&self, _data_class: &DataClass) -> bool {
        true
    }

    fn redact(&self, _: &DataClass, value: &str, output: &mut dyn Write) -> core::fmt::Result {
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(&self.secret) else {
            unreachable!("HMAC accepts keys of any length");
        };
        mac.update(value.as_bytes());

        for byte in mac.finalize().into_bytes() {
            write!(output, "{byte:02x}")?;
        }

        Ok(())
    }
}

impl fmt::Debug for xxH3Redactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("xxH3Redactor").finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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
