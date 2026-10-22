// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock benchmarks for affix-edge trie matching at varying sibling counts.
//!
//! Paired with `routerama_affix_match_cg.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "resolved benchmark variants are consumed through black_box")]

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

include!("common/affix_match_scenarios.rs");

fn affix_match(c: &mut Criterion) {
    assert_equivalent();

    let mut hit = c.benchmark_group("routerama_affix_match/hit");
    for width in [2, 8, 32] {
        let scenario = build_affix_hit(width);
        hit.bench_with_input(BenchmarkId::from_parameter(width), &scenario, |b, scenario| {
            b.iter(|| affix_hit_lookup(scenario));
        });
    }
    hit.finish();

    let mut miss = c.benchmark_group("routerama_affix_match/miss");
    for width in [2, 8, 32] {
        let router = build_affix_router(width);
        miss.bench_with_input(BenchmarkId::from_parameter(width), &router, |b, router| {
            b.iter(|| affix_miss_lookup(router));
        });
    }
    miss.finish();
}

criterion_group!(benches, affix_match);
criterion_main!(benches);
