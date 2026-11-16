// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests heap configuration and identities through the public API.

use allocation_hints::heaps::{Heap, Kind, bump, current_thread_id, general};
use allocation_hints::{active_hint, with_hint};

#[test]
fn general_options_round_trip() {
    let options = general::Options::default()
        .with_locality_segment_bytes(128 * 1024)
        .with_medium_cache_max_bytes(256 * 1024);

    assert_eq!(
        (options.locality_segment_bytes(), options.medium_cache_max_bytes()),
        (128 * 1024, 256 * 1024)
    );
}

#[test]
fn option_defaults_have_exact_documented_values() {
    let general = general::Options::new();
    let bump = bump::Options::new();

    assert_eq!(
        (
            general.locality_segment_bytes(),
            general.medium_cache_max_bytes(),
            bump.max_allocation_bytes(),
            bump.max_alignment(),
            bump.retained_chunks(),
            bump.max_retained_chunks(),
        ),
        (4 * 1024 * 1024, 8 * 1024 * 1024, 32 * 1024, 4 * 1024, 4, 16)
    );
}

#[test]
fn general_option_boundaries_accept_the_exact_documented_maxima() {
    let options = general::Options::new()
        .with_locality_segment_bytes(0x4000_0000)
        .with_medium_cache_max_bytes(0x0080_0000);

    assert_eq!(
        (options.locality_segment_bytes(), options.medium_cache_max_bytes()),
        (0x4000_0000, 0x0080_0000)
    );
    std::panic::catch_unwind(|| general::Options::new().with_locality_segment_bytes(0x8000_0000)).unwrap_err();
    std::panic::catch_unwind(|| general::Options::new().with_medium_cache_max_bytes(0x0100_0000)).unwrap_err();
}

#[test]
fn general_option_boundaries_accept_the_exact_documented_minima() {
    let options = general::Options::new()
        .with_locality_segment_bytes(0x0001_0000)
        .with_medium_cache_max_bytes(0x0001_0000);

    assert_eq!(
        (options.locality_segment_bytes(), options.medium_cache_max_bytes()),
        (0x0001_0000, 0x0001_0000)
    );
    std::panic::catch_unwind(|| general::Options::new().with_locality_segment_bytes(0x0000_8000)).unwrap_err();
    std::panic::catch_unwind(|| general::Options::new().with_medium_cache_max_bytes(0x0000_8000)).unwrap_err();
}

#[test]
fn general_option_boundaries_reject_non_powers_of_two_and_accept_disabled_cache() {
    let options = general::Options::new().with_medium_cache_max_bytes(0);

    assert_eq!(options.medium_cache_max_bytes(), 0);
    std::panic::catch_unwind(|| general::Options::new().with_locality_segment_bytes(3 * 64 * 1024)).unwrap_err();
    std::panic::catch_unwind(|| general::Options::new().with_medium_cache_max_bytes(3 * 64 * 1024)).unwrap_err();
}

#[test]
fn bump_options_round_trip() {
    let options = bump::Options::default()
        .with_max_allocation_bytes(1024)
        .with_max_alignment(128)
        .with_retained_chunks(2)
        .with_max_retained_chunks(8);

    assert_eq!(
        (
            options.max_allocation_bytes(),
            options.max_alignment(),
            options.retained_chunks(),
            options.max_retained_chunks(),
        ),
        (1024, 128, 2, 8)
    );
}

#[test]
fn bump_option_boundaries_are_exact() {
    let minimum = bump::Options::new()
        .with_max_allocation_bytes(1)
        .with_max_alignment(1)
        .with_retained_chunks(1);
    let maximum = bump::Options::new()
        .with_max_allocation_bytes(32 * 1024)
        .with_max_alignment(32 * 1024)
        .with_max_retained_chunks(usize::MAX);

    assert_eq!(
        (
            minimum.max_allocation_bytes(),
            minimum.max_alignment(),
            minimum.retained_chunks(),
            minimum.max_retained_chunks(),
            maximum.max_allocation_bytes(),
            maximum.max_alignment(),
            maximum.max_retained_chunks(),
        ),
        (1, 1, 1, 1, 32 * 1024, 32 * 1024, usize::MAX)
    );
}

#[test]
fn bump_option_boundaries_reject_values_outside_the_contract() {
    std::panic::catch_unwind(|| bump::Options::new().with_max_allocation_bytes(0)).unwrap_err();
    std::panic::catch_unwind(|| bump::Options::new().with_max_allocation_bytes(32 * 1024 + 1)).unwrap_err();
    std::panic::catch_unwind(|| bump::Options::new().with_max_alignment(0)).unwrap_err();
    std::panic::catch_unwind(|| bump::Options::new().with_max_alignment(3)).unwrap_err();
    std::panic::catch_unwind(|| bump::Options::new().with_max_alignment(64 * 1024)).unwrap_err();
    std::panic::catch_unwind(|| bump::Options::new().with_retained_chunks(0)).unwrap_err();
    std::panic::catch_unwind(|| bump::Options::new().with_retained_chunks(2).with_max_retained_chunks(1)).unwrap_err();
}

#[test]
fn heap_accessors_and_debug_report_descriptor() {
    let options = general::Options::new();
    let heap = Heap::general(options);
    let active = with_hint(&heap, || active_hint().unwrap());

    assert_eq!(
        (heap.id().get(), heap.kind(), active.id(), active.kind(), format!("{heap:?}")),
        (
            active.id().get(),
            Kind::General(options),
            heap.id(),
            Kind::General(options),
            format!("Heap {{ id: {:?}, kind: General({options:?}) }}", heap.id()),
        )
    );
}

#[test]
fn heap_constructors_preserve_kinds_and_allocate_distinct_ids() {
    let bump_options = bump::Options::new().with_retained_chunks(3);
    let bump = Heap::bump(bump_options);
    let general = Heap::new();

    assert_eq!(bump.kind(), Kind::Bump(bump_options));
    assert!(matches!(general.kind(), Kind::General(_)));
    assert_ne!(bump.id(), general.id());
    assert_ne!(bump.id().get(), 0);
    assert_ne!(general.id().get(), 0);
}

#[test]
fn default_heap_is_general_purpose() {
    assert!(matches!(Heap::default().kind(), Kind::General(_)));
}

#[test]
fn thread_identity_is_nonzero() {
    assert_ne!(current_thread_id().get(), 0);
}

#[test]
fn thread_identities_are_stable_per_thread_and_distinct_between_threads() {
    let current = current_thread_id();
    let other = std::thread::spawn(|| (current_thread_id(), current_thread_id())).join().unwrap();

    assert_eq!(other.0, other.1);
    assert_ne!(current, other.0);
}
