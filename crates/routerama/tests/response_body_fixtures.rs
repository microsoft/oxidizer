// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Equivalence, allocation, size, and transport checks for the response-body
//! evidence fixture.

#![cfg(not(miri))]
#![allow(dead_code, reason = "the shared fixture also supports both benchmark harnesses")]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]
#![expect(clippy::panic, reason = "a pending or malformed in-memory fixture is a test invariant violation")]

use alloc_tracker::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("../benches/common/response_body_scenarios.rs");

#[test]
fn every_representation_preserves_frames_trailers_hints_and_errors() {
    assert_equivalent();
}

#[test]
fn allocation_boundaries_are_directly_checkable() {
    let diagnostics = allocation_diagnostics();
    let measured_counts = diagnostics.map(|diagnostic| diagnostic.measured.allocations);

    assert_eq!(
        measured_counts,
        [2, 3, 3, 0, 1, 1, 2, 2, 2, 0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 2],
        "static response payloads allocate nothing, text responses allocate only their header map, and only BoxBody construction and boxed error conversion otherwise allocate: {diagnostics:#?}"
    );

    let header_only = diagnostics[0].measured;
    assert_eq!(
        [diagnostics[6].measured, diagnostics[7].measured, diagnostics[8].measured],
        [header_only; 3],
        "StaticText must add no payload-sized allocation beyond the existing text header map"
    );
    assert_eq!(
        [diagnostics[9].measured, diagnostics[10].measured, diagnostics[11].measured],
        [AllocationStats { allocations: 0, bytes: 0 }; 3],
        "StaticBytes must allocate neither response metadata nor payload storage"
    );
    for (copied, zero_copy, payload_length) in [
        (1, 7, STATIC_TEXT_SMALL.len() as u64),
        (2, 8, STATIC_TEXT_MEDIUM.len() as u64),
        (4, 10, STATIC_BYTES_SMALL.len() as u64),
        (5, 11, STATIC_BYTES_MEDIUM.len() as u64),
    ] {
        assert_eq!(
            diagnostics[copied].measured.allocations,
            diagnostics[zero_copy].measured.allocations + 1
        );
        assert_eq!(
            diagnostics[copied].measured.bytes,
            diagnostics[zero_copy].measured.bytes + payload_length
        );
    }
    assert!(
        diagnostics[6..12]
            .iter()
            .all(|diagnostic| diagnostic.setup.allocations == 0 && diagnostic.setup.bytes == 0),
        "static response inputs must be prepared without allocator activity"
    );

    assert!(
        diagnostics[13].setup.allocations > 0,
        "the concrete stream's trailer-map setup allocation must stay outside observation"
    );
    assert!(
        diagnostics[16].setup.allocations > 0,
        "generated stream preparation must retain the same out-of-region trailer-map allocation"
    );
}

#[test]
fn concrete_fallible_part_success_and_rejection_paths_do_not_allocate() {
    let diagnostics = response_part_allocation_diagnostics();

    assert_eq!(
        diagnostics.map(|diagnostic| diagnostic.allocations),
        [0, 0],
        "typed part composition must not introduce a body box or another allocation: {diagnostics:#?}"
    );
    assert_eq!(
        diagnostics.map(|diagnostic| diagnostic.bytes),
        [0, 0],
        "allocation-free part composition must allocate zero bytes: {diagnostics:#?}"
    );
}

#[test]
fn batched_failures_keep_per_instance_drop_evidence() {
    let prepared = (0..64)
        .flat_map(|_| [prepare(Scenario::GeneratedConcreteError), prepare(Scenario::BoxedError)])
        .collect::<Vec<_>>();

    for prepared in prepared.into_iter().rev() {
        assert!(matches!(run_prepared(prepared), ScenarioObservation::Failure(_)));
    }
}

#[test]
fn runtime_sizes_cover_named_and_opaque_representations() {
    let sizes = size_diagnostics();
    eprintln!(
        "host-specific response-body sizes ({}-bit {}-{}): {sizes:#?}",
        usize::BITS,
        std::env::consts::ARCH,
        std::env::consts::OS
    );

    assert!(sizes.body > 0);
    assert!(sizes.concrete_stream >= sizes.body);
    assert!(sizes.either_body >= sizes.concrete_stream);
    assert!(sizes.box_body > 0);
    assert!(sizes.fixed_service_future > 0);
    assert!(sizes.fixed_service_response >= sizes.fixed_service_opaque_body);
    assert!(sizes.multiple_service_future > 0);
    assert!(sizes.multiple_service_opaque_body >= sizes.fixed_service_opaque_body);
    assert!(sizes.generated_body_error_sum > 0);
}

#[test]
fn generated_response_supports_send_adapters_without_restricting_the_core() {
    assert_transport_compatibility();
}
