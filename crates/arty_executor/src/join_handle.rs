// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::pin::Pin;
use std::task;

use events_once::RawLocalPooledReceiver;

/// A handle that can be used to await the completion of a task and to receive its result `R`.
///
/// If the join handle becomes disconnected because the task it references failed to reach
/// completion, the join handle will panic when polled.
///
/// # Resource management
///
/// Join handles must be dropped before the executor they came from shuts down or the executor
/// shutdown will time out and terminate the process. You can think of it as there existing an
/// imaginary `&Executor` shared reference held by the join handle, which must be dropped before
/// the executor itself.
#[derive(Debug)]
pub struct JoinHandle<R: 'static> {
    /// The storage is managed by the executor, which will not shut down until all join handles
    /// have been dropped, thereby ensuring that this receiver remains valid.
    rx: RawLocalPooledReceiver<R>,

    _single_threaded: PhantomData<*const ()>,
}

impl<R: 'static> JoinHandle<R> {
    pub(crate) fn new(rx: RawLocalPooledReceiver<R>) -> Self {
        Self {
            rx,
            _single_threaded: PhantomData,
        }
    }
}

impl<R: 'static> Future for JoinHandle<R> {
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        // SAFETY: We are not moving anything.
        unsafe { self.map_unchecked_mut(|x| &mut x.rx) }
            .poll(cx)
            .map(|x| x.expect("join handle is no longer connected - the task failed to reach completion"))
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // We expect these join handles to be single-threaded.
    assert_not_impl_any!(JoinHandle<u8>: Send, Sync);

    // We do not expect there to be anything that requires pinning the join handle.
    assert_impl_all!(JoinHandle<u8>: Unpin);
}
