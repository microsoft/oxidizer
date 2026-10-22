// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;

use crate::rt_node::RtNode;

/// A trie edge matching segments that start with `prefix` and end with `suffix`.
///
/// The edge owns the subtree reached when a segment satisfies both conditions.
pub(crate) type AffixEdge = (Box<str>, Box<str>, RtNode);

/// Returns whether `segment` is bracketed by the edge's prefix and suffix.
///
/// The segment must be strictly longer than the two affixes combined, so the
/// capture between them is never empty.
///
/// This is the authoritative test. [`AffixFilter`](crate::affix_filter::AffixFilter)
/// only screens candidates cheaply before this runs.
pub(crate) fn brackets((prefix, suffix, _): &AffixEdge, segment: &[u8]) -> bool {
    segment.len() > prefix.len() + suffix.len() && segment.starts_with(prefix.as_bytes()) && segment.ends_with(suffix.as_bytes())
}
