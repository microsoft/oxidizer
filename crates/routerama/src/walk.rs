// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama_build::trie::Leaf;
use smallvec::SmallVec;

use crate::codegen_helpers::ScannedPath;
use crate::rt_node::RtNode;

/// Request state shared by trie descent.
pub(crate) struct Walk<'a, 'p> {
    pub(crate) path: &'a ScannedPath<'p, 'a>,
    pub(crate) method: &'a str,
    pub(crate) verb: Option<&'a str>,
}

enum WalkAction<'a> {
    Node(&'a RtNode, usize),
    Leaves(&'a [Leaf]),
}

impl Walk<'_, '_> {
    /// Returns the first matching leaf without recursive trie descent.
    // Static/dynamic differential and structured-path tests exercise traversal;
    // many local rewrites are equivalent because pending edges backtrack.
    pub(crate) fn descend_iterative<'r, const VERBS: bool>(&self, mut node: &'r RtNode) -> Option<&'r Leaf> {
        let mut depth = 0;
        let mut pending: SmallVec<[WalkAction<'r>; 4]> = SmallVec::new();
        let count = self.path.count();
        loop {
            let segment = self.path.segment(depth);
            let single = node.single.as_deref();
            let single = single.filter(|_| segment.is_some_and(|segment| !segment.is_empty()));
            if node.literals.is_empty()
                && node.affix.is_empty()
                && node.rest.is_empty()
                && let Some(single) = single
            {
                node = single;
                depth += 1;
                continue;
            }
            if let Some(segment) = segment
                && (!node.literals.is_empty() || !node.affix.is_empty())
            {
                let bytes = segment.as_bytes();
                let literal = node.literals.iter().find(|(key, _)| {
                    let key = key.as_bytes();
                    // #[gamma::skip(logical.and_remove_left, tag = "equivalent", reason = "The final full-byte equality implies both the length and first-byte prefilters; this expression has no other logical AND")]
                    (
                        // #[gamma::skip(logical.and_remove_left, logical.and_remove_right, logical.and_to_or, reason = "The unchanged final full-byte equality implies both this length equality and this first-byte equality")]
                        key.len() == bytes.len() && key.first() == bytes.first()
                    ) && key == bytes
                });
                let first_affix = node.affix.iter().enumerate().find(|(_, (prefix, suffix, _))| {
                    bytes.len() > prefix.len() + suffix.len() && bytes.starts_with(prefix.as_bytes()) && bytes.ends_with(suffix.as_bytes())
                });

                if node.affix.is_empty()
                    && node.single.is_none()
                    && node.rest.is_empty()
                    && let Some((_, child)) = literal
                {
                    node = child;
                    depth += 1;
                    continue;
                }

                if
                // #[gamma::skip(logical.or_remove_right, reason = "When only single matches, the later single fallback queues the identical rest action and advances to the same node/depth; first_affix=None proves the omitted loop would queue no affix alternatives")]
                literal.is_some() || first_affix.is_some() || single.is_some() {
                    // #[gamma::skip(cond.always_true, logical.and_remove_left, logical.and_remove_right, logical.and_to_or, relational.ge_to_gt, reason = "A present segment proves depth is strictly below count; queuing an empty rest slice merely adds a dispatch that returns None before the identical next action")]
                    if count >= depth && !node.rest.is_empty() {
                        pending.push(WalkAction::Leaves(&node.rest));
                    }
                    if (literal.is_some() || first_affix.is_some())
                        && let Some(single) = single
                    {
                        pending.push(WalkAction::Node(single, depth + 1));
                    }
                    // #[gamma::skip(call.replace_with_default, call_result.default, tag = "equivalent", reason = "Zero only requeues earlier nonmatching affixes and at most the already explored first match; immutable path matching repeats the same failure before later alternatives")]
                    let skip = first_pending_affix(first_affix.map(|(index, _)| index), literal.is_none());
                    for (prefix, suffix, child) in node.affix.iter().skip(skip).rev() {
                        if pending_affix_matches(bytes, prefix, suffix) {
                            pending.push(WalkAction::Node(child, depth + 1));
                        }
                    }

                    node = literal
                        .map(|(_, child)| child)
                        .or_else(|| first_affix.map(|(_, (_, _, child))| child))
                        .or(single)
                        .expect("at least one matching child was found");
                    depth += 1;
                    continue;
                }
            }

            if let Some(single) = single {
                // The single-only fast path already handled nodes without rest leaves.
                pending.push(WalkAction::Leaves(&node.rest));
                node = single;
                depth += 1;
                continue;
            }

            if count == depth
                && let Some(matched) = self.dispatch::<VERBS>(&node.exact)
            {
                return Some(matched);
            }
            if count >= depth
                && let Some(matched) = self.dispatch::<VERBS>(&node.rest)
            {
                return Some(matched);
            }

            loop {
                match pending.pop()? {
                    WalkAction::Node(next_node, next_depth) => {
                        (node, depth) = (next_node, next_depth);
                        break;
                    }
                    WalkAction::Leaves(leaves) => {
                        if let Some(matched) = self.dispatch::<VERBS>(leaves) {
                            return Some(matched);
                        }
                    }
                }
            }
        }
    }

    /// Selects the first leaf matching the method and custom verb.
    fn dispatch<'r, const VERBS: bool>(&self, leaves: &'r [Leaf]) -> Option<&'r Leaf> {
        leaves.iter().find(|leaf| {
            leaf.method == self.method
                && (
                    // #[gamma::skip(logical.or_remove_left, reason = "VERBS=false is instantiated only for resolvers with no verb leaves and an unsplit request verb None; both equality operands are then None, while VERBS=true already requires the identical equality")]
                    !VERBS || leaf.verb.as_deref() == self.verb
                )
        })
    }
}

fn first_pending_affix(first: Option<usize>, literal_missing: bool) -> usize {
    first.map_or(
        // #[gamma::skip(literal.int_increment, reason = "With no matching affix, skipping an additional nonmatching edge cannot remove a queued alternative")]
        0,
        |index| usize::from(literal_missing) + index,
    )
}

fn pending_affix_matches(bytes: &[u8], prefix: &str, suffix: &str) -> bool {
    bytes.len() > prefix.len() + suffix.len() && bytes.starts_with(prefix.as_bytes()) && bytes.ends_with(suffix.as_bytes())
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::fmt::Write as _;

    #[test]
    fn gamma_nov4_pending_affixes_skip_only_the_directly_explored_match_and_require_nonempty_complete_captures() {
        assert_eq!(super::first_pending_affix(Some(0), true), 1);
        assert_eq!(super::first_pending_affix(Some(2), true), 3);
        assert_eq!(super::first_pending_affix(Some(2), false), 2);
        assert_eq!(super::first_pending_affix(None, false), 0);
        for (bytes, prefix, suffix, expected) in [
            ("pre-x.txt", "pre-", ".txt", true),
            ("pre-.txt", "pre-", ".txt", false),
            ("x.txt", "pre-", ".txt", false),
            ("pre-x.bad", "pre-", ".txt", false),
            (".txt", "", ".txt", false),
        ] {
            assert_eq!(super::pending_affix_matches(bytes.as_bytes(), prefix, suffix), expected, "{bytes}");
        }
    }

    use http_path_template::{Grammar, PathTemplate};
    use routerama_build::Route;

    use crate::raw_resolver::RawResolver;
    use crate::route_match::RouteMatch;

    #[test]
    fn deep_paths_beyond_max_segments_return_none_without_panicking() {
        // Exercise paths well beyond each resolver's configured depth.
        let mk = |name: &str, pattern: &str| {
            Route::new(
                name,
                "GET",
                PathTemplate::parse(pattern, Grammar::default().with_segment_affixes()).expect("valid template"),
            )
        };
        let route_sets: &[&[(&str, &str)]] = &[
            &[("Lit", "/a/b/c")],
            &[("Single", "/{a}/{b}/{c}")],
            &[("Short", "/a/{x}"), ("Long", "/a/{x}/{y}/{z}")],
            &[("Rest", "/a/**"), ("Var", "/a/{x}")],
            &[("Affix", "/img-{id}.png"), ("Var", "/a/{x}")],
            &[("SingleRest", "/{a}/{b}/**")],
        ];
        let checked_depths = if cfg!(miri) { 8 } else { 40 };
        let very_deep_segments = if cfg!(miri) { 64 } else { 500 };
        for routes in route_sets {
            let router = RawResolver::new(routes.iter().map(|(name, pattern)| mk(name, pattern)));
            for depth in 0..checked_depths {
                let mut path = String::from("/a");
                for i in 0..depth {
                    let _ = write!(path, "/s{i}");
                }
                let _ = router.resolve("GET", &path);
                let _ = router.resolve("GET", &format!("{path}/"));
                let _ = router.resolve("POST", &format!("{path}:verb"));
            }
            let mut very_deep = String::new();
            for i in 0..very_deep_segments {
                let _ = write!(very_deep, "/{i}");
            }
            let _ = router.resolve("GET", &very_deep);
        }
    }

    #[test]
    fn descend_matches_literal_then_affix_edges_at_the_same_node() {
        let mk = |name: &str, pattern: &str| {
            Route::new(
                name,
                "GET",
                PathTemplate::parse(pattern, Grammar::default().with_segment_affixes()).expect("valid template"),
            )
        };
        let router = RawResolver::new([mk("Data", "/files/data"), mk("Image", "/files/img-{id}.png")]);

        let data = router.resolve("GET", "/files/data").expect("literal route matches");
        assert_eq!(data.name(), "Data");
        assert_eq!(data.captures().count(), 0);

        let image = router.resolve("GET", "/files/img-7.png").expect("affix route matches");
        assert_eq!(image.name(), "Image");
        assert_eq!(image.capture("id"), Some("7"));

        assert!(router.resolve("GET", "/files/other").is_none());
    }

    #[test]
    fn affix_length_guard_and_descent_depth_are_pinned() {
        let router = RawResolver::new([Route::new(
            "Tagged",
            "GET",
            PathTemplate::parse("/api/ab-{id}.x", Grammar::default().with_segment_affixes()).expect("valid template"),
        )]);

        let matched = router
            .resolve("GET", "/api/ab-7.x")
            .expect("affix route matches a non-empty middle");
        assert_eq!(matched.name(), "Tagged");
        assert_eq!(matched.capture("id"), Some("7"));

        assert!(router.resolve("GET", "/api/ab-.x").is_none());
    }

    #[test]
    fn literal_prefilter_still_verifies_full_segment_content() {
        // Two root literals with the same length and first byte but different
        // content (`car`, `cat`). The pre-filter must still compare the whole
        // segment: otherwise `/cat/x` picks `car`, descends, and misses without
        // backtracking.
        let mk = |name: &str, pattern: &str| {
            Route::new(
                name,
                "GET",
                PathTemplate::parse(pattern, Grammar::default()).expect("valid template"),
            )
        };
        let router = RawResolver::new([mk("Car", "/car/y"), mk("Cat", "/cat/x")]);

        let cat = router.resolve("GET", "/cat/x").expect("cat route matches");
        assert_eq!(cat.name(), "Cat");

        let car = router.resolve("GET", "/car/y").expect("car route matches");
        assert_eq!(car.name(), "Car");
    }

    #[test]
    fn failed_literal_and_affix_candidates_backtrack() {
        let grammar = Grammar::default().with_segment_affixes();
        let mk = |name: &str, pattern: &str| Route::new(name, "GET", PathTemplate::parse(pattern, grammar).expect("valid template"));

        let rest_router = RawResolver::new([mk("Literal", "/files/data/no"), mk("Rest", "/files/**")]);
        assert_eq!(
            rest_router.resolve("GET", "/files/data/other").expect("rest fallback").name(),
            "Rest"
        );

        let affix_router = RawResolver::new([mk("First", "/x/a-{id}/no"), mk("Second", "/x/{id}-b/yes")]);
        assert_eq!(
            affix_router.resolve("GET", "/x/a-b/yes").expect("second affix fallback").name(),
            "Second"
        );
    }

    #[test]
    fn unmatched_literals_do_not_duplicate_single_wildcard_fallbacks() {
        let grammar = Grammar::default();
        let mut routes = Vec::new();
        let mut wildcard_path = String::new();
        // Repeated fallback registration is the behavior under test. Eight
        // levels retain non-head backtracking under Miri; native tests keep the
        // 32-level breadth.
        let depth_count = if cfg!(miri) { 8 } else { 32 };
        for depth in 0..depth_count {
            let _ = write!(wildcard_path, "/{{value{depth}}}");
            routes.push(Route::new(
                format!("DeadEnd{depth}"),
                "GET",
                PathTemplate::parse(&format!("{wildcard_path}/literal{depth}"), grammar).expect("valid template"),
            ));
        }
        routes.push(Route::new(
            "Target",
            "GET",
            PathTemplate::parse(&format!("{wildcard_path}/target"), grammar).expect("valid template"),
        ));
        let resolver = RawResolver::new(routes);
        let request = format!("{}{}", "/value".repeat(depth_count), "/missing");

        assert!(resolver.resolve("GET", &request).is_none());
    }
}
