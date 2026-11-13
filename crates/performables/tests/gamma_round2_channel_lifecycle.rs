// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Predicate-based watch receives acknowledge the value they return.

use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn gamma_round2_closing_the_receiver_wakes_every_pending_sender() {
    let (sender, receiver) = performables::sync::channel::bounded(1);
    sender.try_send(7).unwrap();
    let counter = Arc::new(WakeCounter::default());
    let waker = Waker::from(Arc::clone(&counter));
    let mut context = Context::from_waker(&waker);
    let mut first = pin!(sender.send_async(11));
    let mut second = pin!(sender.send_async(13));
    assert!(first.as_mut().poll(&mut context).is_pending());
    assert!(second.as_mut().poll(&mut context).is_pending());
    assert!(receiver.close());
    assert_eq!(counter.0.load(Ordering::SeqCst), 2);
    assert!(matches!(first.as_mut().poll(&mut context), Poll::Ready(Err(error)) if error.is_closed()));
    assert!(matches!(second.as_mut().poll(&mut context), Poll::Ready(Err(error)) if error.is_closed()));
}

#[test]
fn gamma_round2_wait_for_acknowledges_the_returned_version() {
    let (sender, receiver) = performables::sync::channel::watch(1);
    sender.send_replace(2);
    {
        let mut accepted = pin!(receiver.wait_for_async(|value| *value == 2));
        let Poll::Ready(Ok(value)) = accepted.as_mut().poll(&mut Context::from_waker(Waker::noop())) else {
            panic!("the current value already satisfies the predicate");
        };
        assert_eq!(*value, 2);
    }
    assert!(!receiver.has_changed().unwrap());
    let mut changed = pin!(receiver.changed_async());
    assert!(changed.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
}
