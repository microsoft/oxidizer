// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A redactor based on `RapidHash`.

use crate::{DataClass, Redactor};
use core::fmt::Write;
use rapidhash::v3::{RapidSecrets, rapidhash_v3_seeded};

/// The length of the redacted output in hex digits.
pub const REDACTED_LEN: usize = 16;

/// A redactor that replaces the original string with the `RapidHash` hash of the string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RapidHashRedactor {
    secrets: RapidSecrets,
}

impl RapidHashRedactor {
    /// Creates a new instance with a custom secret.
    #[must_use]
    pub fn with_secrets(secrets: RapidSecrets) -> Self {
        Self { secrets }
    }
}

impl Redactor for RapidHashRedactor {
    fn redact(&self, _: &DataClass, value: &str, output: &mut dyn Write) -> core::fmt::Result {
        let hash = rapidhash_v3_seeded(value.as_bytes(), &self.secrets);
        let buffer = crate::redactors::u64_to_hex_array::<REDACTED_LEN>(hash);

        // SAFETY: The buffer is guaranteed to be valid UTF-8 because it only contains hex digits.
        write!(output, "{}", unsafe { core::str::from_utf8_unchecked(&buffer) })
    }
}
