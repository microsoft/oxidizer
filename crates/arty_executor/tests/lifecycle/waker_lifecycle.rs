// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public executor contracts for retained and panicking task wakers.

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll, Wake, Waker};

use arty_executor::{CycleOutcome, Executor, TaskOptions, TaskPlacement};
use observed::Sink;
use observed::enrichment::{Enrichment, EnrichmentEntry};

use crate::support::{TaskOutcome, TaskSetExt as _};

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
        drop(executor.tasks().add_test(
            poll_fn({
                let retained = Rc::clone(&retained);
                move |cx| {
                    retained.borrow_mut().push(cx.waker().clone());
                    Poll::<()>::Pending
                }
            }),
            TaskOptions::default(),
        ));
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
        executor.tasks().add_test(
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
            TaskOptions::default(),
        )
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
    let _task = executor.tasks().add_test(
        poll_fn({
            let retained = Rc::clone(&retained);
            move |cx| {
                let _probe = &probe;
                *retained.borrow_mut() = Some(cx.waker().clone());
                Poll::<()>::Pending
            }
        }),
        TaskOptions::default(),
    );
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
    let task = executor.tasks().add_test(
        poll_fn({
            let retained = Rc::clone(&retained);
            move |cx| {
                *retained.borrow_mut() = Some(cx.waker().clone());
                Poll::Ready(())
            }
        }),
        TaskOptions::default(),
    );

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    let retained = retained.borrow_mut().take().unwrap();
    drop(task);
    executor.shutdown().trigger(arty_executor::ShutdownCause::default());
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
    drop(executor);

    std::thread::spawn(move || {
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
    let task = executor.tasks().add_test(
        poll_fn({
            let retained = Rc::clone(&retained);
            move |cx| {
                retained.borrow_mut().get_or_insert_with(|| cx.waker().clone());
                Poll::<()>::Pending
            }
        }),
        TaskOptions::default(),
    );
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

#[test]
fn context_restoration_panic_does_not_release_local_payload_to_remote_waker() {
    struct PanicOnDrop;

    impl data_privacy::RedactedDisplay for PanicOnDrop {
        fn fmt(&self, _: &dyn data_privacy::Redactor, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            Ok(())
        }
    }

    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("context restoration dropped user enrichment");
        }
    }

    struct Entries;

    impl Enrichment for Entries {
        fn into_entries(self) -> Vec<EnrichmentEntry> {
            vec![EnrichmentEntry::new("probe", PanicOnDrop)]
        }
    }

    struct OwnerOnly {
        dropped_on: Arc<Mutex<Option<std::thread::ThreadId>>>,
        _local: Rc<()>,
    }

    impl Drop for OwnerOnly {
        fn drop(&mut self) {
            *self.dropped_on.lock().unwrap() = Some(std::thread::current().id());
        }
    }

    // The deliberately forgotten guard retains slot storage, so keep its sink process-lived.
    static SINK: OnceLock<Sink> = OnceLock::new();
    let sink = SINK.get_or_init(|| Sink::builder("context-restoration-panic").build());
    let owner = std::thread::current().id();
    let mut executor = Executor::builder().build();
    let retained = Rc::new(RefCell::new(None));
    let dropped_on = Arc::new(Mutex::new(None));
    let local = Rc::new(());
    let probe = OwnerOnly {
        dropped_on: Arc::clone(&dropped_on),
        _local: Rc::clone(&local),
    };
    let task = executor.tasks().add_test(
        poll_fn({
            let retained = Rc::clone(&retained);
            move |cx| {
                let _probe = &probe;
                *retained.borrow_mut() = Some(cx.waker().clone());
                let transfer = sink.transfer_context().with_enrichment(Entries);
                std::mem::forget(transfer.apply_current_thread());
                Poll::<()>::Pending
            }
        }),
        TaskOptions::observed(sink.transfer_context(), TaskPlacement::Any),
    );

    catch_unwind(AssertUnwindSafe(|| executor.execute_cycle())).unwrap_err();
    let waker = retained.borrow_mut().take().unwrap();
    let remote = std::thread::spawn(move || drop(waker));
    drop(Rc::clone(&local));
    remote.join().unwrap();
    let actual = *dropped_on.lock().unwrap();

    drop(task);
    // Unsupported hook panics need not permit recovery or complete cleanup.
    let _ = catch_unwind(AssertUnwindSafe(|| drop(executor)));
    assert!(actual.is_none_or(|thread| thread == owner), "local payload was dropped remotely");
    assert!(dropped_on.lock().unwrap().is_none_or(|thread| thread == owner));
}
