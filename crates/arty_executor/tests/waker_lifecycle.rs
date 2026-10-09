// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public executor contracts for retained and panicking task wakers.

use std::cell::RefCell;
use std::future::poll_fn;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Poll, Wake, Waker};

use arty_executor::{CycleOutcome, Executor};

#[test]
fn retained_waker_is_inert_after_shutdown() {
    // SAFETY: The test drives the executor through a terminal shutdown cycle before dropping it.
    let executor = unsafe { Executor::builder().build() };
    let retained = Rc::new(RefCell::new(None));
    let task = executor.tasks().add(poll_fn({
        let retained = Rc::clone(&retained);
        move |cx| {
            *retained.borrow_mut() = Some(cx.waker().clone());
            Poll::Ready(())
        }
    }));

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

    // SAFETY: The test drives the executor through a terminal shutdown cycle before dropping it.
    let executor = unsafe { Executor::builder().owner_waker(Waker::from(Arc::new(PanicWake))).build() };
    let retained = Rc::new(RefCell::new(None));
    let task = executor.tasks().add(poll_fn({
        let retained = Rc::clone(&retained);
        move |cx| {
            retained.borrow_mut().get_or_insert_with(|| cx.waker().clone());
            Poll::<()>::Pending
        }
    }));
    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    let retained = retained.borrow_mut().take().unwrap();

    assert!(catch_unwind(AssertUnwindSafe(|| retained.wake())).is_err());
    drop(task);
    executor.begin_shutdown();
    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
}
