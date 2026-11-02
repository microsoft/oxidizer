// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt;

use arty_io_core::{DriverError, IoContext};
use arty_io_host::IoRuntime;
use performables::arc::Arc;
use performables::sync::channel;
use thread_aware::{Thread, ThreadAware};

use super::driver_host::RuntimeDriverHost;
use crate::runtime::Error;
use crate::runtime::dispatch::{DispatcherClient, WorkerIndex};
use crate::runtime::worker::asynchronous::LocalWorker;
use crate::task::Context;

/// Access to an Arty runtime's I/O contexts.
///
/// Use [`Context::io`](crate::task::Context::io) to access these contexts.
/// Cloning an `Io` value keeps its worker assignment.
/// Arty can pass the value to another worker in the same runtime.
/// Its [`ThreadAware`] implementation updates its worker assignment.
///
/// Threads outside Arty can call [`context`](Self::context) on a clone.
/// The call sends a request to the assigned worker and waits for its response.
/// This is slower than access on that worker.
///
/// Arty creates each context type once per runtime.
/// The first request creates and stores a context that does not belong to any worker.
/// Arty clones it and updates the clone's state for the requesting worker.
/// It then creates that worker's driver.
/// Other workers create their own driver when they first request that context.
/// The driver for the first context type registered in the runtime is primary.
/// It can wait for I/O.
/// Later drivers are secondary and must not wait for I/O.
#[derive(Clone)]
pub struct Io {
    runtime: IoRuntime,
    dispatcher: DispatcherClient,
    thread: Thread,
}

impl Io {
    pub(crate) fn new(runtime: IoRuntime, dispatcher: DispatcherClient, thread: Thread) -> Self {
        Self {
            runtime,
            dispatcher,
            thread,
        }
    }

    /// Returns the runtime's context of type `C`, creating it on the first request.
    ///
    /// Prepare `options` before this call.
    /// Arty can clone it while sending the request to the worker or accessing the context there.
    /// Keep context options inexpensive to clone. The options type must implement `Clone`,
    /// `Send`, and `'static`.
    ///
    /// Arty creates each context type once per runtime. The first call
    /// that creates the context determines its `options`. Later calls
    /// return clones of that context and ignore any different options supplied.
    ///
    /// # Panics
    ///
    /// Panics if context access cannot complete, such as during shutdown, from
    /// another Arty async worker, or if creating the driver fails.
    /// Panics in the I/O library or code that reports runtime information also
    /// pass through this method. So do panics from broken internal assumptions.
    #[must_use]
    pub fn context<C>(&self, options: &C::ContextOptions) -> C
    where
        C: IoContext,
        C::ContextOptions: Clone + Send + 'static,
    {
        match self.try_context(options) {
            Ok(context) => context,
            Err(error) => panic_context(&error),
        }
    }

    /// Returns the runtime's context of type `C`, creating it on the first request.
    ///
    /// Prepare `options` before this call.
    /// Arty can clone it while sending the request to the worker or accessing the context there.
    /// Keep context options inexpensive to clone. The options type must implement `Clone`,
    /// `Send`, and `'static`.
    ///
    /// Arty creates each context type once per runtime. The first call
    /// that creates the context determines its `options`. Later calls
    /// return clones of that context and ignore any different options supplied.
    /// The requesting worker's driver is ready before this method returns.
    /// Other workers create their driver when they first request the context.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] in these cases:
    ///
    /// - Arty cannot create the worker's driver.
    /// - The runtime is shutting down.
    /// - This `Io` value does not belong to an active worker.
    /// - Another Arty async worker calls this method. The worker can belong to
    ///   the same runtime or a different runtime.
    /// - Code that creates a context requests another context before it finishes.
    ///
    /// If creating a driver fails, Arty also requests runtime shutdown.
    pub(crate) fn try_context<C>(&self, options: &C::ContextOptions) -> Result<C, Error>
    where
        C: IoContext,
        C::ContextOptions: Clone + Send + 'static,
    {
        if let Some(error) = runtime_error(&self.dispatcher) {
            return Err(error);
        }

        if let Some(result) = RuntimeDriverHost::try_context_current(&self.runtime, &self.thread, options) {
            return result;
        }
        if LocalWorker::is_current() {
            return Err(Error::io_outside_worker());
        }
        let Some(worker_index) = self.dispatcher.worker_index(self.thread.id()) else {
            return Err(Error::io_outside_worker());
        };
        let options = options.clone();
        self.try_context_from_external_thread(worker_index, options)
    }

    fn try_context_from_external_thread<C>(&self, worker_index: WorkerIndex, options: C::ContextOptions) -> Result<C, Error>
    where
        C: IoContext,
        C::ContextOptions: Clone + Send + 'static,
    {
        let (result_tx, result_rx) = channel::unbounded();
        let io = self.clone();
        let access = Box::new(move |_: &Context| {
            let result = RuntimeDriverHost::try_context_current(&io.runtime, &io.thread, &options)
                .unwrap_or_else(|| Err(Error::io_outside_worker()));
            _ = result_tx.send(result);
        });
        self.dispatcher.access_io(worker_index, access)?;
        result_rx
            .recv()
            .map_err(|disconnected| runtime_error(&self.dispatcher).unwrap_or_else(|| Error::new(disconnected)))?
    }
}

impl fmt::Debug for Io {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("thread", &self.thread)
            .finish_non_exhaustive()
    }
}

impl ThreadAware for Io {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        if self.dispatcher.owns(destination) {
            self.thread = destination.clone();
        }
    }
}

pub(super) fn runtime_error(dispatcher: &DispatcherClient) -> Option<Error> {
    Error::from_shutdown(dispatcher.shutdown())
}

pub(super) fn fail_runtime(dispatcher: &DispatcherClient, error: DriverError) -> Error {
    let error = Arc::new(error);
    let cause = dispatcher.stop(Error::io_failure(Arc::clone(&error)).into_shutdown_cause());
    crate::runtime::worker::asynchronous::LocalWorker::begin_executor_shutdown(dispatcher, cause);
    Error::io_failure(error)
}

#[cold]
#[inline(never)]
#[expect(clippy::panic, reason = "context intentionally converts returned runtime errors into panics")]
fn panic_context(error: &Error) -> ! {
    panic!("failed to access Arty I/O context: {error}");
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::sync::mpsc;
    use std::task::Waker;

    use arty_io_core::{Cycle, Driver, DriverOptions};

    use super::*;
    use crate::runtime::worker::WorkerMarker;

    thread_local! {
        static IO_ON_DROP: RefCell<Option<IoOnDrop>> = const { RefCell::new(None) };
    }

    struct IoOnDrop {
        io: Io,
        completed: mpsc::Sender<()>,
    }

    impl Drop for IoOnDrop {
        fn drop(&mut self) {
            _ = self.io.context::<TestContext>(&());
            self.completed.send(()).unwrap();
        }
    }

    #[derive(Clone)]
    struct TestContext;

    impl ThreadAware for TestContext {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    impl IoContext for TestContext {
        type ContextOptions = ();
        type Driver = TestDriver;

        fn context((): Self::ContextOptions) -> Self {
            Self
        }

        fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
            Ok(TestDriver)
        }
    }

    struct TestDriver;

    impl Driver for TestDriver {
        fn waker(&self) -> Waker {
            Waker::noop().clone()
        }

        fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
            Ok(())
        }

        fn shutdown(self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    #[test]
    fn blocking_marker_io_lookup_survives_scheduler_slot_teardown() {
        let runtime = crate::runtime::Runtime::new().unwrap();
        let context = runtime.scheduler().block_on(async |cx| cx).unwrap();
        let scheduler = context.scheduler().clone();
        let io = context.io().clone();
        let (completed, observed) = mpsc::channel();

        std::thread::spawn(move || {
            IO_ON_DROP.with(|owner| {
                assert!(owner.replace(Some(IoOnDrop { io, completed })).is_none());
            });
            WorkerMarker::attach();
            scheduler.spawn(async |_| {}).join().unwrap();
        })
        .join()
        .unwrap();

        observed.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
        runtime.stop().unwrap();
    }
}
