// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use super::negotiation_token::NegotiationToken;
use super::shared::invalid;
use crate::{DecodeError, DecodeErrorKind, FieldName, validate};

/// The semantic classification of a content coding.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ContentCodingKind {
    /// The standalone wildcard `*`.
    Wildcard,
    /// The representation without content encoding, `identity`.
    Identity,
    /// The `gzip` coding.
    Gzip,
    /// The `compress` coding.
    Compress,
    /// The `deflate` coding.
    Deflate,
    /// The Brotli `br` coding.
    Br,
    /// The Zstandard `zstd` coding.
    Zstd,
    /// The dictionary-compressed Brotli `dcb` coding.
    Dcb,
    /// The dictionary-compressed Zstandard `dcz` coding.
    Dcz,
    /// Another validated coding token, including embedded stars.
    Extension,
}

/// A validated content coding with case-insensitive equality.
///
/// Wildcard and identity are distinct; no implicit coding entries or codec
/// selection policies are applied.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ContentCoding<'a> {
    token: NegotiationToken<'a>,
    kind: ContentCodingKind,
}

impl<'a> ContentCoding<'a> {
    /// Validates a coding token and classifies known spellings.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty or malformed HTTP token.
    pub fn parse(value: &'a str) -> Result<Self, DecodeError> {
        if !validate::token(value.as_bytes()) {
            return Err(invalid(&FieldName::AcceptEncoding, DecodeErrorKind::InvalidToken));
        }
        Ok(Self::new(NegotiationToken::from_validated(value.as_bytes())))
    }

    /// Classifies a validated coding token without changing its spelling.
    #[must_use]
    pub fn new(token: NegotiationToken<'a>) -> Self {
        let kind = if token.as_str() == "*" {
            ContentCodingKind::Wildcard
        } else if token.eq_ignore_ascii_case("identity") {
            ContentCodingKind::Identity
        } else if token.eq_ignore_ascii_case("gzip") {
            ContentCodingKind::Gzip
        } else if token.eq_ignore_ascii_case("compress") {
            ContentCodingKind::Compress
        } else if token.eq_ignore_ascii_case("deflate") {
            ContentCodingKind::Deflate
        } else if token.eq_ignore_ascii_case("br") {
            ContentCodingKind::Br
        } else if token.eq_ignore_ascii_case("zstd") {
            ContentCodingKind::Zstd
        } else if token.eq_ignore_ascii_case("dcb") {
            ContentCodingKind::Dcb
        } else if token.eq_ignore_ascii_case("dcz") {
            ContentCodingKind::Dcz
        } else {
            ContentCodingKind::Extension
        };
        Self { token, kind }
    }

    pub(super) fn from_validated(bytes: &'a [u8]) -> Self {
        Self::new(NegotiationToken::from_validated(bytes))
    }

    /// Returns the retained coding classification.
    #[must_use]
    pub const fn kind(self) -> ContentCodingKind {
        self.kind
    }

    /// Returns the validated token with its original spelling.
    #[must_use]
    pub const fn token(self) -> NegotiationToken<'a> {
        self.token
    }

    /// Returns the original coding spelling.
    #[must_use]
    pub const fn as_str(self) -> &'a str {
        self.token.as_str()
    }
}

impl fmt::Display for ContentCoding<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.token.fmt(f)
    }
}
