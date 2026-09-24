// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::{fmt, str};

use super::shared::invalid;
use crate::{DecodeError, DecodeErrorKind, FieldName, validate};

/// A validated HTTP token with case-insensitive semantic equality.
///
/// Original spelling is preserved. This type is for negotiation components,
/// not case-sensitive HTTP methods or arbitrary parameter values.
#[derive(Clone, Copy, Debug)]
pub struct NegotiationToken<'a>(&'a str);

impl<'a> NegotiationToken<'a> {
    /// Validates an HTTP token.
    ///
    /// # Errors
    ///
    /// Returns an error if the string is empty or contains a non-token byte.
    pub fn new(value: &'a str) -> Result<Self, DecodeError> {
        if !validate::token(value.as_bytes()) {
            return Err(invalid(&FieldName::Accept, DecodeErrorKind::InvalidToken));
        }
        Ok(Self(value))
    }

    pub(super) fn from_validated(bytes: &'a [u8]) -> Self {
        Self(str::from_utf8(bytes).expect("validated HTTP tokens contain only ASCII bytes"))
    }

    pub(super) const fn from_validated_str(value: &'a str) -> Self {
        Self(value)
    }

    /// Returns the original case-preserved spelling.
    #[must_use]
    pub const fn as_str(self) -> &'a str {
        self.0
    }

    /// Compares a spelling without ASCII case distinctions.
    #[must_use]
    pub fn eq_ignore_ascii_case(self, other: &str) -> bool {
        self.0.eq_ignore_ascii_case(other)
    }
}

impl PartialEq for NegotiationToken<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(other.0)
    }
}

impl Eq for NegotiationToken<'_> {}

impl PartialOrd for NegotiationToken<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NegotiationToken<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .bytes()
            .map(|byte| byte.to_ascii_lowercase())
            .cmp(other.0.bytes().map(|byte| byte.to_ascii_lowercase()))
    }
}

impl Hash for NegotiationToken<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.len().hash(state);
        for byte in self.0.bytes() {
            byte.to_ascii_lowercase().hash(state);
        }
    }
}

impl fmt::Display for NegotiationToken<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
