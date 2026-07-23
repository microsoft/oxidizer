// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static route-generator scaling benchmarks.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use http_path_template::{Grammar, PathTemplate};
use routerama_build::{Generator, Route};

#[derive(Clone, Copy)]
enum RouteShape {
    Literal,
    Capture,
    Affix,
}

impl RouteShape {
    const fn group(self) -> &'static str {
        match self {
            Self::Literal => "literals",
            Self::Capture => "captures",
            Self::Affix => "affixes",
        }
    }

    fn path(self, index: usize) -> String {
        match self {
            Self::Literal => format!("/generated/group-{}/{item}", index / 32, item = index % 32),
            Self::Capture => format!("/generated/group-{index}/items/{{item}}"),
            Self::Affix => format!("/generated/group-{index}/item-{{item}}.json"),
        }
    }
}

fn generator(route_count: usize, shape: RouteShape) -> Generator {
    let mut generator = Generator::new("GeneratedRoute", false);
    generator.full_api(false);
    generator.add_all((0..route_count).map(|index| {
        let path = shape.path(index);
        Route::new(
            format!("Route{index}"),
            "GET",
            PathTemplate::parse(&path, Grammar::default().with_segment_affixes()).expect("generated benchmark route is valid"),
        )
    }));
    generator
}

fn scaling(c: &mut Criterion) {
    for shape in [RouteShape::Literal, RouteShape::Capture, RouteShape::Affix] {
        let mut group = c.benchmark_group(format!("generator_scaling/{}", shape.group()));
        group.sample_size(10);
        group.measurement_time(Duration::from_secs(3));
        for route_count in [50, 500, 5_000] {
            let generator = generator(route_count, shape);
            group.bench_with_input(BenchmarkId::from_parameter(route_count), &generator, |b, generator| {
                b.iter(|| black_box(generator.generate()));
            });
        }
        group.finish();
    }
}

criterion_group!(benches, scaling);
criterion_main!(benches);
