// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::RemoteFreeAvailability;

#[test]
fn producer_witness_is_thread_bound_without_extra_storage() {
    // Inference becomes ambiguous if the witness ever implements Send.
    trait AmbiguousIfSend<Marker> {
        fn proof() {}
    }
    impl<T: ?Sized> AmbiguousIfSend<()> for T {}
    impl<T: ?Sized + Send> AmbiguousIfSend<u8> for T {}
    <RemoteFreeAvailability as AmbiguousIfSend<_>>::proof();
    assert_eq!(size_of::<RemoteFreeAvailability>(), size_of::<bool>());
}

#[test]
fn positive_witness_survives_another_true_publication() {
    let available = AtomicBool::new(true);
    let witness = RemoteFreeAvailability::new(available.load(Ordering::Relaxed));
    available.store(true, Ordering::Release);

    assert_eq!(
        (available.load(Ordering::Relaxed), witness.available_at_finish(&available)),
        (true, true)
    );
}

#[test]
fn missing_witness_keeps_an_unavailable_finish_unrecorded() {
    let available = AtomicBool::new(false);
    let witness = RemoteFreeAvailability::new(available.load(Ordering::Relaxed));

    assert_eq!(
        (available.load(Ordering::Relaxed), witness.available_at_finish(&available)),
        (false, false)
    );
}

#[test]
fn false_to_true_fallback_rejects_a_negative_cache() {
    let available = AtomicBool::new(false);
    let observed = available.load(Ordering::Relaxed);
    let witness = RemoteFreeAvailability::new(observed);
    available.store(true, Ordering::Release);

    // `observed` alone is the deliberately wrong negative-cache control.
    assert_eq!(
        (witness.available_at_finish(&available), available.load(Ordering::Relaxed), observed,),
        (true, true, false)
    );
}

#[test]
fn false_reset_is_a_counterexample_to_the_positive_witness_premise() {
    let available = AtomicBool::new(true);
    let witness = RemoteFreeAvailability::new(available.load(Ordering::Relaxed));
    // This reset is confined to this isolated fixture; production never resets.
    available.store(false, Ordering::Release);

    assert_eq!(
        (witness.available_at_finish(&available), available.load(Ordering::Relaxed)),
        (true, false)
    );
}

#[test]
fn isolated_modulo_sequence_keeps_the_six_event_positions() {
    // Only the finish decision below is production code. These local counters
    // mirror the event sequence without resetting allocator process globals.
    let available = AtomicBool::new(true);
    let counters = [const { AtomicUsize::new(usize::MAX) }; 4];
    let [f, p, i, d] = &counters;
    let reports = || counters.each_ref().map(|counter| counter.load(Ordering::Relaxed));
    let observed = available.load(Ordering::Relaxed);
    i.fetch_add(1, Ordering::Relaxed);
    f.fetch_add(1, Ordering::Relaxed);
    p.fetch_add(1, Ordering::Relaxed);
    let witness = RemoteFreeAvailability::new(observed);
    let begun = reports();
    if witness.available_at_finish(&available) {
        i.fetch_sub(1, Ordering::Relaxed);
    }
    let finished = reports();
    p.fetch_sub(1, Ordering::Relaxed);
    d.fetch_add(1, Ordering::Relaxed);
    let drained = reports();

    assert_eq!(
        (begun, finished, drained),
        (
            [0, 0, 0, usize::MAX],
            [0, 0, usize::MAX, usize::MAX],
            [0, usize::MAX, usize::MAX, 0],
        )
    );
}
