// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic yield, byte-stream, and isolated capture contracts.

testing_aids::init_tracing!();

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use serial_test::serial;
use testing_aids::tracing_logs::{Capture, write_to_stdout_and_buffer};
use testing_aids::{YieldFuture, repeating_incrementing_bytes, repeating_reverse_incrementing_bytes};

#[test]
#[serial]
fn nov4_mutation_mode_query_matches_the_exact_environment_value() {
    let expected = std::env::var("MUTATION_TESTING").is_ok_and(|value| value == "1");
    assert_eq!(testing_aids::is_mutation_testing(), expected);
}

struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
#[serial]
fn gamma_survivor_yield_wakes_once_and_completes_on_second_poll() {
    let wakes = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&wakes));
    let mut context = Context::from_waker(&waker);
    let mut future = YieldFuture::default();
    assert_eq!(Pin::new(&mut future).poll(&mut context), Poll::Pending);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert_eq!(Pin::new(&mut future).poll(&mut context), Poll::Ready(()));
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
}

#[test]
#[serial]
fn gamma_survivor_byte_generators_repeat_all_256_values_in_order() {
    let forward: Vec<_> = repeating_incrementing_bytes().take(769).collect();
    let reverse: Vec<_> = repeating_reverse_incrementing_bytes().take(769).collect();
    assert_eq!(forward.len(), 769);
    assert_eq!(reverse.len(), 769);
    for index in 0..769 {
        assert_eq!(usize::from(forward[index]), index % 256);
        assert_eq!(usize::from(reverse[index]), 255 - index % 256);
    }
}

#[test]
#[serial]
fn gamma_survivor_dropped_capture_can_be_replaced_and_duplicate_capture_preserves_owner() {
    let first = write_to_stdout_and_buffer();
    tracing::info!(target: "gamma-capture", "first");
    std::panic::catch_unwind(write_to_stdout_and_buffer).unwrap_err();
    tracing::info!(target: "gamma-capture", "still-first");
    assert_eq!(first.snapshot(), [" INFO gamma-capture: first", " INFO gamma-capture: still-first"]);
    drop(first);
    tracing::info!(target: "gamma-capture", "between");
    let second = write_to_stdout_and_buffer();
    tracing::info!(target: "gamma-capture", "second");
    assert_eq!(second.into_inner(), [" INFO gamma-capture: second"]);
    let third = write_to_stdout_and_buffer();
    assert_eq!(third.into_inner(), Vec::<String>::new());
}

#[test]
#[serial]
fn gamma_survivor_capture_assert_contains_rejects_missing_output_and_span_spacing_is_exact() {
    let capture = Capture::new();
    let _guard = tracing::subscriber::set_default(capture.subscriber());
    let span = tracing::info_span!("empty", value = tracing::field::Empty);
    span.record("value", tracing::field::Empty);
    span.record("value", 17);
    span.record("value", tracing::field::Empty);
    let _entered = span.enter();
    tracing::info!(target: "gamma-format", "message");
    assert_eq!(capture.output(), " INFO empty{value=17}: gamma-format: message\n");
    capture.assert_contains("value=17");
    assert!(std::panic::catch_unwind(|| capture.assert_contains("missing-marker")).is_err());
}
