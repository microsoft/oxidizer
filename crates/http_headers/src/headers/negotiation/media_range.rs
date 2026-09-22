// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{fmt, str};

use super::accept::validate_media_range;
use super::negotiation_token::NegotiationToken;
use super::shared::invalid_syntax;
use crate::{DecodeError, FieldName};

/// The wildcard semantics of a validated media range.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MediaRangeKind {
    /// `*/*`, accepting any media type.
    Any,
    /// A named type and `*` subtype, such as `text/*`.
    TypeWildcard,
    /// A type and subtype without a standalone wildcard component.
    Exact,
}

/// A validated media range with case-insensitive component equality.
///
/// Only a complete `*` component is a wildcard. Embedded stars remain ordinary
/// token characters, and the original spelling is retained.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MediaRange<'a> {
    type_: NegotiationToken<'a>,
    subtype: NegotiationToken<'a>,
    kind: MediaRangeKind,
}

impl<'a> MediaRange<'a> {
    /// Validates a media range without parameters or surrounding whitespace.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid components or a wildcard type with a
    /// non-wildcard subtype.
    pub fn parse(value: &'a str) -> Result<Self, DecodeError> {
        validate_media_range(value.as_bytes())?;
        Ok(Self::from_validated(value.as_bytes()))
    }

    /// Constructs a media range from validated components.
    ///
    /// # Errors
    ///
    /// A wildcard type requires a wildcard subtype.
    pub fn new(type_: NegotiationToken<'a>, subtype: NegotiationToken<'a>) -> Result<Self, DecodeError> {
        let kind = if type_.as_str() == "*" {
            if subtype.as_str() != "*" {
                return Err(invalid_syntax(&FieldName::Accept));
            }
            MediaRangeKind::Any
        } else if subtype.as_str() == "*" {
            MediaRangeKind::TypeWildcard
        } else {
            MediaRangeKind::Exact
        };
        Ok(Self { type_, subtype, kind })
    }

    pub(super) fn from_validated(bytes: &'a [u8]) -> Self {
        let slash = bytes
            .iter()
            .position(|byte| *byte == b'/')
            .expect("validated media ranges contain a slash");
        let text = str::from_utf8(bytes).expect("validated media ranges contain only ASCII bytes");
        Self::new(
            NegotiationToken::from_validated_str(&text[..slash]),
            NegotiationToken::from_validated_str(&text[slash + 1..]),
        )
        .expect("validated media ranges satisfy the cross-component wildcard rule")
    }

    /// Returns the range's retained wildcard classification.
    #[must_use]
    pub const fn kind(self) -> MediaRangeKind {
        self.kind
    }

    /// Returns the type token, including `*` for an any-type range.
    #[must_use]
    pub const fn type_(self) -> NegotiationToken<'a> {
        self.type_
    }

    /// Returns the subtype token, including `*` for wildcard ranges.
    #[must_use]
    pub const fn subtype(self) -> NegotiationToken<'a> {
        self.subtype
    }
}

impl fmt::Display for MediaRange<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.type_, self.subtype)
    }
}
