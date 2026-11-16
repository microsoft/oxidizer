// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Completion and cancellation contracts without an executor.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use performables::sync::Async;
use performables::sync::barrier::Barrier;
use performables::sync::channel::{oneshot, watch};
use performables::sync::condition::Condvar;
use performables::sync::mutex::Mutex;

#[derive(Default)]
struct Wakes(AtomicUsize);

impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn cancelling_a_barrier_arrival_does_not_leave_a_generation_waiter() {
    let barrier = Barrier::<Async>::new(2);
    let mut cancelled = Box::pin(barrier.wait_async());
    assert!(cancelled.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    drop(cancelled);
    let counter = Arc::new(Wakes::default());
    let waker = Waker::from(Arc::clone(&counter));
    let mut cx = Context::from_waker(&waker);
    let mut leader = Box::pin(barrier.wait_async());
    assert!(leader.as_mut().poll(&mut cx).is_pending());
    assert_eq!(counter.0.load(Ordering::Relaxed), 0);
    let mut follower = Box::pin(barrier.wait_async());
    assert!(follower.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_ready());
    assert!(leader.as_mut().poll(&mut cx).is_ready());
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    assert_eq!(Arc::strong_count(&counter), 2);
    drop(leader);
    let mut next = Box::pin(barrier.wait_async());
    assert!(next.as_mut().poll(&mut cx).is_pending());
    assert_eq!(Arc::strong_count(&counter), 3);
    drop(next);
    assert_eq!(Arc::strong_count(&counter), 2);
}

#[test]
fn already_closed_channel_waits_finish_without_registering_the_callers_waker() {
    let counter = Arc::new(Wakes::default());
    let waker = Waker::from(Arc::clone(&counter));
    let mut cx = Context::from_waker(&waker);
    let (sender, receiver) = watch(7);
    drop(sender);
    let mut changed = Box::pin(receiver.changed_async());
    assert!(changed.as_mut().poll(&mut cx).is_ready());
    assert_eq!(Arc::strong_count(&counter), 2);
    drop(changed);
    let mut predicate = Box::pin(receiver.wait_for_async(|value| *value == 11));
    let Poll::Ready(Err(error)) = predicate.as_mut().poll(&mut cx) else {
        panic!("a closed watch cannot satisfy a rejected predicate");
    };
    assert!(error.is_closed());
    assert_eq!(Arc::strong_count(&counter), 2);
}

#[test]
fn a_ready_oneshot_releases_the_registered_waker_before_its_future_is_dropped() {
    let (sender, mut receiver) = oneshot::<u32>();
    let counter = Arc::new(Wakes::default());
    let waker = Waker::from(Arc::clone(&counter));
    let mut cx = Context::from_waker(&waker);
    assert!(Pin::new(&mut receiver).poll(&mut cx).is_pending());
    assert_eq!(Arc::strong_count(&counter), 3);
    sender.send(31).unwrap();
    assert_eq!(Pin::new(&mut receiver).poll(&mut cx), Poll::Ready(Ok(31)));
    assert_eq!(Arc::strong_count(&counter), 2);
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
}

#[test]
fn a_notified_condition_keeps_its_mutex_queue_position_across_repolls() {
    let mutex = Mutex::<u32, Async>::new(3);
    let condition = Condvar::<Async>::new();
    let first = Arc::new(Wakes::default());
    let second = Arc::new(Wakes::default());
    let first_waker = Waker::from(Arc::clone(&first));
    let second_waker = Waker::from(Arc::clone(&second));
    let mut waiting = Box::pin(condition.wait_async(mutex.lock()));
    assert!(waiting.as_mut().poll(&mut Context::from_waker(&first_waker)).is_pending());
    let held = mutex.lock();
    condition.notify_one();
    assert!(waiting.as_mut().poll(&mut Context::from_waker(&first_waker)).is_pending());
    let mut successor = Box::pin(mutex.lock_async());
    assert!(successor.as_mut().poll(&mut Context::from_waker(&second_waker)).is_pending());
    for _ in 0..3 {
        assert!(waiting.as_mut().poll(&mut Context::from_waker(&first_waker)).is_pending());
    }
    let before = first.0.load(Ordering::Relaxed);
    drop(held);
    assert_eq!(first.0.load(Ordering::Relaxed), before + 1);
    assert_eq!(second.0.load(Ordering::Relaxed), 0);
    let Poll::Ready(guard) = waiting.as_mut().poll(&mut Context::from_waker(&first_waker)) else {
        panic!("the original condition waiter must retain first position");
    };
    assert_eq!(*guard, 3);
    drop(guard);
    assert_eq!(second.0.load(Ordering::Relaxed), 1);
    assert!(successor.as_mut().poll(&mut Context::from_waker(&second_waker)).is_ready());
}
