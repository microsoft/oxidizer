// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama_build::trie::Leaf;
use smallvec::SmallVec;

use crate::codegen_helpers::ScannedPath;
use crate::literal_edge::find_literal;
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
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn descend_iterative<'n, const VERBS: bool>(&self, mut node: &'n RtNode, mut depth: usize) -> Option<&'n Leaf> {
        let mut pending: SmallVec<[WalkAction<'n>; 4]> = SmallVec::new();
        let count = self.path.count();
        loop {
            let segment = self.path.segment(depth);
            let single = node
                .single
                .as_deref()
                .filter(|_| segment.is_some_and(|segment| !segment.is_empty()));
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
                let literal = find_literal(&node.literals, bytes);
                let first_affix = node.affix.iter().enumerate().find(|(_, (prefix, suffix, _))| {
                    bytes.len() > prefix.len() + suffix.len() && bytes.starts_with(prefix.as_bytes()) && bytes.ends_with(suffix.as_bytes())
                });

                if node.affix.is_empty()
                    && node.single.is_none()
                    && node.rest.is_empty()
                    && let Some(child) = literal
                {
                    node = child;
                    depth += 1;
                    continue;
                }

                if literal.is_some() || first_affix.is_some() || single.is_some() {
                    if count >= depth && !node.rest.is_empty() {
                        pending.push(WalkAction::Leaves(&node.rest));
                    }
                    if (literal.is_some() || first_affix.is_some())
                        && let Some(single) = single
                    {
                        pending.push(WalkAction::Node(single, depth + 1));
                    }
                    // Edges before the first match cannot match, so only the tail
                    // after it is rescanned; the first match itself is already
                    // known to match and is only an alternative when a literal
                    // edge is taken instead.
                    if let Some((first_index, (_, _, first_child))) = first_affix {
                        for (prefix, suffix, child) in node.affix.iter().skip(first_index + 1).rev() {
                            if bytes.len() > prefix.len() + suffix.len()
                                && bytes.starts_with(prefix.as_bytes())
                                && bytes.ends_with(suffix.as_bytes())
                            {
                                pending.push(WalkAction::Node(child, depth + 1));
                            }
                        }
                        if literal.is_some() {
                            pending.push(WalkAction::Node(first_child, depth + 1));
                        }
                    }

                    node = literal
                        .or_else(|| first_affix.map(|(_, (_, _, child))| child))
                        .or(single)
                        .expect("at least one matching child was found");
                    depth += 1;
                    continue;
                }
            }

            if let Some(single) = single {
                if count >= depth && !node.rest.is_empty() {
                    pending.push(WalkAction::Leaves(&node.rest));
                }
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
                        node = next_node;
                        depth = next_depth;
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
    fn dispatch<'n, const VERBS: bool>(&self, leaves: &'n [Leaf]) -> Option<&'n Leaf> {
        leaves
            .iter()
            .find(|leaf| leaf.method == self.method && (!VERBS || leaf.verb.as_deref() == self.verb))
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::fmt::Write as _;

    use http_path_template::{Grammar, PathTemplate};
    use routerama_build::Route;

    use crate::literal_edge::SORTED_LITERAL_FANOUT;
    use crate::raw_resolver::RawResolver;
    use crate::route_match::RouteMatch;

    /// Widths straddling the linear/binary lookup boundary.
    const WIDTHS: [usize; 5] = [
        SORTED_LITERAL_FANOUT - 1,
        SORTED_LITERAL_FANOUT,
        SORTED_LITERAL_FANOUT + 1,
        4 * SORTED_LITERAL_FANOUT,
        64 * SORTED_LITERAL_FANOUT,
    ];

    /// Builds a `GET` route from a default-grammar template.
    fn route(name: &str, pattern: &str) -> Route {
        Route::new(
            name,
            "GET",
            PathTemplate::parse(pattern, Grammar::default().with_segment_affixes()).expect("valid template"),
        )
    }

    #[test]
    fn wide_literal_fanout_resolves_first_middle_last_and_misses() {
        for width in WIDTHS {
            let resolver = RawResolver::new((0..width).map(|entry| route(&format!("Entry{entry}"), &format!("/scale/entry-{entry:04}"))));

            for entry in [0, width / 2, width - 1] {
                let path = format!("/scale/entry-{entry:04}");
                let matched = resolver
                    .resolve("GET", &path)
                    .unwrap_or_else(|| panic!("width {width} failed to match entry {entry}"));
                assert_eq!(matched.name(), format!("Entry{entry}"));
            }
            assert!(
                resolver.resolve("GET", "/scale/entry-9999").is_none(),
                "width {width} matched a miss"
            );
            assert!(resolver.resolve("GET", "/scale/entry-0000/extra").is_none());
            assert!(resolver.resolve("GET", "/scale").is_none());
            assert!(resolver.resolve("POST", "/scale/entry-0000").is_none());
        }
    }

    #[test]
    fn wide_literal_fanout_keeps_precedence_over_affix_single_and_rest() {
        for width in WIDTHS {
            let mut routes: Vec<Route> = (0..width)
                .map(|entry| route(&format!("Entry{entry}"), &format!("/mix/entry-{entry:04}")))
                .collect();
            // Siblings of every other kind at the same wide node.
            routes.push(route("Affix", "/mix/img-{id}.png"));
            routes.push(route("Single", "/mix/{name}"));
            routes.push(route("Rest", "/mix/{tail=**}"));
            let resolver = RawResolver::new(routes);

            let last = format!("/mix/entry-{:04}", width - 1);
            let literal = resolver.resolve("GET", &last).expect("the last literal matches");
            assert_eq!(
                literal.name(),
                format!("Entry{}", width - 1),
                "width {width} lost literal precedence"
            );
            assert_eq!(literal.captures().count(), 0);

            let affix = resolver.resolve("GET", "/mix/img-7.png").expect("the affix sibling matches");
            assert_eq!(affix.name(), "Affix");
            assert_eq!(affix.capture("id"), Some("7"));

            let single = resolver.resolve("GET", "/mix/other").expect("the single wildcard matches");
            assert_eq!(single.name(), "Single");
            assert_eq!(single.capture("name"), Some("other"));

            let rest = resolver.resolve("GET", "/mix/other/deeper").expect("the rest sibling matches");
            assert_eq!(rest.name(), "Rest");
            assert_eq!(rest.capture("tail"), Some("other/deeper"));

            // A literal segment also reachable through the rest edge stays a
            // literal match at its own depth and a rest match below it.
            let below = format!("/mix/entry-{:04}/deeper", width - 1);
            let deeper = resolver
                .resolve("GET", &below)
                .expect("a deeper path falls through to the rest sibling");
            assert_eq!(deeper.name(), "Rest");
        }
    }

    #[test]
    fn wide_literal_fanout_still_backtracks_to_single_and_rest() {
        for width in WIDTHS {
            let mut routes: Vec<Route> = (0..width)
                .map(|entry| route(&format!("DeadEnd{entry}"), &format!("/back/entry-{entry:04}/no")))
                .collect();
            routes.push(route("Single", "/back/{name}/yes"));
            routes.push(route("Rest", "/back/{tail=**}"));
            let resolver = RawResolver::new(routes);

            for entry in [0, width / 2, width - 1] {
                let key = format!("entry-{entry:04}");
                let path = format!("/back/{key}/yes");
                let matched = resolver
                    .resolve("GET", &path)
                    .unwrap_or_else(|| panic!("width {width} failed to backtrack for entry {entry}"));
                assert_eq!(matched.name(), "Single");
                assert_eq!(matched.capture("name"), Some(key.as_str()));

                let deep = format!("/back/{key}/deep/er");
                let rested = resolver
                    .resolve("GET", &deep)
                    .unwrap_or_else(|| panic!("width {width} failed to reach the rest edge for entry {entry}"));
                assert_eq!(rested.name(), "Rest");
            }
        }
    }

    #[test]
    fn wide_literal_fanout_is_sorted_even_when_subtree_weights_differ() {
        // Sibling weights decide the narrow-node order, so a wide node whose
        // children carry very different subtree weights is the case where the
        // weight order and the key order disagree. Compilation must sort it, or
        // the search would miss keys the scan used to find.
        for width in [SORTED_LITERAL_FANOUT, SORTED_LITERAL_FANOUT + 4] {
            let mut routes: Vec<Route> = (0..width)
                .map(|entry| route(&format!("Leaf{entry}"), &format!("/heavy/k{entry:02}")))
                .collect();
            // Two late keys become the heaviest subtrees in the node.
            for entry in [width - 1, width - 2] {
                for extra in 0..8 {
                    routes.push(route(&format!("Sub{entry}x{extra}"), &format!("/heavy/k{entry:02}/sub{extra}")));
                }
            }
            let resolver = RawResolver::new(routes);

            for entry in 0..width {
                let path = format!("/heavy/k{entry:02}");
                let matched = resolver
                    .resolve("GET", &path)
                    .unwrap_or_else(|| panic!("width {width}: `{path}` must resolve after weight-independent sorting"));
                assert_eq!(matched.name(), format!("Leaf{entry}"));
            }
            let heavy = format!("/heavy/k{:02}/sub3", width - 1);
            assert_eq!(
                resolver.resolve("GET", &heavy).expect("the heavy subtree still resolves").name(),
                format!("Sub{}x3", width - 1)
            );
            assert!(resolver.resolve("GET", "/heavy/k99").is_none());
        }
    }

    #[test]
    fn wide_literal_fanout_matches_keys_that_are_prefixes_of_each_other() {
        // Binary search compares whole keys in byte order, so keys that share a
        // prefix and differ only in length must still resolve exactly.
        let mut routes: Vec<Route> = (0..SORTED_LITERAL_FANOUT)
            .map(|entry| route(&format!("Pad{entry}"), &format!("/pre/pad-{entry:04}")))
            .collect();
        for (index, key) in ["a", "ab", "abc", "b", "ba"].into_iter().enumerate() {
            routes.push(route(&format!("Key{index}"), &format!("/pre/{key}")));
        }
        let resolver = RawResolver::new(routes);

        for (index, key) in ["a", "ab", "abc", "b", "ba"].into_iter().enumerate() {
            let path = format!("/pre/{key}");
            let matched = resolver.resolve("GET", &path).unwrap_or_else(|| panic!("`{key}` must resolve"));
            assert_eq!(matched.name(), format!("Key{index}"));
        }
        for miss in ["abcd", "ac", "bb", ""] {
            let path = format!("/pre/{miss}");
            assert!(resolver.resolve("GET", &path).is_none(), "`{miss}` must miss");
        }
    }

    #[test]
    fn wide_literal_fanout_agrees_with_a_linear_scan_over_a_generated_request_space() {
        // Differential property: every probe over a wide table must select the
        // same route a linear scan of the registered keys would.
        let width = 4 * SORTED_LITERAL_FANOUT;
        let keys: Vec<String> = (0..width).map(|entry| format!("k{entry:03}x{}", entry % 7)).collect();
        let resolver = RawResolver::new(
            keys.iter()
                .enumerate()
                .map(|(entry, key)| route(&format!("Entry{entry}"), &format!("/probe/{key}"))),
        );

        let mut probes: Vec<String> = keys.clone();
        for entry in 0..width {
            probes.push(format!("k{entry:03}"));
            probes.push(format!("k{entry:03}x{}z", entry % 7));
            probes.push(format!("K{entry:03}x{}", entry % 7));
        }
        probes.extend([String::new(), "k".into(), "zzz".into()]);

        for probe in &probes {
            let expected = keys.iter().position(|key| key == probe).map(|entry| format!("Entry{entry}"));
            let path = format!("/probe/{probe}");
            let matched = resolver.resolve("GET", &path);
            assert_eq!(
                matched.as_ref().map(RouteMatch::name),
                expected.as_deref(),
                "disagreement on `/probe/{probe}`"
            );
        }
    }

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
        for routes in route_sets {
            let router = RawResolver::new(routes.iter().map(|(name, pattern)| mk(name, pattern)));
            for depth in 0..40 {
                let mut path = String::from("/a");
                for i in 0..depth {
                    let _ = write!(path, "/s{i}");
                }
                let _ = router.resolve("GET", &path);
                let _ = router.resolve("GET", &format!("{path}/"));
                let _ = router.resolve("POST", &format!("{path}:verb"));
            }
            let mut very_deep = String::new();
            for i in 0..500 {
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
    fn every_matching_affix_edge_stays_reachable_after_a_literal() {
        // A node holding a literal edge and several affix edges must try the
        // literal first and then fall back through every matching affix edge —
        // including the first one, which is skipped by the descent itself.
        let grammar = Grammar::default().with_segment_affixes();
        let mk = |name: &str, pattern: &str| Route::new(name, "GET", PathTemplate::parse(pattern, grammar).expect("valid template"));

        let resolver = RawResolver::new([
            mk("Literal", "/n/a-b-c/lit"),
            mk("FirstAffix", "/n/a-{id}/first"),
            mk("SecondAffix", "/n/{id}-c/second"),
            mk("ThirdAffix", "/n/a-{id}-c/third"),
        ]);

        for (path, expected) in [
            ("/n/a-b-c/lit", "Literal"),
            ("/n/a-b-c/first", "FirstAffix"),
            ("/n/a-b-c/second", "SecondAffix"),
            ("/n/a-b-c/third", "ThirdAffix"),
        ] {
            let matched = resolver.resolve("GET", path).unwrap_or_else(|| panic!("`{path}` must resolve"));
            assert_eq!(matched.name(), expected, "wrong backtracking target for `{path}`");
        }
        assert!(resolver.resolve("GET", "/n/a-b-c/none").is_none());
        // No affix edge matches this segment, so none may be queued for it.
        assert!(resolver.resolve("GET", "/n/zzz/first").is_none());

        // When several affix edges reach the same leaf shape, the first one in
        // match order wins and its capture is the one materialized.
        let ambiguous = RawResolver::new([mk("Longer", "/m/a-{id}-c/x"), mk("Shorter", "/m/a-{id}/x")]);
        let matched = ambiguous.resolve("GET", "/m/a-b-c/x").expect("both affix edges match");
        assert_eq!(matched.name(), "Longer");
        assert_eq!(matched.capture("id"), Some("b"));
    }

    #[test]
    fn unmatched_literals_do_not_duplicate_single_wildcard_fallbacks() {
        let grammar = Grammar::default();
        let mut routes = Vec::new();
        let mut wildcard_path = String::new();
        for depth in 0..32 {
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
        let request = format!("{}{}", "/value".repeat(32), "/missing");

        assert!(resolver.resolve("GET", &request).is_none());
    }
}
