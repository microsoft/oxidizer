// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use super::FieldNameView;

/// A Vary member: either a wildcard or a validated field name.
///
/// Wildcards have no field name. Named members compare and hash
/// case-insensitively while retaining their original spelling.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VaryEntryView<'a>(Option<FieldNameView<'a>>);

impl<'a> VaryEntryView<'a> {
    /// The wildcard selection member.
    pub const WILDCARD: Self = Self(None);

    /// Constructs a member from a validated name.
    ///
    /// The name `*` becomes a wildcard, never a literal selection field.
    #[must_use]
    #[inline]
    pub fn from_field_name(name: FieldNameView<'a>) -> Self {
        if name.as_bytes() == b"*" {
            Self::WILDCARD
        } else {
            Self(Some(name))
        }
    }

    #[inline]
    pub(super) fn from_validated(bytes: &'a [u8]) -> Self {
        if bytes == b"*" {
            Self::WILDCARD
        } else {
            Self(Some(FieldNameView::from_validated(bytes)))
        }
    }

    /// Returns whether this member is the wildcard.
    #[must_use]
    #[inline]
    pub const fn is_wildcard(self) -> bool {
        self.0.is_none()
    }

    /// Returns the validated field name, or `None` for the wildcard.
    #[must_use]
    #[inline]
    pub const fn field_name(self) -> Option<FieldNameView<'a>> {
        self.0
    }

    /// Returns the member's original spelling.
    #[must_use]
    #[inline]
    pub fn as_str(self) -> &'a str {
        match self.0 {
            Some(name) => name.as_str(),
            None => "*",
        }
    }
}

impl fmt::Display for VaryEntryView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
