// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The Azure scheduler adapter yields once and wakes exactly once.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use anyspawn::Spawner;
use anyspawn_azure::Runtime;
use azure_core::async_runtime::AsyncRuntime;
use tick::ClockControl;

#[derive(Default)]
struct CountWake(AtomicUsize);

impl Wake for CountWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn gamma_survivor_yield_now_wakes_first_poll_and_completes_second_without_runtime() {
    let runtime = Runtime::new(Spawner::new_tokio(None), ClockControl::new().to_clock());
    let mut future = runtime.yield_now();
    let count = Arc::new(CountWake::default());
    let waker = Waker::from(Arc::clone(&count));
    let mut cx = Context::from_waker(&waker);
    assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
    assert_eq!(count.0.load(Ordering::Relaxed), 1);
    assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(()));
    assert_eq!(count.0.load(Ordering::Relaxed), 1);
}
