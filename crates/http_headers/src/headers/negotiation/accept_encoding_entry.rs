// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::content_coding::ContentCoding;
use super::negotiation_members::{append_weight, checked_len, weight_len, weighted_parts};
use super::quality::QualityView;
use crate::{DecodeError, FieldName};

/// One validated Accept-Encoding preference with retained coding and quality.
///
/// Wildcard, identity and extension codings remain distinct. No implicit
/// identity entry, sorting or codec selection is performed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AcceptEncodingEntry<'a> {
    coding: ContentCoding<'a>,
    quality: Option<QualityView<'a>>,
}

impl<'a> AcceptEncodingEntry<'a> {
    /// Constructs a coding preference without revalidating its components.
    ///
    /// # Errors
    ///
    /// Returns an error if its serialized size exceeds
    /// [`MAX_CUSTOM_FIELD_BYTES`](crate::source::MAX_CUSTOM_FIELD_BYTES).
    pub fn new(coding: ContentCoding<'a>, quality: Option<QualityView<'a>>) -> Result<Self, DecodeError> {
        let entry = Self { coding, quality };
        entry.encoded_len()?;
        Ok(entry)
    }

    pub(super) fn from_validated(bytes: &'a [u8]) -> Self {
        let (coding, quality) = weighted_parts(bytes);
        Self {
            coding: ContentCoding::from_validated(coding),
            quality,
        }
    }

    /// Returns the retained coding classification and original token.
    #[must_use]
    pub const fn coding(self) -> ContentCoding<'a> {
        self.coding
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
        checked_len([self.coding.as_str().len(), weight_len(self.quality)], &FieldName::AcceptEncoding)
    }

    pub(super) fn append_to(&self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(self.coding.as_str().as_bytes());
        append_weight(bytes, self.quality);
    }
}
