// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public custom-runtime driver behavior.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::task::{Context, Wake, Waker};
use std::thread;
use std::time::Duration;

use tick::runtime::InactiveClock;

#[derive(Debug)]
struct CountingWake(AtomicUsize);

impl Wake for CountingWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn activation_waker_fires_only_when_the_earliest_deadline_changes() {
    let wake_count = Arc::new(CountingWake(AtomicUsize::new(0)));
    let driver_waker = Waker::from(Arc::clone(&wake_count));
    let (clock, _driver) = InactiveClock::default().activate(driver_waker);
    let mut context = Context::from_waker(Waker::noop());
    let mut wake_counts = [0; 4];

    let mut first = std::pin::pin!(clock.delay(Duration::from_secs(2)));
    assert!(first.as_mut().poll(&mut context).is_pending());
    wake_counts[0] = wake_count.0.load(Ordering::Relaxed);

    let mut later = std::pin::pin!(clock.delay(Duration::from_secs(3)));
    assert!(later.as_mut().poll(&mut context).is_pending());
    wake_counts[1] = wake_count.0.load(Ordering::Relaxed);

    let mut same_or_later = std::pin::pin!(clock.delay(Duration::from_secs(2)));
    assert!(same_or_later.as_mut().poll(&mut context).is_pending());
    wake_counts[2] = wake_count.0.load(Ordering::Relaxed);

    let mut earlier = std::pin::pin!(clock.delay(Duration::from_secs(1)));
    assert!(earlier.as_mut().poll(&mut context).is_pending());
    wake_counts[3] = wake_count.0.load(Ordering::Relaxed);

    assert_eq!(wake_counts, [1, 1, 1, 2]);
}

#[test]
fn dropping_the_final_clock_owner_wakes_the_driver() {
    let wake_count = Arc::new(CountingWake(AtomicUsize::new(0)));
    let driver_waker = Waker::from(Arc::clone(&wake_count));
    let (clock, _driver) = InactiveClock::default().activate(driver_waker);

    drop(clock);

    assert_eq!(wake_count.0.load(Ordering::Relaxed), 1);
}

#[test]
fn concurrent_final_owner_drops_wake_the_driver_once() {
    const OWNER_COUNT: usize = 8;
    let wake_count = Arc::new(CountingWake(AtomicUsize::new(0)));
    let driver_waker = Waker::from(Arc::clone(&wake_count));
    let (clock, _driver) = InactiveClock::default().activate(driver_waker);
    let release = Arc::new(Barrier::new(OWNER_COUNT + 1));
    let owners: [_; OWNER_COUNT] = std::array::from_fn(|_| clock.clone());
    drop(clock);
    let threads: Vec<_> = owners
        .into_iter()
        .map(|clock| {
            let release = Arc::clone(&release);
            thread::spawn(move || {
                release.wait();
                drop(clock);
            })
        })
        .collect();

    release.wait();
    for thread in threads {
        thread.join().unwrap();
    }

    assert_eq!(wake_count.0.load(Ordering::Relaxed), 1);
}
