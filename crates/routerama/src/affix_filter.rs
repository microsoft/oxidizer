// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A cheap, necessary-but-not-sufficient rejection test for one affix edge.
///
/// [`AffixEdge`](crate::affix_edge::AffixEdge) is 104 bytes and holds its
/// prefix and suffix behind separate heap allocations, so testing a segment
/// against a node's affix edges directly costs one strided read plus two
/// pointer dereferences per edge. A node with wide affix fanout therefore
/// walks hundreds of kilobytes to reject candidates, and the scan becomes
/// memory-bound long before the byte comparisons matter.
///
/// This filter mirrors `RtNode::affix` element for element in the same match
/// order, but stores only the three facts that reject most candidates
/// outright. It is 8 bytes, so a scan reads a compact contiguous array and
/// touches the wide edge — and its two heap allocations — only for the few
/// candidates that survive.
///
/// A pass does not confirm a match: the caller must still run the full
/// prefix/suffix comparison. A failure is conclusive.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AffixFilter {
    /// `prefix.len() + suffix.len()`; a matching segment must be longer.
    min_len: u32,
    /// The first prefix byte, or `None` when the prefix is empty.
    first: Option<u8>,
    /// The last suffix byte, or `None` when the suffix is empty.
    last: Option<u8>,
}

impl AffixFilter {
    /// Derives the filter for an edge with the given prefix and suffix.
    pub(crate) fn new(prefix: &str, suffix: &str) -> Self {
        Self {
            // A prefix and suffix long enough to overflow `u32` cannot be
            // matched by any segment, so saturating keeps every such edge
            // rejected without widening the filter.
            min_len: u32::try_from(prefix.len().saturating_add(suffix.len())).unwrap_or(u32::MAX),
            first: prefix.as_bytes().first().copied(),
            last: suffix.as_bytes().last().copied(),
        }
    }

    /// Returns whether `segment` could match the edge this filter describes.
    #[inline]
    pub(crate) fn admits(self, segment: &[u8]) -> bool {
        segment.len() as u64 > u64::from(self.min_len)
            && self.first.is_none_or(|byte| segment.first() == Some(&byte))
            && self.last.is_none_or(|byte| segment.last() == Some(&byte))
    }
}
