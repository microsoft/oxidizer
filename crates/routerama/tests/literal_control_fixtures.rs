// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Equivalence contracts for literal-only generated-router controls.

#![cfg(not(miri))]
#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]

use alloc_tracker::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("../benches/common/literal_control_scenarios.rs");

#[test]
fn every_literal_topology_selects_first_middle_last_and_miss() {
    assert_equivalent();
}

#[test]
fn literal_control_allocation_diagnostics() {
    for size in RouteSetSize::ALL {
        let routers = prepare(size);
        for shape in Shape::ALL {
            for scenario in Scenario::ALL {
                let session = alloc_tracker::Session::new().no_stdout().no_file();
                let operation = session.operation("resolve");
                {
                    let _span = operation.measure_thread().iterations(1);
                    std::hint::black_box(run_prepared(&routers, shape, scenario));
                }
                let report = session.to_report();
                let (_, operation) = report
                    .operations()
                    .find(|(name, _)| *name == "resolve")
                    .expect("the literal resolve allocation operation is recorded");
                let allocations = operation.total_allocations_count();
                let bytes = operation.total_bytes_allocated();
                eprintln!(
                    "{}/{}/{}: {allocations} allocations, {bytes} bytes",
                    size.name(),
                    shape.name(),
                    scenario.name()
                );
                assert_eq!(
                    allocations,
                    u64::from(shape == Shape::DeepChain),
                    "{}/{}/{} changed the baseline allocation boundary",
                    size.name(),
                    shape.name(),
                    scenario.name()
                );
                assert_eq!(
                    bytes,
                    if shape == Shape::DeepChain { 288 } else { 0 },
                    "{}/{}/{} changed the baseline allocation size",
                    size.name(),
                    shape.name(),
                    scenario.name()
                );
            }
        }
    }
}
