// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public executor contracts for retained and panicking task wakers.

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Poll, Wake, Waker};

use arty_executor::{CycleOutcome, Executor, TaskOptions};

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
    let _task = executor.tasks().add(
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
    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
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
    let task = executor.tasks().add(
        poll_fn({
            let retained = Rc::clone(&retained);
            move |cx| {
                *retained.borrow_mut() = Some(cx.waker().clone());
                Poll::Ready(())
            }
        }),
        TaskOptions::default(),
    );

    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    let retained = retained.borrow_mut().take().unwrap();
    drop(task);
    executor.begin_shutdown();
    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
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
    let task = executor.tasks().add(
        poll_fn({
            let retained = Rc::clone(&retained);
            move |cx| {
                retained.borrow_mut().get_or_insert_with(|| cx.waker().clone());
                Poll::<()>::Pending
            }
        }),
        TaskOptions::default(),
    );
    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    let retained = retained.borrow_mut().take().unwrap();

    assert!(catch_unwind(AssertUnwindSafe(|| retained.wake())).is_err());
    drop(task);
    executor.begin_shutdown();
    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
}
