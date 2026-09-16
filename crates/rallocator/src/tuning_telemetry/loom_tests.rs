// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Models the production admission protocol using Loom atomics. Its closed-bit
//! transition calculation is shared with production; std atomics cannot be
//! instrumented directly by Loom.

use loom::sync::Arc;
use loom::sync::atomic::{AtomicUsize, Ordering};
use loom::thread;

use super::{RECORDER_COUNT, RECORDING_CLOSED, admitted_count};

#[test]
#[should_panic(expected = "controller missed admitted recorder")]
fn old_release_acquire_protocol_allows_the_reported_store_buffering_failure() {
    loom::model(|| {
        let session = Arc::new(AtomicUsize::new(1));
        let recorders = Arc::new(AtomicUsize::new(0));
        let active = Arc::clone(&session);
        let in_flight = Arc::clone(&recorders);
        let recorder = thread::spawn(move || {
            if active.load(Ordering::Acquire) != 1 {
                return false;
            }
            in_flight.fetch_add(1, Ordering::Acquire);
            active.load(Ordering::Acquire) == 1
        });
        session.store(0, Ordering::Release);
        let drained = recorders.load(Ordering::Acquire) == 0;
        let admitted = recorder.join().unwrap();
        assert!(!(drained && admitted), "controller missed admitted recorder");
    });
}

#[test]
fn closed_gate_cannot_miss_an_admitted_recorder() {
    loom::model(|| {
        let state = Arc::new(AtomicUsize::new(0));
        let in_flight = Arc::clone(&state);
        let recorder = thread::spawn(move || {
            // An admitted recorder deliberately remains counted throughout the
            // controller observation, modeling an outstanding counter update.
            in_flight.fetch_update(Ordering::AcqRel, Ordering::Acquire, admitted_count).is_ok()
        });
        state.fetch_or(RECORDING_CLOSED, Ordering::AcqRel);
        let drained = state.load(Ordering::Acquire) & RECORDER_COUNT == 0;
        let admitted = recorder.join().unwrap();
        assert!(!(drained && admitted));
    });
}

fn reset_model(workers: usize) {
    loom::model(move || {
        let session = Arc::new(AtomicUsize::new(1));
        let state = Arc::new(AtomicUsize::new(0));
        let counter = Arc::new(AtomicUsize::new(0));
        let recorders = (0..workers)
            .map(|_| {
                let active = Arc::clone(&session);
                let in_flight = Arc::clone(&state);
                let recorded = Arc::clone(&counter);
                thread::spawn(move || {
                    let session = active.load(Ordering::Acquire);
                    if session != 1 {
                        return;
                    }
                    if in_flight.fetch_update(Ordering::AcqRel, Ordering::Acquire, admitted_count).is_err() {
                        return;
                    }
                    if active.load(Ordering::Acquire) == session {
                        recorded.fetch_add(1, Ordering::Relaxed);
                    }
                    in_flight.fetch_sub(1, Ordering::Release);
                })
            })
            .collect::<Vec<_>>();
        state.fetch_or(RECORDING_CLOSED, Ordering::AcqRel);
        session.swap(0, Ordering::AcqRel);
        while state.load(Ordering::Acquire) & RECORDER_COUNT != 0 {
            thread::yield_now();
        }
        counter.store(0, Ordering::Relaxed);
        session.store(2, Ordering::Release);
        state.store(0, Ordering::Release);
        for recorder in recorders {
            recorder.join().unwrap();
        }
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    });
}

#[test]
fn reset_excludes_old_writes_and_rejects_late_old_session_admission() {
    reset_model(1);
}

#[test]
fn last_departure_publishes_all_old_writes_before_reset() {
    loom::model(|| {
        // Admission itself is covered separately. Start with two admitted
        // recorders to isolate the final-departure release-sequence obligation.
        let state = Arc::new(AtomicUsize::new(2));
        let counters = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
        let recorders = (0..2)
            .map(|index| {
                let state = Arc::clone(&state);
                let counters = Arc::clone(&counters);
                thread::spawn(move || {
                    counters[index].store(1, Ordering::Relaxed);
                    state.fetch_sub(1, Ordering::Release);
                })
            })
            .collect::<Vec<_>>();
        state.fetch_or(RECORDING_CLOSED, Ordering::AcqRel);
        while state.load(Ordering::Acquire) & RECORDER_COUNT != 0 {
            thread::yield_now();
        }
        assert_eq!(counters[0].load(Ordering::Relaxed), 1);
        assert_eq!(counters[1].load(Ordering::Relaxed), 1);
        for recorder in recorders {
            recorder.join().unwrap();
        }
    });
}
