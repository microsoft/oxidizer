// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Relocation preserves the clock's original per-thread timer storage.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Wake, Waker};

use seatbelt::ResilienceContext;
use thread_aware::{ThreadAware, ThreadBuilder};
use tick::runtime::InactiveClock;

testing_aids::init_tracing!();

#[derive(Default, Debug)]
struct DriverWake(AtomicUsize);

impl Wake for DriverWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn gamma_round2_resilience_context_relocation_restores_the_original_clock_driver_ownership() {
    let builder = ThreadBuilder::default();
    let source = builder.build(std::thread::spawn(|| std::thread::current().id()).join().unwrap());
    let destination = builder.build(std::thread::current().id());
    let wakes = Arc::new(DriverWake::default());
    let (clock, driver) = InactiveClock::new_isolated().activate(Waker::from(Arc::clone(&wakes)));
    let mut context = ResilienceContext::<(), ()>::new(clock);
    context.relocate(Some(&source), &destination);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    context.relocate(Some(&destination), &source);
    drop(context);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 2);
    drop(driver);
}
