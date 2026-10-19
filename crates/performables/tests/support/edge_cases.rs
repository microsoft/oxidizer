// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub(crate) const DEADLINE: Duration = Duration::from_secs(if cfg!(miri) { 120 } else { 10 });

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) struct Task {
    count: Arc<WakeCount>,
    waker: Waker,
}

impl Task {
    pub(crate) fn new() -> Self {
        let count = Arc::new(WakeCount::default());
        let waker = Waker::from(Arc::clone(&count));
        Self { count, waker }
    }

    pub(crate) fn poll<F: Future + ?Sized>(&self, future: Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(&self.waker))
    }

    pub(crate) fn wakes(&self) -> usize {
        self.count.0.load(Ordering::Relaxed)
    }

    pub(crate) fn references(&self) -> usize {
        Arc::strong_count(&self.count)
    }
}

#[expect(clippy::unwrap_used, reason = "thread panics and lost progress must fail the bounded test")]
pub(crate) fn join<T>(thread: JoinHandle<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    while !thread.is_finished() {
        assert!(Instant::now() < deadline, "worker did not finish before the test deadline");
        std::thread::yield_now();
    }
    thread.join().unwrap()
}
