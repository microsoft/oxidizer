// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Equivalence and allocation diagnostics for encoded primitive fixtures.

#![cfg(not(miri))]
#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]

use alloc_tracker::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("../benches/common/primitive_decode_scenarios.rs");

#[derive(routerama::query::FromQuery)]
struct OptionalPrimitive {
    value: Option<i32>,
}

#[derive(routerama::query::FromQuery)]
struct RepeatedPrimitive {
    value: Vec<u32>,
}

#[derive(routerama::query::FromQuery)]
struct CowControl<'q> {
    value: std::borrow::Cow<'q, str>,
}

fn measured_allocations(run: impl FnOnce()) -> u64 {
    let session = alloc_tracker::Session::new().no_stdout().no_file();
    let operation = session.operation("decode");
    {
        let _span = operation.measure_thread().iterations(1);
        run();
    }
    let report = session.to_report();
    let (_, operation) = report
        .operations()
        .find(|(name, _)| *name == "decode")
        .expect("the decode allocation operation is recorded");
    operation.total_allocations_count()
}

#[test]
fn path_and_query_decoding_preserve_values_and_error_categories() {
    assert_equivalent();
}

#[test]
fn generated_primitive_decoders_avoid_transient_allocations() {
    let diagnostics = allocation_diagnostics();
    for source in Source::ALL {
        for scenario in Scenario::ALL {
            let scenario_index = Scenario::ALL
                .iter()
                .position(|candidate| *candidate == scenario)
                .expect("every checked scenario is in Scenario::ALL");
            let source_index = Source::ALL
                .iter()
                .position(|candidate| *candidate == source)
                .expect("every checked source is in Source::ALL");
            let stats = diagnostics[source_index][scenario_index];
            let generic = scenario.is_control() || matches!(scenario, Scenario::GenericFromStr | Scenario::GenericUnescaped);
            let expected = u64::from(generic && !scenario.is_unescaped());
            assert_eq!(
                stats.allocations,
                expected,
                "{}/{} should allocate only for an encoded custom FromStr control",
                source.name(),
                scenario.name()
            );
        }
    }
}

#[test]
fn optional_and_repeated_primitives_allocate_only_for_output_storage() {
    assert_eq!(
        measured_allocations(|| {
            std::hint::black_box(OptionalPrimitive::from_query("value=%2D%34%32").expect("encoded optional parses"));
        }),
        0
    );
    assert_eq!(
        measured_allocations(|| {
            std::hint::black_box(RepeatedPrimitive::from_query("value=%31&value=%32").expect("encoded repeated values parse"));
        }),
        1,
        "the repeated Vec is the only allocation"
    );
}

#[test]
fn generic_and_string_controls_keep_their_materialized_decode() {
    assert_eq!(
        measured_allocations(|| {
            std::hint::black_box(GenericQuery::from_query("value=%34%32").expect("encoded custom value parses"));
        }),
        1
    );
    assert_eq!(
        measured_allocations(|| {
            std::hint::black_box(CowControl::from_query("value=%34%32").expect("encoded Cow parses"));
        }),
        1
    );
}
