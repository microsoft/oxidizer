// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared route-table scenarios measuring `RawResolver::new` and
// `RawResolver::new_checked` (the conflict check) at increasing route-table
// sizes, for both a purely-literal table and one dominated by captures and
// affixes. Fixture route-table construction is excluded from the measured
// region; only the resolver-build call itself is timed.

use routerama::resolve::__private::{RawResolver, RouteMatch as _, Route};
use http_path_template::{Grammar, PathTemplate};

fn route(name: String, path: &str) -> Route {
    Route::new(
        name,
        "GET",
        PathTemplate::parse(path, Grammar::default().with_segment_affixes()).expect("benchmark route template is valid"),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// Every route is a distinct fixed-depth literal path; no captures.
    Literal,
    /// Every route shares one parent segment and differs only in the affix
    /// wrapped around a capture, so the conflict check must compare every
    /// pair of affix siblings at that node.
    Captures,
}

impl Shape {
    const ALL: [Self; 2] = [Self::Literal, Self::Captures];

    const fn name(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Captures => "captures",
        }
    }
}

/// Route-table sizes exercised by both harnesses. `1024` is the largest size
/// kept tractable for Callgrind for the `Captures` shape, whose conflict check
/// compares every pair of affix siblings under the shared parent node; the
/// Callgrind file benchmarks only a subset of these sizes for that shape (see
/// its module doc comment).
const SIZES: [usize; 3] = [16, 128, 1024];

fn build_routes(shape: Shape, size: usize) -> Vec<Route> {
    (0..size)
        .map(|index| match shape {
            Shape::Literal => route(format!("Lit{index:04}"), &format!("/lit/segment{index:04}/detail")),
            Shape::Captures => route(
                format!("Cap{index:04}"),
                &format!("/assets/pre{index:04}-{{value}}.suffix{index:04}"),
            ),
        })
        .collect()
}

fn sample_lookup(shape: Shape, size: usize) -> (String, String) {
    let middle = size / 2;
    match shape {
        Shape::Literal => (format!("Lit{middle:04}"), format!("/lit/segment{middle:04}/detail")),
        Shape::Captures => (
            format!("Cap{middle:04}"),
            format!("/assets/pre{middle:04}-anything.suffix{middle:04}"),
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Scenario {
    shape: Shape,
    size: usize,
}

impl Scenario {
    const ALL: [Self; SIZES.len() * Shape::ALL.len()] = [
        Self { shape: Shape::Literal, size: SIZES[0] },
        Self { shape: Shape::Literal, size: SIZES[1] },
        Self { shape: Shape::Literal, size: SIZES[2] },
        Self { shape: Shape::Captures, size: SIZES[0] },
        Self { shape: Shape::Captures, size: SIZES[1] },
        Self { shape: Shape::Captures, size: SIZES[2] },
    ];

    fn name(self) -> String {
        format!("{}_{:04}", self.shape.name(), self.size)
    }
}

struct PreparedScenario {
    routes: Vec<Route>,
    expected_name: String,
    lookup_path: String,
}

fn prepare(scenario: Scenario) -> PreparedScenario {
    let (expected_name, lookup_path) = sample_lookup(scenario.shape, scenario.size);
    PreparedScenario {
        routes: build_routes(scenario.shape, scenario.size),
        expected_name,
        lookup_path,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Observation {
    route_count: usize,
    max_segments: usize,
    splits_verbs: bool,
    conflicts: usize,
    matched_name: Option<String>,
}

fn run_new_prepared(prepared: PreparedScenario) -> Observation {
    let route_count = prepared.routes.len();
    let resolver = RawResolver::new(prepared.routes);
    let matched_name = resolver.resolve("GET", &prepared.lookup_path).map(|matched| matched.name().to_owned());
    Observation {
        route_count,
        max_segments: resolver.max_segments(),
        splits_verbs: resolver.splits_verbs(),
        conflicts: 0,
        matched_name,
    }
}

fn run_new_checked_prepared(prepared: PreparedScenario) -> Observation {
    let route_count = prepared.routes.len();
    let (resolver, conflicts) = RawResolver::new_checked(prepared.routes);
    let matched_name = resolver.resolve("GET", &prepared.lookup_path).map(|matched| matched.name().to_owned());
    Observation {
        route_count,
        max_segments: resolver.max_segments(),
        splits_verbs: resolver.splits_verbs(),
        conflicts: conflicts.len(),
        matched_name,
    }
}

fn assert_equivalent() {
    for scenario in Scenario::ALL {
        let expected_name = sample_lookup(scenario.shape, scenario.size).0;

        let new_observation = run_new_prepared(prepare(scenario));
        assert_eq!(
            new_observation.matched_name,
            Some(expected_name.clone()),
            "RawResolver::new/{} must resolve the sample lookup to the expected route",
            scenario.name()
        );
        assert_eq!(new_observation.route_count, scenario.size, "{}", scenario.name());
        assert!(!new_observation.splits_verbs, "{}", scenario.name());

        let checked_observation = run_new_checked_prepared(prepare(scenario));
        assert_eq!(
            checked_observation.matched_name,
            Some(expected_name),
            "RawResolver::new_checked/{} must resolve the sample lookup to the expected route",
            scenario.name()
        );
        assert_eq!(
            checked_observation.conflicts, 0,
            "RawResolver::new_checked/{} must build a conflict-free table",
            scenario.name()
        );
        assert_eq!(
            new_observation.max_segments, checked_observation.max_segments,
            "new and new_checked must compile identical trees for {}",
            scenario.name()
        );
    }
}
