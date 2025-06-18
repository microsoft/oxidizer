// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::Rc;

use futures::future::{FutureExt, LocalBoxFuture};

use crate::{
    DispatchStop, DispatcherClient, Instantiation, RemoteJoinHandle, RuntimeBuilder,
    RuntimeThreadState, SpawnInstance, TaskMeta, TaskScheduler, TaskSchedulerCore,
    non_blocking_thread,
};

pub type BoxedFutureFactory<'a, R, TS> = Box<dyn (FnOnce(TS) -> LocalBoxFuture<'a, R>) + 'a + Send>;

/// Provides arbitrary code access to an instance of Oxidizer Runtime, allowing the caller to
/// observe and control the runtime, as well as to schedule work on it.
///
/// You can either create an instance via `Runtime::new()` or customize the runtime by start with
/// [`RuntimeBuilder::new()`], specifying custom options and calling [`build()`][RuntimeBuilder::build].
///
/// When an instance of this type is dropped, scheduled tasks are abandoned and the runtime is
/// stopped. This will block the current thread until the runtime completes shutdown, which implies
/// that instances of this type should not be dropped on a thread owned by the runtime itself.
///
/// # Thread safety
///
/// This type is thread-safe. You may share instances between any number of threads (e.g. via `Arc`)
/// and access it from any thread.
///
/// Note that some methods (those that may block the thread) must not be called from threads owned
/// by the Oxidizer Runtime, or they will panic. See documentation of individual methods for details.
#[derive(Debug)]
pub struct Runtime<TS>
where
    TS: RuntimeThreadState,
{
    // Since Runtime is a thread-safe type, we create a new clone of `DispatcherClient` for each
    // call instead of merely cloning a Rc (since the Rc would not be thread-safe). This is not
    // exactly optimal but the dispatcher logic is not performance-oriented anyway right now, so
    // once we get to performance work we will need to change much in there.
    dispatcher: DispatcherClient<TS>,
}

impl<TS> Runtime<TS>
where
    TS: RuntimeThreadState,
    TS::SharedState: Default,
{
    /// Creates and starts a new instance of the runtime with the default configuration.
    ///
    /// This is equivalent to calling [`RuntimeBuilder::new::<TS>().build()`][RuntimeBuilder].
    pub fn new() -> Result<Self, TS::Error> {
        RuntimeBuilder::new::<TS>().build()
    }
}

impl<TS> Runtime<TS>
where
    TS: RuntimeThreadState,
{
    /// Creates and starts a new instance of the runtime with the default configuration and given
    /// [runtime thread state config][RuntimeThreadState].
    ///
    /// This is equivalent to calling [`RuntimeBuilder::new::<TS>().build()`][RuntimeBuilder].
    // "Default runtime" != "new runtime" - dangerously different concepts.
    pub fn with_shared_state(config: TS::SharedState) -> Result<Self, TS::Error> {
        RuntimeBuilder::with_shared_state(config).build()
    }

    /// Convenience method to start an async task using the provided future factory and shut
    /// down the runtime once that task completes, intended to wrap an `async fn main()` equivalent.
    ///
    /// # Panics
    ///
    /// Panics if called from a thread owned by the Oxidizer Runtime. This function is only intended
    /// to be called from a blocking-safe context such as `fn main()` or a `#[test]` entry point.
    #[doc = include_str!("../doc/snippets/async_task.md")]
    pub fn run<FF, F, R>(self, future_factory: FF) -> R
    where
        FF: FnOnce(TS) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        non_blocking_thread::assert_not_flagged();
        self.spawn(future_factory).wait()
    }

    #[doc = include_str!("../doc/snippets/fn_runtime_stop.md")]
    #[cfg_attr(test, mutants::skip)] // It is hard to check for a lack of code execution.
    pub fn stop(&self) {
        self.dispatcher.stop();
    }

    /// Waits for the runtime to shut down.
    ///
    /// It is safe to call this function multiple times.
    ///
    /// # Panics
    ///
    /// Panics if called from a thread owned by the Oxidizer Runtime. This function is only intended
    /// to be called from a blocking-safe context such as `fn main()` or a `#[test]` entry point.
    // Impractical to test real waiting at this API layer. We test the real waiter implementation
    // but not the API layers that simply call the waiter, as it is hard to prove the wait failed.
    #[cfg_attr(test, mutants::skip)]
    pub fn wait(&self) {
        non_blocking_thread::assert_not_flagged();

        self.dispatcher.wait();
    }

    /// Blocks the current thread and spawns the provided future factory on runtime,
    /// waiting for future to finish
    ///
    /// Allows capturing by reference
    /// ```
    /// use oxidizer_rt::BasicThreadStateError;
    ///
    /// fn main() -> Result<(), BasicThreadStateError> {
    ///     use oxidizer_rt::{BasicThreadState, Runtime};
    ///     let mut runtime = Runtime::<BasicThreadState>::new()?;
    ///     let mut foo = String::from("Hello");
    ///     let result = runtime.block_on({async |_ctx| {
    ///        &foo.push_str(" World");
    ///     }});
    ///     assert_eq!(&foo, "Hello World");
    ///
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if called from a thread owned by the Oxidizer Runtime. This function is only intended
    /// to be called from a blocking-safe context such as `fn main()` or a `#[test]` entry point.
    pub fn block_on<'a, FF, F, R>(&self, future_factory: FF) -> R
    where
        FF: FnOnce(TS) -> F + Send + 'a,
        F: Future<Output = R> + 'a,
        R: Send + 'static,
    {
        self.block_on_with_meta(TaskMeta::default(), future_factory)
    }

    /// Same as [`Runtime::block_on`], but allows specification of task metadata
    pub fn block_on_with_meta<'a, FF, F, R>(
        &self,
        meta: impl Into<TaskMeta>,
        future_factory: FF,
    ) -> R
    where
        FF: FnOnce(TS) -> F + Send + 'a,
        F: Future<Output = R> + 'a,
        R: Send + 'static,
    {
        non_blocking_thread::assert_not_flagged();
        let boxed_future_factory: BoxedFutureFactory<'a, R, TS> =
            Box::new(|ctx| future_factory(ctx).boxed_local());
        // Safety: We are transmuting the lifetime of the future factory to `'static.`
        // This is safe, because the task is self-contained and ensures that the closure
        // capturing a reference to the outside world finishes before we return from this function.
        // Any panic during the execution takes down the whole runtime including spawned tasks,
        // so the lifetime of the captured reference is guaranteed to be valid even in edge cases
        let future_factory = unsafe {
            std::mem::transmute::<BoxedFutureFactory<'a, R, TS>, BoxedFutureFactory<'static, R, TS>>(
                boxed_future_factory,
            )
        };
        let mut join_handle = self.scheduler().spawn_with_meta(meta, future_factory);
        join_handle.wait()
    }

    /// Starts a new async task using default task metadata
    ///
    #[doc = include_str!("../doc/snippets/async_task.md")]
    ///
    #[doc = include_str!("../doc/snippets/task_future_factory.md")]
    ///
    /// # Panics
    ///
    /// Panics if called from a thread owned by the Oxidizer Runtime. This function is only intended
    /// to be called from a blocking-safe context such as `fn main()` or a `#[test]` entry point.
    pub fn spawn<FF, F, R>(&self, future_factory: FF) -> RemoteJoinHandle<R>
    where
        FF: FnOnce(TS) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.scheduler().spawn(future_factory)
    }

    /// Starts a new async task using the specified task metadata.
    ///
    #[doc = include_str!("../doc/snippets/async_task.md")]
    ///
    #[doc = include_str!("../doc/snippets/task_future_factory.md")]
    ///
    /// # Panics
    ///
    /// Panics if called from a thread owned by the Oxidizer Runtime. This function is only intended
    /// to be called from a blocking-safe context such as `fn main()` or a `#[test]` entry point.
    pub fn spawn_with_meta<FF, F, R>(
        &self,
        meta: impl Into<TaskMeta>,
        future_factory: FF,
    ) -> RemoteJoinHandle<R>
    where
        FF: FnOnce(TS) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.scheduler().spawn_with_meta(meta, future_factory)
    }

    /// Starts multiple async tasks using default task metadata.
    ///
    #[doc = include_str!("../doc/snippets/async_task.md")]
    ///
    #[doc = include_str!("../doc/snippets/task_future_factory.md")]
    ///
    /// # Panics
    ///
    /// Panics if called from a thread owned by the Oxidizer Runtime. This function is only intended
    /// to be called from a blocking-safe context such as `fn main()` or a `#[test]` entry point.
    pub fn spawn_multiple<FF, F, R>(
        &self,
        instantiation: Instantiation,
        future_factory: FF,
    ) -> Box<[RemoteJoinHandle<R>]>
    where
        FF: FnOnce(TS, SpawnInstance) -> F + Clone + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.scheduler()
            .spawn_multiple(instantiation, future_factory)
    }

    /// Starts multiple async tasks used the specified task metadata.
    ///
    #[doc = include_str!("../doc/snippets/async_task.md")]
    ///
    #[doc = include_str!("../doc/snippets/task_future_factory.md")]
    ///
    /// # Panics
    ///
    /// Panics if called from a thread owned by the Oxidizer Runtime. This function is only intended
    /// to be called from a blocking-safe context such as `fn main()` or a `#[test]` entry point.
    pub fn spawn_multiple_with_meta<FF, F, R>(
        &self,
        instantiation: Instantiation,
        meta: impl Into<TaskMeta>,
        future_factory: FF,
    ) -> Box<[RemoteJoinHandle<R>]>
    where
        FF: FnOnce(TS, SpawnInstance) -> F + Clone + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.scheduler()
            .spawn_multiple_with_meta(instantiation, meta, future_factory)
    }

    // Exists to reuse the scheduler functionality for TaskMeta processing and similar.
    // We do not expose schedulers via the Runtime type because they are intended for usage in an
    // async context, whereas the Runtime type is intended for usage in a synchronous context.
    fn scheduler(&self) -> TaskScheduler<'static, TS> {
        TaskScheduler::new(TaskSchedulerCore::new(Rc::new(self.dispatcher.clone())))
    }

    pub(crate) const fn with_dispatcher(dispatcher: DispatcherClient<TS>) -> Self {
        Self { dispatcher }
    }
}

impl<TS> Drop for Runtime<TS>
where
    TS: RuntimeThreadState,
{
    // Inconvenient to test because we would be checking for "does some code stop executing".
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        self.stop();
        self.wait();
    }
}