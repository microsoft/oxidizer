// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;

use crate::rt_node::RtNode;

/// A literal edge: an exact segment key and the subtree it leads to.
pub(crate) type LiteralEdge = (Box<str>, RtNode);

/// Sibling-literal count at which a node switches lookup strategy.
///
/// Below this fanout, [`RtNode::compile`](RtNode::compile) keeps a node's
/// literal edges ordered by descending subtree weight and [`find_literal`]
/// scans them linearly, so the busiest subtree is tried first. At or above it,
/// compilation sorts the same edges by key bytes and [`find_literal`] binary
/// searches them, which is what keeps a wide runtime-registered table — an
/// erased mount table, a `#[route(dynamic)]` table, or a
/// [`RawResolver`](crate::raw_resolver::RawResolver) — from costing work
/// proportional to a matched entry's registration position. Generated static
/// routers use neither path: their segment dispatch is compiled.
///
/// Ordering is a lookup heuristic only. Literal keys are unique within a node,
/// so at most one can equal a segment; no precedence rule, capture, or
/// backtracking order depends on the order the edges are stored in.
///
/// The value is measured, not assumed. `docs/PERF.md` records the mount
/// scaling matrix run twice over eight table widths, once with each strategy
/// forced everywhere: `16` is the width at which binary search stops being a
/// mean regression (`1.00x` the four positions' mean) while already removing
/// `12%` of the last-registered entry's cost, `8` and `12` are still `3-4%`
/// mean regressions, and every narrower node — which is most of a real trie —
/// pays up to `7%` for a search it does not need.
///
/// The two sides are one invariant: [`find_literal`] picks its strategy from
/// `literals.len()` alone, so [`RtNode::compile`](RtNode::compile) must sort
/// exactly the nodes whose fanout reaches this threshold.
pub(crate) const SORTED_LITERAL_FANOUT: usize = 16;

/// Returns the subtree behind the literal edge whose key equals `segment`.
///
/// See [`SORTED_LITERAL_FANOUT`] for the strategy split and for the invariant
/// that ties this lookup to [`RtNode::compile`](RtNode::compile).
// The narrow-node scan is inlined into the descent loop and the wide-node
// search is kept out of line: inlining both costs every node visit about 19
// instructions, and outlining both costs a deep descent about 17 per segment.
#[inline]
pub(crate) fn find_literal<'e>(literals: &'e [LiteralEdge], segment: &[u8]) -> Option<&'e RtNode> {
    if literals.len() >= SORTED_LITERAL_FANOUT {
        return find_sorted_literal(literals, segment);
    }
    literals
        .iter()
        .find(|(key, _)| {
            let key = key.as_bytes();
            key.len() == segment.len() && key.first() == segment.first() && key == segment
        })
        .map(|(_, child)| child)
}

/// Binary searches a node whose keys `RtNode::compile` sorted by byte order.
fn find_sorted_literal<'e>(literals: &'e [LiteralEdge], segment: &[u8]) -> Option<&'e RtNode> {
    debug_assert!(
        literals.is_sorted_by(|(left, _), (right, _)| left.as_bytes() <= right.as_bytes()),
        "`RtNode::compile` sorts the keys of every node holding at least `SORTED_LITERAL_FANOUT` literal edges"
    );
    literals
        .binary_search_by(|(key, _)| key.as_bytes().cmp(segment))
        .ok()
        .and_then(|index| literals.get(index))
        .map(|(_, child)| child)
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::format;
    use alloc::string::{String, ToString as _};
    use alloc::vec::Vec;

    use routerama_build::trie::Leaf;

    use crate::literal_edge::{LiteralEdge, SORTED_LITERAL_FANOUT, find_literal};
    use crate::rt_node::RtNode;

    /// Builds a childless node carrying `marker + 1` exact leaves, so a lookup
    /// result can be told apart from its siblings.
    fn leaf_node(marker: usize) -> RtNode {
        RtNode {
            literals: Box::default(),
            affix: Box::default(),
            single: None,
            exact: (0..=marker)
                .map(|route_index| Leaf {
                    method: String::new(),
                    verb: None,
                    name: String::new(),
                    vars: Vec::new(),
                    route_index,
                })
                .collect(),
            rest: Box::default(),
        }
    }

    /// Recovers the marker [`leaf_node`] encoded into a subtree.
    fn marker_of(node: &RtNode) -> usize {
        node.exact.len() - 1
    }

    /// Builds edges in the given order, marking each with its position.
    fn edges(keys: &[&str]) -> Vec<LiteralEdge> {
        keys.iter()
            .enumerate()
            .map(|(marker, key)| ((*key).to_string().into_boxed_str(), leaf_node(marker)))
            .collect()
    }

    /// The reference lookup: the linear scan the hybrid replaces above the
    /// threshold.
    fn reference(literals: &[LiteralEdge], segment: &[u8]) -> Option<usize> {
        literals
            .iter()
            .find(|(key, _)| key.as_bytes() == segment)
            .map(|(_, child)| marker_of(child))
    }

    #[test]
    fn narrow_nodes_find_every_key_and_reject_misses() {
        let literals = edges(&["alpha", "beta", "gamma"]);

        assert_eq!(find_literal(&literals, b"alpha").map(marker_of), Some(0));
        assert_eq!(find_literal(&literals, b"beta").map(marker_of), Some(1));
        assert_eq!(find_literal(&literals, b"gamma").map(marker_of), Some(2));
        assert!(find_literal(&literals, b"delta").is_none());
        assert!(find_literal(&literals, b"").is_none());
        assert!(find_literal(&literals, b"alph").is_none());
        assert!(find_literal(&literals, b"alphas").is_none());
    }

    #[test]
    fn empty_and_boundary_widths_agree_with_a_linear_scan() {
        for width in [0, 1, SORTED_LITERAL_FANOUT - 1, SORTED_LITERAL_FANOUT, SORTED_LITERAL_FANOUT + 1] {
            let keys: Vec<String> = (0..width).map(|index| format!("key-{index:04}")).collect();
            let literals = edges(&keys.iter().map(String::as_str).collect::<Vec<_>>());

            for key in &keys {
                assert_eq!(
                    find_literal(&literals, key.as_bytes()).map(marker_of),
                    reference(&literals, key.as_bytes()),
                    "width {width} disagreed on `{key}`"
                );
            }
            assert!(find_literal(&literals, b"key-9999").is_none(), "width {width} matched a miss");
        }
    }

    #[test]
    fn wide_nodes_match_a_linear_scan_on_random_hits_and_misses() {
        // Deterministic xorshift, so the property runs identically everywhere.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        for width in [SORTED_LITERAL_FANOUT, SORTED_LITERAL_FANOUT + 7, 4 * SORTED_LITERAL_FANOUT] {
            // Keys of varied length and with shared prefixes, so byte-order
            // comparisons and length differences both matter.
            let mut keys: Vec<String> = (0..width)
                .map(|index| {
                    let depth = usize::try_from(next() % 3).expect("a remainder below three fits in a usize");
                    format!("{}{index}-{}", "prefix/".repeat(depth), next() % 1_000)
                })
                .collect();
            keys.sort_unstable();
            keys.dedup();

            let mut literals = edges(&keys.iter().map(String::as_str).collect::<Vec<_>>());
            literals.sort_unstable_by(|(left, _), (right, _)| left.as_bytes().cmp(right.as_bytes()));

            for key in &keys {
                assert_eq!(
                    find_literal(&literals, key.as_bytes()).map(marker_of),
                    reference(&literals, key.as_bytes()),
                    "width {width} disagreed on hit `{key}`"
                );
            }
            for _ in 0..256 {
                let probe = format!("{}-{}", next() % 4_096, next() % 4_096);
                assert_eq!(
                    find_literal(&literals, probe.as_bytes()).map(marker_of),
                    reference(&literals, probe.as_bytes()),
                    "width {width} disagreed on probe `{probe}`"
                );
            }
        }
    }
}
