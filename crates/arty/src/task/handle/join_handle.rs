// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{self, Poll};

use arty_executor::ShutdownCause;
use events_once::PooledReceiver;
use thread_aware::{Thread, ThreadAware};

use super::JoinError;
use crate::runtime::Shutdown;
use crate::runtime::worker::asynchronous::LocalWorker as AsyncLocalWorker;
use crate::runtime::worker::blocking::{LocalWorker as BlockingLocalWorker, PoolIdentity};

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
/// A result from [`Scheduler::spawn_anywhere`](crate::task::Scheduler::spawn_anywhere)
/// or [`Scheduler::spawn_at`](crate::task::Scheduler::spawn_at) is
/// relocated to the joining Arty worker before it is returned. Joining from
/// outside an Arty worker leaves its association unchanged. Other submission
/// methods do not relocate their results.
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
/// # async fn main(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
/// let task = cx.scheduler().spawn(async |_| 42);
/// assert_eq!(task.await?, 42);
/// # Ok(())
/// # }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
#[derive(derive_more::Debug)]
pub struct JoinHandle<R>
where
    R: Send + 'static,
{
    #[debug(ignore)]
    state: State<R>,
    blocking_pool: Option<PoolIdentity>,
    #[debug(ignore)]
    relocation: Option<ResultRelocation<R>>,
}

/// Optional metadata for relocating a successful result.
///
/// `source` is the worker that executed the task. The function pointer keeps
/// the [`ThreadAware`] bound on registration instead of every join handle.
struct ResultRelocation<R> {
    source: Thread,
    relocate: fn(&mut R, Option<&Thread>, &Thread),
}

enum State<R>
where
    R: Send + 'static,
{
    Pending {
        rx: PooledReceiver<Result<R, JoinError>>,
        shutdown: Shutdown,
    },
    Error(Option<ImmediateError>),
    Completed,
}

enum ImmediateError {
    Shutdown(ShutdownCause),
    InvalidDestination,
}

impl ImmediateError {
    fn into_join_error(self) -> JoinError {
        match self {
            Self::Shutdown(cause) => JoinError::shutdown(cause.into_error()),
            Self::InvalidDestination => JoinError::invalid_destination(),
        }
    }
}

impl<R> JoinHandle<R>
where
    R: Send + 'static,
{
    pub(super) fn new(rx: PooledReceiver<Result<R, JoinError>>, shutdown: Shutdown) -> Self {
        Self {
            state: State::Pending { rx, shutdown },
            blocking_pool: None,
            relocation: None,
        }
    }

    pub(crate) fn shutdown(cause: ShutdownCause) -> Self {
        Self {
            state: State::Error(Some(ImmediateError::Shutdown(cause))),
            blocking_pool: None,
            relocation: None,
        }
    }

    pub(crate) fn try_from_shutdown(shutdown: &Shutdown) -> Option<Self> {
        shutdown.cause().map(Self::shutdown)
    }

    pub(crate) fn invalid_destination() -> Self {
        Self {
            state: State::Error(Some(ImmediateError::InvalidDestination)),
            blocking_pool: None,
            relocation: None,
        }
    }

    pub(crate) fn blocking_pool(mut self, pool: PoolIdentity) -> Self {
        self.blocking_pool = Some(pool);
        self
    }

    /// Opts a successful result into relocation from its execution worker when received.
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
fn assert_not_current_blocking_pool(blocking_pool: Option<&PoolIdentity>) {
    assert!(
        !blocking_pool.is_some_and(BlockingLocalWorker::is_current_pool),
        "blocking JoinHandle cannot be polled while the current blocking pool waits for it"
    );
}

impl<R> Future for JoinHandle<R>
where
    R: Send + 'static,
{
    type Output = Result<R, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        assert_not_current_blocking_pool(this.blocking_pool.as_ref());

        let mut result = match &mut this.state {
            State::Pending { rx, shutdown } => Pin::new(rx)
                .poll(cx)
                .map(|result| result.unwrap_or_else(|_| Err(JoinError::shutdown(shutdown.error())))),
            State::Error(error) => Poll::Ready(Err(error
                .take()
                .expect("an immediate join error is returned exactly once")
                .into_join_error())),
            State::Completed => unreachable!("JoinHandle polled after completion"),
        };

        if let Poll::Ready(Ok(value)) = &mut result
            && let Some(relocation) = this.relocation.as_ref()
            && let Some(destination) = AsyncLocalWorker::current_thread()
        {
            (relocation.relocate)(value, Some(&relocation.source), &destination);
        }
        if result.is_ready() {
            this.state = State::Completed;
        }
        result
    }
}
