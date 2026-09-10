// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`JoinHandle`] for awaiting spawned task results.

use std::fmt::Debug;
use std::pin::Pin;
use std::task::{Context, Poll};

use performables::sync::channel::OneshotReceiver;

/// A handle to a spawned task that can be awaited to retrieve its result.
///
/// This is returned by [`Spawner::spawn`](crate::Spawner::spawn) and implements
/// [`Future`] to allow awaiting the task's completion.
///
/// # Panics
///
/// Awaiting a `JoinHandle` will panic if the spawned task panicked or its
/// runtime stopped before delivering the result.
pub struct JoinHandle<T>(pub(crate) JoinHandleInner<T>);

pub(crate) enum JoinHandleInner<T> {
    #[cfg(feature = "tokio")]
    Tokio(::tokio::task::JoinHandle<T>),
    Custom(OneshotReceiver<T>),
}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut self.get_mut().0 {
            #[cfg(feature = "tokio")]
            JoinHandleInner::Tokio(jh) => Pin::new(jh).poll(cx).map(unwrap_tokio_result),
            JoinHandleInner::Custom(rx) => Pin::new(rx)
                .poll(cx)
                .map(|res| res.expect("spawned task did not produce a result because its channel closed")),
        }
    }
}

#[cfg(feature = "tokio")]
#[expect(clippy::panic, reason = "JoinHandle documents runtime join failures as panics")]
fn unwrap_tokio_result<T>(result: Result<T, tokio::task::JoinError>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("spawned task did not complete: {error}"),
    }
}

impl<T> Debug for JoinHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinHandle").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "tokio")]
    use std::future;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use performables::sync::channel;

    use super::*;

    fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
        panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap()
    }

    #[test]
    fn closed_custom_channel_reports_the_channel_error() {
        let (sender, receiver) = channel::oneshot::<()>();
        drop(sender);
        let handle = JoinHandle(JoinHandleInner::Custom(receiver));

        let panic = catch_unwind(AssertUnwindSafe(|| futures::executor::block_on(handle))).unwrap_err();

        assert!(panic_message(&*panic).contains("channel closed"));
    }

    #[cfg(feature = "tokio")]
    #[test]
    fn cancelled_tokio_task_reports_the_join_error() -> std::io::Result<()> {
        tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
            let task = tokio::spawn(future::pending::<()>());
            task.abort();
            let handle = JoinHandle(JoinHandleInner::Tokio(task));

            let panic = tokio::spawn(handle).await.unwrap_err().into_panic();

            assert!(panic_message(&*panic).contains("was cancelled"));
        });
        Ok(())
    }
}
