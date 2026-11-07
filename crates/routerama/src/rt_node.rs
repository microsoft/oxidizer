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

#[cfg(test)]
mod gamma_round2_tests {
    use http_path_template::{Grammar, PathTemplate};
    use routerama_build::Route;
    use routerama_build::trie::build_trie;

    use super::*;

    std::thread_local! {
        static DROP_EVENTS: std::cell::RefCell<Option<Vec<Vec<String>>>> = const { std::cell::RefCell::new(None) };
    }

    pub(super) fn observe_drop(node: &RtNode) {
        DROP_EVENTS.with_borrow_mut(|events| {
            if let Some(events) = events {
                events.push(node.exact.iter().chain(node.rest.iter()).map(|leaf| leaf.name.clone()).collect());
            }
        });
    }

    #[test]
    fn gamma_nov4_actual_drop_visits_every_descendant_in_iterative_family_order() {
        let routes = [
            route("literal", "/literal/deep"),
            route("affix", "/pre-{id}.txt/deep"),
            route("single", "/{id}/deep"),
            route("root", "/root"),
        ];
        let root = RtNode::compile(build_trie(&routes).root);
        DROP_EVENTS.with_borrow_mut(|events| assert!(events.replace(Vec::new()).is_none()));
        drop(root);
        let events = DROP_EVENTS.with_borrow_mut(Option::take).unwrap();
        assert_eq!(
            events,
            [
                vec![],
                vec![String::from("single")],
                vec![],
                vec![String::from("affix")],
                vec![String::from("root")],
                vec![],
                vec![String::from("literal")],
            ]
        );
    }

    fn route(name: &str, path: &str) -> Route {
        Route::new(
            name,
            "GET",
            PathTemplate::parse(path, Grammar::default().with_segment_affixes()).unwrap(),
        )
    }

    #[test]
    fn gamma_round2_child_extraction_empties_all_edge_families_once() {
        let trie = build_trie(&[
            route("literal", "/literal"),
            route("affix", "/pre-{id}.txt"),
            route("single", "/{id}"),
        ]);
        let mut root = RtNode::compile(trie.root);
        let mut children = Vec::new();
        root.move_children_to(&mut children);
        assert!(root.literals.is_empty());
        assert!(root.affix.is_empty());
        assert!(root.single.is_none());
        assert_eq!(children.len(), 3);
        assert_eq!(children.iter().map(|node| node.exact.len()).sum::<usize>(), 3);
        root.move_children_to(&mut children);
        assert_eq!(children.len(), 3);
    }

    #[test]
    fn gamma_round2_iterative_destruction_visits_every_edge_family_and_grandchild_once() {
        let routes = [
            route("literal", "/literal/deep"),
            route("affix", "/pre-{id}.txt/deep"),
            route("single", "/{id}/deep"),
            route("root", "/root"),
        ];
        let mut source_names = Vec::new();
        let mut source_nodes = 0;
        RtNode::discard_source_with(build_trie(&routes).root, |node| {
            source_nodes += 1;
            source_names.extend(node.exact.iter().map(|leaf| leaf.name.clone()));
        });
        source_names.sort();
        assert_eq!(source_nodes, 8);
        assert_eq!(source_names, ["affix", "literal", "root", "single"]);
        let mut root = RtNode::compile(build_trie(&routes).root);
        let mut runtime_names = Vec::new();
        let mut runtime_nodes = 0;
        root.drop_descendants_with(|node| {
            runtime_nodes += 1;
            runtime_names.extend(node.exact.iter().map(|leaf| leaf.name.clone()));
        });
        runtime_names.sort();
        assert_eq!(runtime_nodes, 7);
        assert_eq!(runtime_names, ["affix", "literal", "root", "single"]);
        assert!(root.literals.is_empty());
        assert!(root.affix.is_empty());
        assert!(root.single.is_none());
    }

    #[test]
    fn gamma_round2_compile_orders_literals_by_descendant_leaf_weight() {
        let trie = build_trie(&[
            route("light", "/a"),
            route("heavy1", "/z/one"),
            route("heavy2", "/z/two"),
            route("heavy3", "/z/{id}"),
        ]);
        let root = RtNode::compile(trie.root);
        assert_eq!(root.literals.iter().map(|(key, _)| &**key).collect::<Vec<_>>(), ["z", "a"]);
        let empty = FlatNode {
            literals: Vec::new(),
            affix: Vec::new(),
            single: None,
            exact: Box::new([]),
            rest: Box::new([]),
        };
        assert_eq!(node_weights(&[empty]), [0]);
    }
}

impl RtNode {
    /// Destroys a build-time trie without recursively dropping its nodes.
    pub(crate) fn discard_source(node: Node) {
        Self::discard_source_with(node, |_| {});
    }

    fn discard_source_with(node: Node, mut visit: impl FnMut(&Node)) {
        let mut pending = vec![node];
        while let Some(node) = pending.pop() {
            visit(&node);
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

    fn drop_descendants_with(&mut self, mut visit: impl FnMut(&Self)) {
        let mut pending = Vec::new();
        self.move_children_to(&mut pending);
        while let Some(mut node) = pending.pop() {
            visit(&node);
            node.move_children_to(&mut pending);
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
    for index in (
        // #[gamma::skip(literal.int_increment, reason = "Flat node zero is the root and is never a child; order_literals_by_weight reads only child weights, so computing or leaving the unused root weight at zero cannot affect ordering")]
        0..nodes.len()
    )
        .rev()
    {
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
        self.drop_descendants_with(observe_drop);
    }
}

#[cfg(test)]
use gamma_round2_tests::observe_drop;

#[cfg(not(test))]
fn observe_drop(_: &RtNode) {}

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
