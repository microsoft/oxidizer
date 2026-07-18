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

impl<'p> Walk<'_, 'p> {
    /// Returns the first matching leaf without recursive trie descent.
    // Static/dynamic differential and structured-path tests exercise traversal;
    // many local rewrites are equivalent because pending edges backtrack.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn descend_iterative<const VERBS: bool>(&self, mut node: &'p RtNode, mut depth: usize) -> Option<&'p Leaf> {
        let mut pending: SmallVec<[WalkAction<'p>; 4]> = SmallVec::new();
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
                let literal = node.literals.iter().find(|(key, _)| {
                    let key = key.as_bytes();
                    key.len() == bytes.len() && key.first() == bytes.first() && key == bytes
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

                if literal.is_some() || first_affix.is_some() || single.is_some() {
                    if count >= depth && !node.rest.is_empty() {
                        pending.push(WalkAction::Leaves(&node.rest));
                    }
                    if (literal.is_some() || first_affix.is_some())
                        && let Some(single) = single
                    {
                        pending.push(WalkAction::Node(single, depth + 1));
                    }
                    let skip = first_affix.map_or(0, |(index, _)| usize::from(literal.is_none()) + index);
                    for (prefix, suffix, child) in node.affix.iter().skip(skip).rev() {
                        if bytes.len() > prefix.len() + suffix.len()
                            && bytes.starts_with(prefix.as_bytes())
                            && bytes.ends_with(suffix.as_bytes())
                        {
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
    fn dispatch<const VERBS: bool>(&self, leaves: &'p [Leaf]) -> Option<&'p Leaf> {
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
