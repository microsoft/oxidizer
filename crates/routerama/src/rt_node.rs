// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Reverse;
use core::{iter, mem};

use routerama_build::trie::{Leaf, Node, affix_edges_in_match_order};

use crate::affix_edge::AffixEdge;
use crate::literal_edge::LiteralEdge;

/// Runtime representation of a [`Node`].
pub(crate) struct RtNode {
    pub(crate) literals: Box<[LiteralEdge]>,
    /// Affix edges, in most-specific-first match order.
    pub(crate) affix: Box<[AffixEdge]>,
    pub(crate) single: Option<Box<Self>>,
    pub(crate) exact: Box<[Leaf]>,
    pub(crate) rest: Box<[Leaf]>,
}

struct FlatNode {
    literals: Vec<(Box<str>, usize)>,
    affix: Vec<(Box<str>, Box<str>, usize)>,
    single: Option<usize>,
    exact: Box<[Leaf]>,
    rest: Box<[Leaf]>,
}

impl RtNode {
    /// Destroys a build-time trie without recursively dropping its nodes.
    pub(crate) fn discard_source(node: Node) {
        let mut pending = vec![node];
        while let Some(node) = pending.pop() {
            let Node {
                literals, affix, single, ..
            } = node;
            pending.extend(literals.into_values());
            pending.extend(affix.into_values());
            if let Some(child) = single {
                pending.push(*child);
            }
        }
    }

    #[expect(
        clippy::needless_collect,
        reason = "the collect ends a borrow of `node` so `node.affix` can be moved"
    )]
    pub(crate) fn compile(node: Node) -> Self {
        let mut source = vec![Some(node)];
        let mut flat = Vec::new();
        let mut index = 0;
        while index < source.len() {
            let node = source[index].take().expect("each queued source node is compiled exactly once");
            let affix_order: Vec<(String, String)> = affix_edges_in_match_order(&node).into_iter().map(|(key, _)| key.clone()).collect();
            let Node {
                literals,
                mut affix,
                single,
                exact,
                rest,
            } = node;
            let literals = literals
                .into_iter()
                .map(|(key, child)| {
                    let child_index = source.len();
                    source.push(Some(child));
                    (key.into_boxed_str(), child_index)
                })
                .collect();
            let affix = affix_order
                .into_iter()
                .map(|key| {
                    let child = affix
                        .remove(&key)
                        .expect("affix order was derived from this node and each key is removed once");
                    let child_index = source.len();
                    source.push(Some(child));
                    (key.0.into_boxed_str(), key.1.into_boxed_str(), child_index)
                })
                .collect();
            let single = single.map(|child| {
                let child_index = source.len();
                source.push(Some(*child));
                child_index
            });
            flat.push(FlatNode {
                literals,
                affix,
                single,
                exact: exact.into_boxed_slice(),
                rest: rest.into_boxed_slice(),
            });
            index += 1;
        }

        order_literals_by_weight(&mut flat);

        let mut built: Vec<Option<Self>> = iter::repeat_with(|| None).take(flat.len()).collect();
        for (index, node) in flat.into_iter().enumerate().rev() {
            let literals = node
                .literals
                .into_iter()
                .map(|(key, child)| {
                    (
                        key,
                        built[child]
                            .take()
                            .expect("children are queued after parents and built in reverse order"),
                    )
                })
                .collect();
            let affix = node
                .affix
                .into_iter()
                .map(|(prefix, suffix, child)| {
                    (
                        prefix,
                        suffix,
                        built[child]
                            .take()
                            .expect("children are queued after parents and built in reverse order"),
                    )
                })
                .collect();
            let single = node.single.map(|child| {
                Box::new(
                    built[child]
                        .take()
                        .expect("children are queued after parents and built in reverse order"),
                )
            });
            built[index] = Some(Self {
                literals,
                affix,
                single,
                exact: node.exact,
                rest: node.rest,
            });
        }
        built[0].take().expect("the root is always built")
    }

    // The iterative destruction strategy is validated by the isolated
    // deep-route test; deleting it only changes stack usage.
    #[cfg_attr(test, mutants::skip)]
    fn move_children_to(&mut self, pending: &mut Vec<Self>) {
        pending.extend(mem::take(&mut self.literals).into_vec().into_iter().map(|(_, child)| child));
        pending.extend(mem::take(&mut self.affix).into_vec().into_iter().map(|(_, _, child)| child));
        if let Some(child) = self.single.take() {
            pending.push(*child);
        }
    }
}

fn order_literals_by_weight(nodes: &mut [FlatNode]) {
    let weights = node_weights(nodes);
    for node in nodes {
        node.literals.sort_by_key(|(_, child)| Reverse(weights[*child]));
    }
}

fn node_weights(nodes: &[FlatNode]) -> Vec<usize> {
    let mut weights = vec![0_usize; nodes.len()];
    for index in (0..nodes.len()).rev() {
        let node = &nodes[index];
        weights[index] = node.exact.len()
            + node.rest.len()
            + node.literals.iter().map(|(_, child)| weights[*child]).sum::<usize>()
            + node.affix.iter().map(|(_, _, child)| weights[*child]).sum::<usize>()
            + node.single.map_or(0, |child| weights[child]);
    }
    weights
}

impl Drop for RtNode {
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        let mut pending = Vec::new();
        self.move_children_to(&mut pending);
        while let Some(mut node) = pending.pop() {
            node.move_children_to(&mut pending);
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    use http_path_template::{Grammar, PathTemplate};
    use routerama_build::Route;
    use routerama_build::trie::{Leaf, build_trie};

    use crate::raw_resolver::RawResolver;
    use crate::route_match::RouteMatch;
    use crate::rt_node::{FlatNode, RtNode, node_weights, order_literals_by_weight};

    fn flat_node(exact: usize) -> FlatNode {
        FlatNode {
            literals: Vec::new(),
            affix: Vec::new(),
            single: None,
            exact: (0..exact)
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

    #[test]
    fn literal_priority_includes_every_descendant_edge_kind() {
        let mut weighted = flat_node(1);
        weighted.rest = flat_node(1).exact;
        weighted.literals.push(("literal".into(), 2));
        weighted.affix.push(("prefix".into(), "suffix".into(), 3));
        weighted.single = Some(4);

        let mut root = flat_node(0);
        root.literals = vec![("low".into(), 2), ("weighted".into(), 1)];
        let mut nodes = vec![root, weighted, flat_node(2), flat_node(3), flat_node(4)];

        assert_eq!(node_weights(&nodes)[1], 11);
        order_literals_by_weight(&mut nodes);
        assert_eq!(nodes[0].literals[0].0.as_ref(), "weighted");
    }

    #[test]
    fn compile_builds_affix_edges_and_resolves_them() {
        let router = RawResolver::new([
            Route::new(
                "Image",
                "GET",
                PathTemplate::parse("/img-{id}.png", Grammar::default().with_segment_affixes()).expect("valid template"),
            ),
            Route::new(
                "Thumb",
                "GET",
                PathTemplate::parse("/thumb-{id}.jpg", Grammar::default().with_segment_affixes()).expect("valid template"),
            ),
        ]);
        let matched = router.resolve("GET", "/img-42.png").expect("affix route matches");
        assert_eq!(matched.name(), "Image");
        assert_eq!(matched.capture("id"), Some("42"));
        assert!(router.resolve("GET", "/img-.png").is_none());
    }

    #[test]
    fn source_discard_handles_single_wildcard_children() {
        let routes = [Route::new(
            "Wildcard",
            "GET",
            PathTemplate::parse("/{value}", Grammar::default()).expect("valid template"),
        )];
        RtNode::discard_source(build_trie(&routes).root);
    }
}
