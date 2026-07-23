// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock, allocation, and Hyper HTTP/1 evidence for response templates.
//!
//! Paired with `routerama_response_templates_cg.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]

use std::io::Write as _;

use alloc_tracker::Allocator;
use criterion::{Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("common/response_template_scenarios.rs");

#[expect(
    clippy::too_many_lines,
    reason = "one entry keeps allocation diagnostics and paired response-template groups together"
)]
fn response_templates(c: &mut Criterion) {
    assert_equivalent();
    let mut stderr = std::io::stderr().lock();
    for (representation, diagnostics) in Representation::ALL.into_iter().zip(body_allocation_diagnostics()) {
        for (scenario, stats) in diagnostics {
            writeln!(
                stderr,
                "response-template allocations/in_memory/{}/{}: measured={} allocations/{} bytes",
                representation.name(),
                scenario.name(),
                stats.allocations,
                stats.bytes
            )
            .expect("writing response-template diagnostics to stderr should succeed");
        }
    }
    for (representation, diagnostics) in Representation::ALL.into_iter().zip(transport_allocation_diagnostics()) {
        for (scenario, stats) in diagnostics {
            writeln!(
                stderr,
                "response-template allocations/hyper_http1/{}/{}: measured={} allocations/{} bytes",
                representation.name(),
                scenario.name(),
                stats.allocations,
                stats.bytes
            )
            .expect("writing response-template transport diagnostics to stderr should succeed");
        }
    }
    for representation in Representation::ALL {
        for scenario in BodyScenario::ALL {
            let observation = run_transport(representation, scenario);
            writeln!(
                stderr,
                concat!(
                    "response-template transport/{}/{}: body={} bytes, future={} bytes, ",
                    "connection_polls={}, body_polls={}, frames={}, size_hints={}, ",
                    "writes={} (vectored={}), iovecs={}, static_direct={}, static_copied={}"
                ),
                representation.name(),
                scenario.name(),
                observation.body_size,
                observation.future_size,
                observation.connection_polls,
                observation.body_polls,
                observation.body_frames,
                observation.size_hint_calls,
                observation.write_calls,
                observation.vectored_write_calls,
                observation.io_slices,
                observation.direct_static_bytes,
                observation.copied_static_bytes
            )
            .expect("writing response-template transport diagnostics to stderr should succeed");
        }
    }
    for (scenario, stats) in head_allocation_diagnostics() {
        writeln!(
            stderr,
            "response-template allocations/response_head/{}: measured={} allocations/{} bytes",
            scenario.name(),
            stats.allocations,
            stats.bytes
        )
        .expect("writing response-head diagnostics to stderr should succeed");
    }
    for (representation, diagnostics) in HeadRepresentation::ALL.into_iter().zip(head_candidate_allocation_diagnostics()) {
        for (negotiated, diagnostics) in [false, true].into_iter().zip(diagnostics) {
            if representation == HeadRepresentation::Ordinary && !negotiated {
                continue;
            }
            for (scenario, stats) in diagnostics {
                writeln!(
                    stderr,
                    "response-template allocations/response_head/{}/{}/{}: measured={} allocations/{} bytes",
                    representation.name(),
                    if negotiated { "negotiated" } else { "plain" },
                    scenario.name(),
                    stats.allocations,
                    stats.bytes
                )
                .expect("writing response-head candidate diagnostics to stderr should succeed");
            }
        }
    }

    let mut bodies = c.benchmark_group("routerama_response_templates/in_memory");
    for representation in Representation::ALL {
        for scenario in BodyScenario::ALL {
            bodies.bench_function(format!("{}/{}", representation.name(), scenario.name()), |b| {
                b.iter(|| std::hint::black_box(run_body(representation, scenario)));
            });
        }
    }
    bodies.finish();

    let mut transport = c.benchmark_group("routerama_response_templates/hyper_http1");
    for representation in Representation::ALL {
        for scenario in BodyScenario::ALL {
            transport.bench_function(format!("{}/{}", representation.name(), scenario.name()), |b| {
                b.iter(|| std::hint::black_box(run_transport(representation, scenario)));
            });
        }
    }
    transport.finish();

    let mut heads = c.benchmark_group("routerama_response_templates/response_head");
    for scenario in HeadScenario::ALL {
        heads.bench_function(scenario.name(), |b| {
            b.iter(|| std::hint::black_box(run_head(scenario)));
        });
    }
    for representation in [
        HeadRepresentation::Reserved,
        HeadRepresentation::StaticPlan,
        HeadRepresentation::GeneratedPlan,
    ] {
        for scenario in HeadScenario::ALL {
            heads.bench_function(format!("{}/{}", representation.name(), scenario.name()), |b| {
                b.iter(|| std::hint::black_box(run_head_with(representation, scenario, false)));
            });
        }
    }
    for representation in HeadRepresentation::ALL {
        for scenario in HeadScenario::ALL {
            heads.bench_function(format!("{}_negotiated/{}", representation.name(), scenario.name()), |b| {
                b.iter(|| std::hint::black_box(run_head_with(representation, scenario, true)));
            });
        }
    }
    heads.finish();
}

criterion_group!(benches, response_templates);
criterion_main!(benches);
