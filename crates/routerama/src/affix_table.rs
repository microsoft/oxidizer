// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;

use crate::affix_edge::{AffixEdge, brackets};
use crate::affix_filter::AffixFilter;
use crate::rt_node::RtNode;

/// A node's affix edges together with their rejection filters.
///
/// Held behind a single `Option<Box<_>>` in [`RtNode`] rather than as two
/// inline slice fields. Two `Box<[_]>` fields would add 32 bytes to every
/// node, and `RtNode` is embedded by value in each of its parent's literal
/// edges, so the cost would fall on literal-only routing that never looks at
/// an affix edge — measured at three percent for flat literal fanout and seven percent for deep
/// paths. Going through a pointer instead makes `RtNode` smaller than it was with one
/// inline slice, and the extra pointer hop is paid once per node visit and
/// only by nodes that actually have affix edges.
pub(crate) struct AffixTable {
    /// Affix edges, in most-specific-first match order.
    edges: Box<[AffixEdge]>,
    /// Filters mirroring [`edges`](Self::edges) element for element.
    filters: Box<[AffixFilter]>,
}

impl AffixTable {
    /// Builds the table for a node, or `None` when it has no affix edges.
    pub(crate) fn new(edges: Box<[AffixEdge]>) -> Option<Box<Self>> {
        if edges.is_empty() {
            return None;
        }
        let filters = edges.iter().map(|(prefix, suffix, _)| AffixFilter::new(prefix, suffix)).collect();
        Some(Box::new(Self { edges, filters }))
    }

    /// Returns the first edge matching `segment`, with its index in match order.
    #[inline]
    pub(crate) fn first_match(&self, segment: &[u8]) -> Option<(usize, &AffixEdge)> {
        // Zipping the slices keeps the scan free of bounds checks, and `&&`
        // short-circuits, so a rejected edge is never dereferenced: the filters
        // are read contiguously while the 104-byte edges and the two heap
        // allocations behind each of them stay untouched.
        self.filters
            .iter()
            .zip(self.edges.iter())
            .enumerate()
            .find(|(_, (filter, edge))| filter.admits(segment) && brackets(edge, segment))
            .map(|(index, (_, edge))| (index, edge))
    }

    /// Returns the subtrees of every edge after `first` matching `segment`.
    ///
    /// Only reached once an earlier edge matched, so the remaining edges are
    /// tested directly rather than through a second filtered scan.
    #[inline]
    pub(crate) fn matches_after<'s, 'b>(
        &'s self,
        first: usize,
        segment: &'b [u8],
    ) -> impl DoubleEndedIterator<Item = &'s RtNode> + use<'s, 'b> {
        self.edges
            .get(first + 1..)
            .unwrap_or_default()
            .iter()
            .filter_map(move |edge| brackets(edge, segment).then_some(&edge.2))
    }

    /// Consumes the table and returns its edges, for iterative destruction.
    pub(crate) fn into_edges(self) -> Box<[AffixEdge]> {
        self.edges
    }
}
