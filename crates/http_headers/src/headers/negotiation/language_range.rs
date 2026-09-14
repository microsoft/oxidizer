// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use super::accept_language::valid_language_range;
use super::negotiation_token::NegotiationToken;
use super::shared::invalid;
use crate::{DecodeError, DecodeErrorKind, FieldName};

/// A validated basic language range, or the standalone wildcard `*`.
///
/// This models the basic HTTP range grammar, not a `BCP 47` registry or extended
/// language ranges. Equality is ASCII case-insensitive. No case normalization,
/// locale fallback or matching policy is applied.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LanguageRange<'a> {
    token: NegotiationToken<'a>,
    primary_len: usize,
}

impl<'a> LanguageRange<'a> {
    /// Validates a wildcard or a primary subtag followed by optional subtags.
    ///
    /// # Errors
    ///
    /// The primary must contain one to eight ASCII letters; subsequent
    /// subtags must contain one to eight ASCII letters or digits. Internal
    /// wildcards and empty subtags are rejected.
    pub fn parse(value: &'a str) -> Result<Self, DecodeError> {
        if !valid_language_range(value.as_bytes()) {
            return Err(invalid(&FieldName::AcceptLanguage, DecodeErrorKind::InvalidToken));
        }
        Ok(Self::from_validated(value.as_bytes()))
    }

    pub(super) fn from_validated(bytes: &'a [u8]) -> Self {
        let primary_len = if bytes == b"*" {
            0
        } else {
            bytes.iter().position(|byte| *byte == b'-').unwrap_or(bytes.len())
        };
        Self {
            token: NegotiationToken::from_validated(bytes),
            primary_len,
        }
    }

    /// Returns the complete case-preserved range.
    #[must_use]
    pub const fn as_str(self) -> &'a str {
        self.token.as_str()
    }

    /// Whether the range is exactly `*`.
    #[must_use]
    pub const fn is_wildcard(self) -> bool {
        self.primary_len == 0
    }

    /// Returns the primary subtag, or `None` for a wildcard.
    #[must_use]
    pub fn primary(self) -> Option<NegotiationToken<'a>> {
        if self.is_wildcard() {
            None
        } else {
            Some(NegotiationToken::from_validated_str(&self.as_str()[..self.primary_len]))
        }
    }

    /// Iterates subsequent subtags without allocating, excluding the primary.
    ///
    /// A wildcard and a primary-only range both yield an empty iterator.
    pub fn subtags(self) -> impl Iterator<Item = NegotiationToken<'a>> {
        let rest = (!self.is_wildcard() && self.primary_len < self.as_str().len()).then(|| &self.as_str()[self.primary_len + 1..]);
        rest.into_iter()
            .flat_map(|rest| rest.split('-'))
            .map(NegotiationToken::from_validated_str)
    }

    /// Compares a complete range without ASCII case distinctions.
    #[must_use]
    pub fn eq_ignore_ascii_case(self, other: &str) -> bool {
        self.token.eq_ignore_ascii_case(other)
    }
}

impl fmt::Display for LanguageRange<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.token.fmt(f)
    }
}
