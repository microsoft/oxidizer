// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cross-thread watch registration, cancellation, publication and closure.

use std::cell::Cell;
use std::panic::{RefUnwindSafe, UnwindSafe, catch_unwind};
use std::pin::pin;
use std::sync::{Arc, Barrier, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::{Duration, Instant};

use performables::sync::channel::{WatchReceiver, WatchRef, WatchSender, watch};
use testing_aids::is_mutation_testing;

const DEADLINE: Duration = Duration::from_secs(if cfg!(miri) { 120 } else { 10 });

fn receive<T>(receiver: &mpsc::Receiver<T>) -> Result<T, mpsc::RecvTimeoutError> {
    if is_mutation_testing() {
        receiver.recv().map_err(Into::into)
    } else {
        receiver.recv_timeout(DEADLINE)
    }
}

struct ThreadWake(thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

#[expect(clippy::unwrap_used, reason = "the bounded native test executor must fail if a wake is lost")]
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWake(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    let deadline = (!is_mutation_testing()).then(|| Instant::now() + DEADLINE);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => match deadline {
                Some(deadline) => thread::park_timeout(deadline.checked_duration_since(Instant::now()).unwrap()),
                None => thread::park(),
            },
        }
    }
}

#[test]
fn watch_metadata_does_not_wait_for_a_borrowed_value() {
    let (sender, receiver) = watch(7);
    let sender = Arc::new(sender);
    let held = sender.borrow();
    let (finished, notifications) = mpsc::channel();
    let worker = thread::spawn({
        let sender = Arc::clone(&sender);
        move || {
            let sender = WatchSender::clone(&sender);
            let subscribed = sender.subscribe();
            let cloned = subscribed.clone();
            assert_eq!(cloned.has_changed(), Ok(false));
            assert!(!sender.is_closed());
            assert!(!subscribed.is_closed());
            drop(cloned);
            drop(subscribed);
            drop(sender);
            drop(receiver);
            finished.send(()).unwrap();
        }
    });
    receive(&notifications).unwrap();
    assert_eq!(*held, 7);
    drop(held);
    worker.join().unwrap();
    assert!(sender.is_closed());
}

#[test]
fn many_threads_cancel_waits_while_retaining_a_large_shared_backlog() {
    let workers = if cfg!(miri) { 2 } else { 8 };
    let backlog = if cfg!(miri) { 2 } else { 256 };
    let rounds = if cfg!(miri) { 4 } else { 1_000 };
    let (sender, _receiver) = watch(false);
    let sender = Arc::new(sender);
    let start = Arc::new(Barrier::new(workers));
    let (finished, notifications) = mpsc::channel();
    let handles = (0..workers)
        .map(|_| {
            let sender = Arc::clone(&sender);
            let start = Arc::clone(&start);
            let finished = finished.clone();
            thread::spawn(move || {
                let mut context = Context::from_waker(Waker::noop());
                let pending = (0..backlog)
                    .map(|_| {
                        let receiver = sender.subscribe();
                        let mut wait = Box::pin(async move {
                            let _value = receiver.wait_for_async(|value| *value).await.unwrap();
                        });
                        assert!(wait.as_mut().poll(&mut context).is_pending());
                        wait
                    })
                    .collect::<Vec<_>>();
                start.wait();
                for _ in 0..rounds {
                    let receiver = sender.subscribe();
                    let mut wait = pin!(receiver.wait_for_async(|value| *value));
                    assert!(wait.as_mut().poll(&mut context).is_pending());
                }
                drop(pending);
                finished.send(()).unwrap();
            })
        })
        .collect::<Vec<_>>();
    drop(finished);
    for _ in 0..workers {
        receive(&notifications).unwrap();
    }
    for handle in handles {
        handle.join().unwrap();
    }
    assert!(!*sender.borrow());
}

#[test]
fn executor_threads_observe_complete_values_and_the_final_update_before_close() {
    let workers = if cfg!(miri) { 2 } else { 8 };
    let rounds = if cfg!(miri) { 4 } else { 1_000 };
    let (sender, receiver) = watch((0_usize, 0_usize));
    let start = Arc::new(Barrier::new(workers + 1));
    let (finished, notifications) = mpsc::channel();
    let handles = (0..workers)
        .map(|_| {
            let receiver = receiver.clone();
            let start = Arc::clone(&start);
            let finished = finished.clone();
            thread::spawn(move || {
                start.wait();
                let mut last = 0;
                while block_on(receiver.changed_async()).is_ok() {
                    let value = *receiver.borrow_and_update();
                    assert_eq!(value.0, value.1);
                    assert!(value.0 >= last);
                    last = value.0;
                }
                assert_eq!((last, *receiver.borrow()), (rounds, (rounds, rounds)));
                finished.send(()).unwrap();
            })
        })
        .collect::<Vec<_>>();
    drop(finished);
    start.wait();
    for value in 1..=rounds {
        sender.send_modify(|current| {
            current.0 = value;
            thread::yield_now();
            current.1 = value;
        });
    }
    drop(sender);
    for _ in 0..workers {
        receive(&notifications).unwrap();
    }
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn cancelled_and_live_waiters_race_with_cross_thread_broadcasts() {
    for _ in 0..if cfg!(miri) { 2 } else { 100 } {
        let (sender, first) = watch(false);
        let second = first.clone();
        let mut cancelled = Box::pin(first.changed_async());
        assert!(cancelled.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        let start = Arc::new(Barrier::new(3));
        thread::scope(|scope| {
            let start_cancel = Arc::clone(&start);
            let cancellation = scope.spawn(move || {
                start_cancel.wait();
                drop(cancelled);
            });
            let start_send = Arc::clone(&start);
            let publication = scope.spawn(move || {
                start_send.wait();
                sender.send(true).unwrap();
            });
            start.wait();
            assert_eq!(block_on(second.wait_for_async(|value| *value)).map(|value| *value), Ok(true));
            cancellation.join().unwrap();
            publication.join().unwrap();
        });
    }
}

#[test]
fn dropping_a_notified_registration_cannot_cancel_a_reused_slot() {
    let (sender, first) = watch(0);
    let mut old = Box::pin(first.changed_async());
    let mut context = Context::from_waker(Waker::noop());
    assert!(old.as_mut().poll(&mut context).is_pending());
    sender.send(1).unwrap();
    let mut newer = (0..32)
        .map(|_| {
            let receiver = sender.subscribe();
            let mut wait = Box::pin(async move {
                receiver.changed_async().await.unwrap();
                *receiver.borrow()
            });
            assert!(wait.as_mut().poll(&mut context).is_pending());
            wait
        })
        .collect::<Vec<_>>();
    drop(old);
    sender.send(2).unwrap();
    for wait in &mut newer {
        assert_eq!(wait.as_mut().poll(&mut context), Poll::Ready(2));
    }
}

#[test]
fn concurrent_waits_on_one_receiver_claim_each_version_once() {
    let rounds = if cfg!(miri) { 4 } else { 100 };
    let (sender, receiver) = watch(0);
    let receiver = Arc::new(receiver);
    let start = Arc::new(Barrier::new(3));
    let completed = Arc::new(Barrier::new(3));
    let ready = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let handles = (0..2)
        .map(|_| {
            let receiver = Arc::clone(&receiver);
            let start = Arc::clone(&start);
            let completed = Arc::clone(&completed);
            let ready = Arc::clone(&ready);
            thread::spawn(move || {
                for _ in 0..rounds {
                    start.wait();
                    {
                        let mut wait = pin!(receiver.changed_async());
                        if wait.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_ready() {
                            ready.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    completed.wait();
                }
            })
        })
        .collect::<Vec<_>>();
    for version in 1..=rounds {
        sender.send(version).unwrap();
        start.wait();
        completed.wait();
        assert_eq!(ready.swap(0, std::sync::atomic::Ordering::Relaxed), 1);
    }
    for handle in handles {
        handle.join().unwrap();
    }
}

#[derive(Default)]
struct WakeCount(std::sync::atomic::AtomicUsize);

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[test]
fn a_repolled_wait_uses_its_latest_waker() {
    let (sender, receiver) = watch(0);
    let first = Arc::new(WakeCount::default());
    let second = Arc::new(WakeCount::default());
    let mut wait = pin!(receiver.changed_async());
    let first_waker = Waker::from(Arc::clone(&first));
    let second_waker = Waker::from(Arc::clone(&second));
    assert!(wait.as_mut().poll(&mut Context::from_waker(&first_waker)).is_pending());
    assert!(wait.as_mut().poll(&mut Context::from_waker(&second_waker)).is_pending());
    sender.send(1).unwrap();
    assert_eq!(
        (
            first.0.load(std::sync::atomic::Ordering::Relaxed),
            second.0.load(std::sync::atomic::Ordering::Relaxed),
        ),
        (0, 1)
    );
    assert_eq!(wait.as_mut().poll(&mut Context::from_waker(&second_waker)), Poll::Ready(Ok(())));
}

#[test]
fn publication_from_a_waker_clone_cannot_be_lost_or_deadlock_registration() {
    let (sender, receiver) = watch(0);
    let waker = waker_support::clone_hook_waker(move || sender.send(7).unwrap());
    let mut wait = pin!(receiver.changed_async());
    assert_eq!(wait.as_mut().poll(&mut Context::from_waker(&waker)), Poll::Ready(Ok(())));
    assert_eq!(*receiver.borrow(), 7);
}

struct PublishOnDrop(WatchSender<u64>);

impl Wake for PublishOnDrop {
    fn wake(self: Arc<Self>) {
        unreachable!("the test replaces or cancels this waker before its drop publishes");
    }
}

impl Drop for PublishOnDrop {
    fn drop(&mut self) {
        self.0.send_replace(7);
    }
}

#[test]
fn replacing_or_cancelling_a_waker_can_reenter_publication() {
    for cancel in [false, true] {
        let (sender, receiver) = watch(0);
        let waker = Waker::from(Arc::new(PublishOnDrop(sender.clone())));
        let mut wait = Box::pin(receiver.changed_async());
        assert!(wait.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
        drop(waker);
        if cancel {
            drop(wait);
        } else {
            let mut context = Context::from_waker(Waker::noop());
            let _first = wait.as_mut().poll(&mut context);
            assert_eq!(wait.as_mut().poll(&mut context), Poll::Ready(Ok(())));
        }
        assert_eq!(*receiver.borrow(), 7);
    }
}

#[test]
fn metadata_bounds_and_non_sync_payloads_remain_supported() {
    fn endpoint<T: Send + Sync + UnwindSafe + RefUnwindSafe>() {}
    fn shared_guard<T: Sync>() {}
    fn send<T: Send>(_: T) {}
    trait AmbiguousIfSend<A> {
        fn check() {}
    }
    impl<T: ?Sized> AmbiguousIfSend<()> for T {}
    impl<T: ?Sized + Send> AmbiguousIfSend<u8> for T {}

    endpoint::<WatchSender<Cell<u64>>>();
    endpoint::<WatchReceiver<Cell<u64>>>();
    shared_guard::<WatchRef<'_, std::sync::MutexGuard<'_, ()>>>();
    // Inference becomes ambiguous if a watch guard accidentally gains Send.
    <WatchRef<'_, u64> as AmbiguousIfSend<_>>::check();

    let (sender, receiver) = watch(Cell::new(7));
    send(receiver.changed_async());
    send(receiver.wait_for_async(|value| value.get() == 11));
    let worker = thread::spawn(move || {
        sender.send_modify(|value| value.set(11));
    });
    worker.join().unwrap();
    assert_eq!(receiver.borrow().get(), 11);
}

#[test]
fn unwinding_a_value_modification_releases_the_internal_hal_lock() {
    let (sender, receiver) = watch(7);
    catch_unwind(|| {
        sender.send_modify(|value| {
            *value = 11;
            panic!("test a callback unwinding while the value is held");
        });
    })
    .unwrap_err();
    assert_eq!(*receiver.borrow(), 11);
    assert_eq!(receiver.has_changed(), Ok(false));
    sender.send(13).unwrap();
    assert_eq!(receiver.has_changed(), Ok(true));
    assert_eq!(*receiver.borrow(), 13);
}
#[path = "support/waker.rs"]
mod waker_support;
