// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::{Cell, RefCell};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{self, Waker};

/// A future that can be externally commanded and observed by its creator.
///
/// Features include:
///
/// * Stores the waker for the creator's inspection (and triggering).
/// * Allows the creator to indicate that the future should complete when next polled.
/// * Allows the creator to indicate that the future should self-waken when next polled.
/// * Allows the creator to execute custom logic in the middle of a poll.
#[derive(derive_more::Debug)]
pub(crate) struct TestSubjectFuture {
    // The waker from the most recent poll() is stored in here.
    waker: Rc<RefCell<Option<Waker>>>,

    completes_on_next_poll: Rc<Cell<bool>>,
    wakes_self_on_next_poll: Rc<Cell<bool>>,

    #[expect(clippy::type_complexity, reason = "never needs to be named, good enough")]
    #[debug(ignore)]
    on_poll: Rc<RefCell<Box<dyn FnMut(&mut task::Context<'_>)>>>,
}

impl TestSubjectFuture {
    pub(crate) fn new() -> Self {
        Self {
            waker: Rc::new(RefCell::new(None)),
            completes_on_next_poll: Rc::new(Cell::new(false)),
            wakes_self_on_next_poll: Rc::new(Cell::new(false)),
            on_poll: Rc::new(RefCell::new(Box::new(move |_| {}))),
        }
    }

    pub(crate) fn waker(&self) -> Rc<RefCell<Option<Waker>>> {
        Rc::clone(&self.waker)
    }

    pub(crate) fn completes_on_next_poll(&self) -> Rc<Cell<bool>> {
        Rc::clone(&self.completes_on_next_poll)
    }

    pub(crate) fn wakes_self_on_next_poll(&self) -> Rc<Cell<bool>> {
        Rc::clone(&self.wakes_self_on_next_poll)
    }

    pub(crate) fn on_poll(&self, f: impl FnMut(&mut task::Context<'_>) + 'static) {
        *self.on_poll.borrow_mut() = Box::new(f);
    }
}

impl Default for TestSubjectFuture {
    fn default() -> Self {
        Self::new()
    }
}

impl Future for TestSubjectFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        *self.waker.borrow_mut() = Some(cx.waker().clone());

        self.on_poll.borrow_mut()(cx);

        if self.completes_on_next_poll.get() {
            task::Poll::Ready(())
        } else {
            if self.wakes_self_on_next_poll.replace(false) {
                cx.waker().wake_by_ref();
            }

            task::Poll::Pending
        }
    }
}
