// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;
use std::rc::Rc;

use futures::FutureExt;
use negative_impl::negative_impl;

use crate::once_event;

/// Enables the caller to obtain a result from another async task on the same worker thread as
/// the current async task.
///
/// This join handle type supports receiving a single-threaded type (`!Send`) as the result.
///
/// Spawning a task supplies the caller a join handle for the task.
///
/// # Panics
///
/// The result may be obtained at most once. Polling the join handle after it has returned a result
/// will panic.
#[derive(Debug)]
pub struct LocalJoinHandle<R>
where
    R: 'static,
{
    rx: once_event::isolated::InefficientReceiver<R>,
    abort: Rc<Cell<bool>>,
}

impl<R> LocalJoinHandle<R>
where
    R: 'static,
{
    pub(crate) const fn new(
        rx: once_event::isolated::InefficientReceiver<R>,
        abort: Rc<Cell<bool>>,
    ) -> Self {
        Self { rx, abort }
    }

    /// Requests the runtime to stop executing the task associated with this join handle.
    pub fn request_abort(self) {
        self.abort.set(true);
    }
}

impl<R> Future for LocalJoinHandle<R>
where
    R: 'static,
{
    type Output = R;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.rx.poll_unpin(cx)
    }
}

#[negative_impl]
impl<R> !Send for LocalJoinHandle<R> where R: 'static {}
#[negative_impl]
impl<R> !Sync for LocalJoinHandle<R> where R: 'static {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_abort_ensure_set() {
        let abort = Rc::new(Cell::new(false));
        let (_, receiver) = once_event::isolated::new_inefficient::<()>();

        let handle = LocalJoinHandle::new(receiver, Rc::clone(&abort));

        assert!(!handle.abort.get());

        handle.request_abort();

        assert!(abort.get());
    }
}