// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared, network-free affix-matching fixtures. Each router registers `width`
// sibling routes at the same parent segment, differing only in an affix
// (a literal prefix and suffix around one capture), e.g.
// `/asset-00-{id}.png` .. `/asset-NN-{id}.png`. Router construction happens
// outside measured calls; only the trie walk for one lookup is measured.

use std::hint::black_box;

use http_path_template::{Grammar, PathTemplate};
use routerama::resolve::__private::{RawResolver, RouteMatch as _, Route};

// No registered width in this file reaches two digits' worth of siblings, so
// a fixed two-digit numeral outside every registered range is a miss against
// every affix candidate: none of their literal prefixes can match it.
const MISS_PATH: &str = "/asset-99-cat.png";

fn affix_route(index: usize) -> Route {
    Route::new(
        format!("Asset{index:02}"),
        "GET",
        PathTemplate::parse(&format!("/asset-{index:02}-{{id}}.png"), Grammar::default().with_segment_affixes())
            .expect("benchmark affix route is valid"),
    )
}

fn build_affix_router(width: usize) -> RawResolver {
    RawResolver::new((0..width).map(affix_route))
}

fn hit_path(width: usize) -> String {
    format!("/asset-{:02}-cat.png", width - 1)
}

fn build_affix_hit(width: usize) -> (RawResolver, String) {
    (build_affix_router(width), hit_path(width))
}

fn affix_hit_lookup(scenario: &(RawResolver, String)) {
    black_box(scenario.0.resolve("GET", black_box(&scenario.1)));
}

fn affix_miss_lookup(router: &RawResolver) {
    black_box(router.resolve("GET", black_box(MISS_PATH)));
}

fn assert_equivalent() {
    for width in [2, 8, 32] {
        let (router, path) = build_affix_hit(width);
        let matched = router
            .resolve("GET", &path)
            .expect("the last affix sibling's own path always matches, by construction of hit_path");
        assert_eq!(matched.name(), format!("Asset{:02}", width - 1));
        assert_eq!(matched.capture("id"), Some("cat"));

        let miss_router = build_affix_router(width);
        assert!(
            miss_router.resolve("GET", MISS_PATH).is_none(),
            "width {width}: no affix sibling may match a path outside every registered prefix"
        );
    }
}
