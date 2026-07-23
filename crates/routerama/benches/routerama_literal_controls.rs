// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock controls for literal-only generated route topologies.
//!
//! Paired with `routerama_literal_controls_cg.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]

use criterion::{Criterion, criterion_group, criterion_main};

include!("common/literal_control_scenarios.rs");

fn literal_controls(c: &mut Criterion) {
    assert_equivalent();
    for size in RouteSetSize::ALL {
        let routers = prepare(size);
        for shape in Shape::ALL {
            let mut group = c.benchmark_group(format!("routerama_literal_controls/{}/{}", size.name(), shape.name()));
            for scenario in Scenario::ALL {
                group.bench_function(scenario.name(), |b| {
                    b.iter(|| std::hint::black_box(run_prepared(&routers, shape, scenario)));
                });
            }
            group.finish();
        }
    }
}

criterion_group!(benches, literal_controls);
criterion_main!(benches);
