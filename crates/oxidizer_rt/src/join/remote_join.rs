// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use futures::FutureExt;

use crate::{PlacementToken, non_blocking_thread, once_event};

/// Enables the caller to obtain a result from a task running on any worker thread.
///
/// Spawning a task supplies the caller a join handle for the task.
///
/// # Panics
///
/// The result may be obtained at most once, either by awaitng the future or by calling `wait`.
/// Attempting to obtain the result multiple times will panic.
#[derive(Debug)]
pub struct RemoteJoinHandle<R>
where
    R: Send + 'static,
{
    rx: once_event::shared::InefficientReceiver<R>,
    placement_token: Option<PlacementToken>,
}

impl<R> RemoteJoinHandle<R>
where
    R: Send + 'static,
{
    pub(crate) const fn new(
        rx: once_event::shared::InefficientReceiver<R>,
        placement_token: PlacementToken,
    ) -> Self {
        Self {
            rx,
            placement_token: Some(placement_token),
        }
    }

    pub(crate) const fn new_unplaced(rx: once_event::shared::InefficientReceiver<R>) -> Self {
        Self {
            rx,
            placement_token: None,
        }
    }

    /// Creates a join handle that will never succeed in joining with anything.
    pub(crate) fn new_never() -> Self {
        Self {
            rx: once_event::shared::new_inefficient().1,
            placement_token: None,
        }
    }

    /// Synchronously waits for the task to complete, returning the result.
    ///
    /// # Panics
    ///
    /// Panics if the result has already been obtained either via `wait()` or by awaiting.
    ///
    /// Panics if called from a thread owned by the Oxidizer Runtime. This function is only intended
    /// to be called from a blocking-safe context such as `fn main()` or a `#[test]` entry point.
    pub fn wait(&mut self) -> R {
        non_blocking_thread::assert_not_flagged();

        futures::executor::block_on(futures::future::poll_fn(|cx| self.rx.poll_unpin(cx)))
    }

    #[doc = include_str!("../../doc/snippets/fn_runtime_placement.md")]
    #[must_use]
    pub const fn placement(&self) -> Option<PlacementToken> {
        self.placement_token
    }
}

impl<R> Future for RemoteJoinHandle<R>
where
    R: Send + 'static,
{
    type Output = R;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.rx.poll_unpin(cx)
    }
}