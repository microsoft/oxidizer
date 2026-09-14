// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::language_range::LanguageRange;
use super::negotiation_members::{append_weight, checked_len, weight_len, weighted_parts};
use super::quality::QualityView;
use crate::{DecodeError, FieldName};

/// One validated Accept-Language preference with retained range and quality.
///
/// The basic language-range grammar does not perform locale selection,
/// normalization or fallback. Missing quality remains distinct from `q=1`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AcceptLanguageEntry<'a> {
    range: LanguageRange<'a>,
    quality: Option<QualityView<'a>>,
}

impl<'a> AcceptLanguageEntry<'a> {
    /// Constructs a language preference from validated components.
    ///
    /// # Errors
    ///
    /// Returns an error if its serialized size exceeds
    /// [`MAX_CUSTOM_FIELD_BYTES`](crate::source::MAX_CUSTOM_FIELD_BYTES).
    pub fn new(range: LanguageRange<'a>, quality: Option<QualityView<'a>>) -> Result<Self, DecodeError> {
        let entry = Self { range, quality };
        entry.encoded_len()?;
        Ok(entry)
    }

    pub(super) fn from_validated(bytes: &'a [u8]) -> Self {
        let (range, quality) = weighted_parts(bytes);
        Self {
            range: LanguageRange::from_validated(range),
            quality,
        }
    }

    /// Returns the validated basic language range.
    #[must_use]
    pub const fn range(self) -> LanguageRange<'a> {
        self.range
    }

    /// Returns the exact quality, defaulting to one when `q` was omitted.
    #[must_use]
    pub const fn quality(self) -> QualityView<'a> {
        match self.quality {
            Some(quality) => quality,
            None => QualityView::ONE,
        }
    }

    /// Returns only an explicitly supplied quality.
    #[must_use]
    pub const fn explicit_quality(self) -> Option<QualityView<'a>> {
        self.quality
    }

    pub(super) fn encoded_len(&self) -> Result<usize, DecodeError> {
        checked_len([self.range.as_str().len(), weight_len(self.quality)], &FieldName::AcceptLanguage)
    }

    pub(super) fn append_to(&self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(self.range.as_str().as_bytes());
        append_weight(bytes, self.quality);
    }
}
