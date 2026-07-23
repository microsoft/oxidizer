// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared mixed static/dynamic resolver scenarios for Criterion and Callgrind.

use std::hint::black_box;

use routerama::HttpMethod;

#[::routerama::resolver]
#[derive(Debug)]
enum MixedScenario {
    #[route(GET, "/health")]
    Health,
    #[route(GET, "/numbers/{value}")]
    Number { value: u32 },
    Plugin {
        name: String,
    },
    NumberFallback {
        value: String,
    },
}

fn build_mixed_scenario() -> MixedScenarioResolver {
    MixedScenario::builder()
        .add_plugin(HttpMethod::GET, "/plugins/{name}")
        .add_number_fallback(HttpMethod::GET, "/numbers/{value}")
        .build()
        .expect("mixed scenario builds")
}

#[inline]
fn mixed_static_hit(router: &MixedScenarioResolver) {
    let _ = black_box(router.resolve("GET", black_box("/health")));
}

#[inline]
fn mixed_dynamic_hit(router: &MixedScenarioResolver) {
    let _ = black_box(router.resolve("GET", black_box("/plugins/auth")));
}

#[inline]
fn mixed_complete_miss(router: &MixedScenarioResolver) {
    let _ = black_box(router.resolve("GET", black_box("/missing")));
}

#[inline]
fn mixed_static_capture_error(router: &MixedScenarioResolver) {
    let _ = black_box(router.resolve("GET", black_box("/numbers/not-a-number")));
}
