// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retained permits, cancellation handoff and mixed notification waiters.

use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use performables::sync::notify::Notify;
use performables::sync::{Async, Sync};

#[path = "support/waker.rs"]
mod waker_support;

const DEADLINE: Duration = Duration::from_secs(if cfg!(miri) { 120 } else { 5 });

#[test]
fn construction_and_coalescing_work_in_both_modes() {
    fn check<M: performables::sync::Mode>() {
        let notify = Notify::<M>::new();
        assert!(!notify.is_notified());
        assert!(!notify.try_wait());
        notify.notify_one();
        notify.notify_one();
        assert!(notify.is_notified());
        assert!(notify.wait_timeout(Duration::ZERO));
        assert!(!notify.wait_timeout(Duration::ZERO));
        notify.notify_one();
        notify.wait();
        assert!(!notify.try_wait());
    }
    let _: Notify = const { Notify::new() };
    let _: Notify<Async> = const { Notify::new() };
    check::<Sync>();
    check::<Async>();
}

#[test]
fn enabling_two_consumers_preserves_two_notifications() {
    let notify = Notify::<Async>::new();
    let mut first = pin!(notify.wait_async());
    let mut second = pin!(notify.wait_async());
    assert!(!first.enable());
    assert!(!first.enable());
    assert!(!second.enable());
    notify.notify_one();
    notify.notify_one();
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(first.as_mut().poll(&mut cx), Poll::Ready(()));
    assert_eq!(second.as_mut().poll(&mut cx), Poll::Ready(()));
    assert!(!notify.try_wait());
}

#[test]
fn cancelling_a_selected_waiter_passes_its_notification_to_the_next() {
    let notify = Notify::<Async>::new();
    let mut first = Box::pin(notify.wait_async());
    let mut second = pin!(notify.wait_async());
    assert!(!first.enable());
    assert!(!second.enable());
    notify.notify_one();
    drop(first);
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(second.as_mut().poll(&mut cx), Poll::Ready(()));
    assert!(!notify.try_wait());
}

#[test]
fn cancelling_a_received_or_selected_notification_retains_one_permit() {
    let notify = Notify::<Async>::new();
    notify.notify_one();
    let mut received = notify.wait_async();
    assert!(received.enable());
    assert!(received.enable());
    drop(received);
    assert!(notify.try_wait());
    let mut selected = notify.wait_async();
    assert!(!selected.enable());
    notify.notify_one();
    drop(selected);
    assert!(notify.try_wait());
    assert!(!notify.try_wait());
}

#[test]
fn notification_during_waker_registration_is_not_lost() {
    let notify = Arc::new(Notify::<Async>::new());
    let waker = waker_support::clone_hook_waker({
        let notify = Arc::clone(&notify);
        move || notify.notify_one()
    });
    let mut wait = pin!(notify.wait_async());
    assert!(wait.as_mut().poll(&mut Context::from_waker(&waker)).is_ready());
    assert!(!notify.try_wait());
}

#[test]
fn cancelling_unselected_waiters_and_unpolled_futures_does_not_consume() {
    let notify = Notify::<Async>::new();
    let mut cancelled = notify.wait_async();
    assert!(!cancelled.enable());
    drop(cancelled);
    notify.notify_one();
    drop(notify.wait_async());
    assert!(notify.try_wait());
}

#[test]
fn extra_notifications_after_queued_delivery_leave_one_stored_permit() {
    let notify = Notify::<Async>::new();
    let mut wait = pin!(notify.wait_async());
    assert!(!wait.enable());
    notify.notify_one();
    notify.notify_one();
    notify.notify_one();
    assert!(notify.try_wait());
    assert!(!notify.try_wait());
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(()));
    assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(()));
    assert!(!notify.try_wait());
}

#[test]
fn cancelling_one_of_two_unselected_waiters_keeps_the_other_notifiable() {
    let notify = Notify::<Async>::new();
    let mut cancelled = notify.wait_async();
    let mut remaining = pin!(notify.wait_async());
    assert!(!cancelled.enable());
    assert!(!remaining.enable());
    drop(cancelled);
    notify.notify_one();
    assert!(remaining.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_ready());
    assert!(!notify.try_wait());
}

#[test]
fn blocking_and_async_waiters_share_one_notification_state() {
    let notify = Arc::new(Notify::<Async>::new());
    let mut asynchronous = pin!(notify.wait_async());
    assert!(!asynchronous.enable());
    let (done_tx, done_rx) = mpsc::channel();
    let worker = std::thread::spawn({
        let notify = Arc::clone(&notify);
        move || done_tx.send(notify.wait_timeout(DEADLINE)).unwrap()
    });
    notify.notify_one();
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(asynchronous.as_mut().poll(&mut cx), Poll::Ready(()));
    notify.notify_one();
    assert!(done_rx.recv_timeout(DEADLINE).unwrap());
    worker.join().unwrap();
    assert!(!notify.try_wait());
}

#[test]
fn blocking_waiters_publish_and_observe_across_threads() {
    let rounds = if cfg!(miri) { 4 } else { 100 };
    let notify = Arc::new(Notify::<Sync>::new());
    let published = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = mpsc::channel();
    for _ in 0..rounds {
        let worker = std::thread::spawn({
            let notify = Arc::clone(&notify);
            let published = Arc::clone(&published);
            let done_tx = done_tx.clone();
            move || {
                notify.wait();
                assert!(published.load(Ordering::Relaxed));
                done_tx.send(()).unwrap();
            }
        });
        published.store(true, Ordering::Relaxed);
        notify.notify_one();
        done_rx.recv_timeout(DEADLINE).unwrap();
        worker.join().unwrap();
        published.store(false, Ordering::Relaxed);
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "OS timer smoke runs natively/careful; deterministic expiry and spurious-wake tests remain under Miri"
)]
fn timeout_removes_registration_and_does_not_discard_a_later_notification() {
    let notify = Notify::<Sync>::new();
    assert!(!notify.wait_timeout(Duration::from_millis(1)));
    notify.notify_one();
    assert!(notify.wait_timeout(Duration::ZERO));
    assert!(!notify.try_wait());
}

#[test]
fn an_overflowing_deadline_can_receive_a_notification() {
    let notify = Arc::new(Notify::<Sync>::new());
    let (done_tx, done_rx) = mpsc::channel();
    let worker = std::thread::spawn({
        let notify = Arc::clone(&notify);
        move || done_tx.send(notify.wait_timeout(Duration::MAX)).unwrap()
    });
    notify.notify_one();
    assert!(done_rx.recv_timeout(DEADLINE).unwrap());
    worker.join().unwrap();
}

#[derive(Default)]
struct Counter(std::sync::atomic::AtomicUsize);

impl Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn the_latest_registered_waker_is_used_once() {
    let notify = Notify::<Async>::new();
    let first = Arc::new(Counter::default());
    let second = Arc::new(Counter::default());
    let first_waker = Waker::from(Arc::clone(&first));
    let second_waker = Waker::from(Arc::clone(&second));
    let mut wait = pin!(notify.wait_async());
    assert!(wait.as_mut().poll(&mut Context::from_waker(&first_waker)).is_pending());
    assert!(wait.as_mut().poll(&mut Context::from_waker(&second_waker)).is_pending());
    notify.notify_one();
    assert_eq!((first.0.load(Ordering::Relaxed), second.0.load(Ordering::Relaxed)), (0, 1),);
    assert!(wait.as_mut().poll(&mut Context::from_waker(&second_waker)).is_ready());
}

#[test]
fn notification_types_and_future_are_send_and_sync() {
    fn send_sync<T: Send + std::marker::Sync>() {}
    fn send<T: Send>(_: T) {}
    send_sync::<Notify>();
    send_sync::<Notify<Async>>();
    let notify = Notify::<Async>::default();
    send(notify.wait_async());
    assert!(format!("{notify:?}").contains("Notify"));
}
