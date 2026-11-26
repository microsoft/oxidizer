// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public executor contracts for retained and panicking task wakers.

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use arty_executor::{CycleOutcome, Executor, PanicPolicy};
use seismograph_runtime::TaskIdentity;

use super::support::{TaskBuilderExt as _, TaskOutcome};

struct CountingWake(AtomicUsize);

impl Wake for CountingWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn remote_wake_burst_notifies_the_owner_once() {
    let owner = Arc::new(CountingWake(AtomicUsize::new(0)));
    let mut executor = Executor::builder().owner_waker(Waker::from(Arc::clone(&owner))).build();
    let retained = Rc::new(RefCell::new(Vec::new()));
    for _ in 0..2 {
        drop(
            executor
                .tasks()
                .task(
                    TaskIdentity::from_location(std::panic::Location::caller()),
                    poll_fn({
                        let retained = Rc::clone(&retained);
                        move |cx| {
                            retained.borrow_mut().push(cx.waker().clone());
                            Poll::<()>::Pending
                        }
                    }),
                )
                .register_test(),
        );
    }
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    owner.0.store(0, Ordering::Relaxed);
    let wakers = retained.take();

    std::thread::spawn(move || {
        for waker in wakers {
            waker.wake();
        }
    })
    .join()
    .unwrap();

    assert_eq!(owner.0.load(Ordering::Relaxed), 1);
}

#[test]
fn remote_notifications_preserve_first_wake_order_and_task_ownership() {
    let mut executor = Executor::builder().build();
    let ready = Rc::new(Cell::new(false));
    let polls = Rc::new(RefCell::new(Vec::new()));
    let retained = Rc::new(RefCell::new(Vec::new()));
    let mut handles = std::array::from_fn::<_, 3, _>(|index| {
        executor
            .tasks()
            .task(
                TaskIdentity::from_location(std::panic::Location::caller()),
                poll_fn({
                    let ready = Rc::clone(&ready);
                    let polls = Rc::clone(&polls);
                    let retained = Rc::clone(&retained);
                    move |cx| {
                        polls.borrow_mut().push(index);
                        if ready.get() {
                            Poll::Ready(index)
                        } else {
                            retained.borrow_mut().push(cx.waker().clone());
                            Poll::Pending
                        }
                    }
                }),
            )
            .register_test()
    });
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    polls.borrow_mut().clear();
    ready.set(true);
    let wakers = retained.take();

    std::thread::spawn(move || {
        let [first, second, third]: [Waker; 3] = wakers.try_into().unwrap();
        third.wake_by_ref();
        first.wake();
        third.wake();
        second.wake();
    })
    .join()
    .unwrap();

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(*polls.borrow(), [2, 0, 1]);
    for (index, handle) in handles.iter_mut().enumerate() {
        assert!(matches!(
            Pin::new(handle).poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(TaskOutcome::Completed(actual)) if actual == index
        ));
    }
    assert!(!executor.has_tasks());
}

#[test]
fn completed_task_discards_its_queued_remote_notification() {
    let mut executor = Executor::builder().build();
    let polls = Rc::new(Cell::new(0));
    let mut handle = executor
        .tasks()
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            poll_fn({
                let polls = Rc::clone(&polls);
                move |cx| {
                    polls.set(polls.get() + 1);
                    let waker = cx.waker().clone();
                    std::thread::spawn(move || waker.wake()).join().unwrap();
                    Poll::Ready(42)
                }
            }),
        )
        .register_test();

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Continue));
    assert!(matches!(
        Pin::new(&mut handle).poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(TaskOutcome::Completed(42))
    ));
    assert!(!executor.has_tasks());
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(polls.get(), 1);
}

#[test]
fn a_panicked_tasks_retained_waker_does_not_notify_the_live_executor() {
    let owner = Arc::new(CountingWake(AtomicUsize::new(0)));
    let mut executor = Executor::builder()
        .owner_waker(Waker::from(Arc::clone(&owner)))
        .panic_policy(PanicPolicy::IGNORE)
        .build();
    let retained = Rc::new(RefCell::new(None));
    let _task = executor
        .tasks()
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            poll_fn({
                let retained = Rc::clone(&retained);
                move |cx| -> Poll<()> {
                    *retained.borrow_mut() = Some(cx.waker().clone());
                    panic!("contained task panic");
                }
            }),
        )
        .register_test();
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert!(!executor.has_tasks());
    // The test joins the waking thread before reading again. The count publishes no other state.
    let notifications = owner.0.load(Ordering::Relaxed);

    let waker = retained.take().unwrap();
    std::thread::spawn(move || {
        waker.wake_by_ref();
        waker.wake();
    })
    .join()
    .unwrap();

    assert_eq!(owner.0.load(Ordering::Relaxed), notifications);
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
}

#[test]
fn remote_consuming_wake_keeps_payload_on_owner_when_notification_panics() {
    struct PanicWake;

    impl Wake for PanicWake {
        fn wake(self: Arc<Self>) {
            panic!("remote owner wake panic");
        }
    }

    struct DropProbe {
        owner: std::thread::ThreadId,
        dropped: Rc<Cell<bool>>,
    }

    impl Drop for DropProbe {
        fn drop(&mut self) {
            assert_eq!(std::thread::current().id(), self.owner);
            assert!(!self.dropped.replace(true));
        }
    }

    let mut executor = Executor::builder().owner_waker(Waker::from(Arc::new(PanicWake))).build();
    let retained = Rc::new(RefCell::new(None));
    let dropped = Rc::new(Cell::new(false));
    let probe = DropProbe {
        owner: std::thread::current().id(),
        dropped: Rc::clone(&dropped),
    };
    let _task = executor
        .tasks()
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            poll_fn({
                let retained = Rc::clone(&retained);
                move |cx| {
                    let _probe = &probe;
                    *retained.borrow_mut() = Some(cx.waker().clone());
                    Poll::<()>::Pending
                }
            }),
        )
        .register_test();
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    let waker = retained.borrow_mut().take().unwrap();

    std::thread::spawn(move || {
        assert!(catch_unwind(AssertUnwindSafe(|| waker.wake())).is_err());
    })
    .join()
    .unwrap();

    assert!(!dropped.get());
    drop(executor);
    assert!(dropped.get());
}

#[test]
fn retained_waker_is_noop_after_shutdown() {
    let mut executor = Executor::builder().build();
    let retained = Rc::new(RefCell::new(None));
    let task = executor
        .tasks()
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            poll_fn({
                let retained = Rc::clone(&retained);
                move |cx| {
                    *retained.borrow_mut() = Some(cx.waker().clone());
                    Poll::Ready(())
                }
            }),
        )
        .register_test();

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    let retained = retained.borrow_mut().take().unwrap();
    drop(task);
    executor.shutdown().trigger(arty_executor::ShutdownCause::default());
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
    drop(executor);

    std::thread::spawn(move || {
        let cloned = retained.clone();
        cloned.wake_by_ref();
        drop(cloned);
        retained.wake_by_ref();
        retained.wake();
    })
    .join()
    .unwrap();
}

#[test]
fn owner_waker_panic_does_not_prevent_shutdown() {
    struct PanicWake;

    impl Wake for PanicWake {
        fn wake(self: Arc<Self>) {
            panic!("owner wake panic");
        }

        fn wake_by_ref(self: &Arc<Self>) {
            panic!("owner wake panic");
        }
    }

    let mut executor = Executor::builder().owner_waker(Waker::from(Arc::new(PanicWake))).build();
    let retained = Rc::new(RefCell::new(None));
    let task = executor
        .tasks()
        .task(
            TaskIdentity::from_location(std::panic::Location::caller()),
            poll_fn({
                let retained = Rc::clone(&retained);
                move |cx| {
                    retained.borrow_mut().get_or_insert_with(|| cx.waker().clone());
                    Poll::<()>::Pending
                }
            }),
        )
        .register_test();
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    let retained = retained.borrow_mut().take().unwrap();

    assert!(catch_unwind(AssertUnwindSafe(|| retained.wake())).is_err());
    drop(task);
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            executor.shutdown().trigger(arty_executor::ShutdownCause::default());
        }))
        .is_err()
    );
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
}
