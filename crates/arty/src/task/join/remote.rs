// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{self, Poll};

use arty_executor::remote::{JoinHandle as RemoteJoinHandle, Pool as RemotePool};
use performables::arc::Arc;
use pin_project::pin_project;
use thread_aware::{Thread, ThreadAware};

use super::JoinError;
use crate::runtime::blocking_worker::is_current_blocking_pool;
use crate::task::Scheduler;

struct ResultRelocation<R> {
    source: Thread,
    relocate: fn(&mut R, Option<&Thread>, &Thread),
}

/// A handle for receiving an async or blocking task's result.
///
/// Await the handle inside async code. Completion produces `Ok(result)`. A task
/// panic or shutdown cancellation produces [`JoinError`] without unwinding the
/// joining task.
///
/// Dropping the handle does not cancel its task or rethrow a task panic.
/// The runtime must remain running for pending async work to complete.
/// If the task returns its own `Result<T, E>`, joining it produces
/// `Result<Result<T, E>, JoinError>`.
///
/// A result from [`Scheduler::spawn_anywhere`] is relocated to the joining
/// Arty worker before it is returned. Joining from outside an Arty worker
/// leaves its association unchanged. Other submission methods do not relocate
/// their results.
///
/// # Panics
///
/// Panics if polled again after its result has been received. A blocking
/// handle also panics if it is polled directly from a callback running in the
/// same blocking pool.
/// Result relocation also propagates a panic if its `ThreadAware`
/// implementation violates the trait's non-panicking contract.
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// # #[arty::main]
/// # async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
/// let task = cx.scheduler().spawn(async |_| 42);
/// assert_eq!(task.await?, 42);
/// # Ok(())
/// # }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
#[derive(derive_more::Debug)]
#[pin_project]
pub struct JoinHandle<R>
where
    R: Send + 'static,
{
    #[debug(ignore)]
    #[pin]
    inner: RemoteJoinHandle<R>,
    invalid_destination: bool,
    blocking_pool: Option<Arc<()>>,
    #[debug(ignore)]
    relocation: Option<ResultRelocation<R>>,
    completed: bool,
}

impl<R> JoinHandle<R>
where
    R: Send + 'static,
{
    pub(crate) fn new(inner: RemoteJoinHandle<R>) -> Self {
        Self {
            inner,
            invalid_destination: false,
            blocking_pool: None,
            relocation: None,
            completed: false,
        }
    }

    pub(crate) fn shutdown(pool: &RemotePool) -> Self {
        let (sender, inner) = pool.channel();
        drop(sender);
        Self::new(inner)
    }

    pub(crate) fn invalid_destination(pool: &RemotePool) -> Self {
        Self {
            invalid_destination: true,
            ..Self::shutdown(pool)
        }
    }

    pub(crate) fn blocking_pool(mut self, pool: Arc<()>) -> Self {
        self.blocking_pool = Some(pool);
        self
    }

    pub(crate) fn relocate_result_from(mut self, source: Thread) -> Self
    where
        R: ThreadAware,
    {
        self.relocation = Some(ResultRelocation {
            source,
            relocate: R::relocate,
        });
        self
    }

    #[cfg(test)]
    pub(crate) fn join(self) -> Result<R, JoinError> {
        futures::executor::block_on(self)
    }
}

#[cfg_attr(test, mutants::skip)] // Removing this guard makes the black-box blocking-pool regression deadlock.
fn assert_not_current_blocking_pool(blocking_pool: Option<&Arc<()>>) {
    assert!(
        !blocking_pool.is_some_and(is_current_blocking_pool),
        "blocking JoinHandle cannot be polled while the current blocking pool waits for it"
    );
}

impl<R> Future for JoinHandle<R>
where
    R: Send + 'static,
{
    type Output = Result<R, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        assert!(!*this.completed, "JoinHandle polled after completion");
        assert_not_current_blocking_pool(this.blocking_pool.as_ref());
        if std::mem::take(this.invalid_destination) {
            *this.completed = true;
            return Poll::Ready(Err(JoinError::invalid_destination()));
        }
        match this.inner.poll(cx) {
            Poll::Ready(result) => {
                *this.completed = true;
                let mut result = result.map_err(JoinError::from_executor);
                if let (Some(relocation), Ok(value)) = (this.relocation.as_ref(), &mut result)
                    && let Some(destination) = Scheduler::current_thread()
                {
                    (relocation.relocate)(value, Some(&relocation.source), &destination);
                }
                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
