// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! I/O drivers for Arty runtimes.
//!
//! Arty hosts I/O drivers but does not implement I/O itself. An I/O provider
//! supplies an [`IoContext`](arty_io_core::IoContext) and
//! [`Driver`](arty_io_core::Driver); Arty runs the driver and gives tasks access
//! to the context. Providers remain free to use any protocol, library, or
//! operating-system facility.
//!
//! # Example
//!
//! Obtain an I/O context from the task's built-in services, then use the
//! operations exposed by that context:
//!
//! ```
//! # use std::task::Waker;
//! # use arty::core::{Thread, ThreadAware};
//! # use arty::task::Builtins;
//! # use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, IoContext};
//! # #[derive(Clone)]
//! # struct ExampleIo;
//! # impl ExampleIo {
//! #     fn submit(&self, _request: &str) {}
//! # }
//! # impl ThreadAware for ExampleIo {
//! #     fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
//! # }
//! # impl IoContext for ExampleIo {
//! #     type ContextOptions = ();
//! #     type Driver = ExampleDriver;
//! #
//! #     fn context((): Self::ContextOptions) -> Self {
//! #         Self
//! #     }
//! #
//! #     fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
//! #         Ok(ExampleDriver)
//! #     }
//! # }
//! # struct ExampleDriver;
//! # impl Driver for ExampleDriver {
//! #     fn waker(&self) -> Waker {
//! #         Waker::noop().clone()
//! #     }
//! #
//! #     fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
//! #         Ok(())
//! #     }
//! #
//! #     fn shutdown(self) -> Result<(), DriverError> {
//! #         Ok(())
//! #     }
//! # }
//! #[arty::main]
//! async fn main(cx: Builtins) {
//!     let io = cx.io().context::<ExampleIo>(&());
//!     io.submit("request");
//! }
//! ```
//!
//! `ExampleIo` stands in for a provider's context; real contexts expose their
//! own operations.
//!
//! # Using multiple providers
//!
//! A runtime can host multiple context types, with one driver of each type per
//! worker. The first context type requested becomes the blocking
//! [`Primary`](arty_io_core::DriverRole::Primary); the others run as non-blocking
//! [`Secondary`](arty_io_core::DriverRole::Secondary) drivers.
//!
//! Request a context with [`Io::context`]. Each concrete context type is
//! registered once per runtime and reused on later requests.

mod current_host;

use std::fmt;

use arty_io_core::DriverError;
use arty_io_host::IoRuntime;
use performables::arc::Arc;
use performables::sync::channel;
use thread_aware::{Thread, ThreadAware};

use crate::runtime::Error;
use crate::runtime::dispatch::{DispatcherClient, WorkerIndex};
use crate::task::{Builtins, Scheduler};
pub(crate) use current_host::RuntimeDriverHost;

pub(crate) fn runtime_error(dispatcher: &DispatcherClient) -> Option<Error> {
    dispatcher.is_shutting_down().then(Error::runtime_shutting_down)
}

fn fail_runtime(dispatcher: &DispatcherClient, error: DriverError) -> Error {
    let error = Arc::new(error);
    dispatcher.stop_with_error(Error::io_failure(Arc::clone(&error)));
    Error::io_failure(error)
}

/// Access to I/O contexts hosted by an Arty runtime.
///
/// Obtain this handle from [`Builtins::io`](crate::task::Builtins::io). Clones
/// remain associated with the same worker until their runtime relocates them.
/// They can be used from ordinary threads; such access synchronously dispatches
/// to the associated worker and is slower than worker-local access.
///
/// A context type is registered once per runtime. The first request creates,
/// stores a thread-neutral context, then clones and relocates it to initialize
/// the requesting worker's driver. Other workers initialize their own driver
/// when they first request that context. The first context type requested from
/// a runtime is assigned the primary driver role; later context types are
/// assigned the secondary role.
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

    /// Returns the runtime's context of type `C`, registering it if necessary.
    ///
    /// `options` must be initialized before this call. It is cloned and
    /// consumed only when this call performs the runtime-wide registration for
    /// `C`. External-thread access makes an additional clone to cross the
    /// worker boundary.
    ///
    /// Each concrete context type is initialized once per runtime. The first
    /// successful call fixes the context created from `options`; later calls
    /// return clones of that context and ignore any different options supplied.
    ///
    /// # Panics
    ///
    /// Panics if the driver cannot be initialized, the runtime is shutting
    /// down, the handle is no longer bound to an active worker, or the caller
    /// is another Arty worker. Also panics if context retrieval is nested
    /// inside another context's initialization. Driver initialization failure
    /// also requests runtime shutdown.
    #[must_use]
    pub fn context<C>(&self, options: &C::ContextOptions) -> C
    where
        C: arty_io_core::IoContext,
        C::ContextOptions: Clone + Send + 'static,
    {
        match self.try_context(options) {
            Ok(context) => context,
            Err(error) => panic_context(&error),
        }
    }

    /// Returns the runtime's context of type `C`, registering it if necessary.
    ///
    /// `options` must be initialized before this call. It is cloned and
    /// consumed only when this call performs the runtime-wide registration for
    /// `C`. External-thread access makes an additional clone to cross the
    /// worker boundary.
    ///
    /// Each concrete context type is initialized once per runtime. The first
    /// successful call fixes the context created from `options`; later calls
    /// return clones of that context and ignore any different options supplied.
    /// The requesting worker's driver is initialized before this method
    /// returns. Other workers initialize their driver on first retrieval.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the worker-local driver cannot be initialized,
    /// the runtime is shutting down, the handle is no longer bound to an active
    /// worker, the caller is another Arty worker, or context retrieval is
    /// nested inside another context's initialization. Driver initialization
    /// failure also requests runtime shutdown.
    pub(crate) fn try_context<C>(&self, options: &C::ContextOptions) -> Result<C, Error>
    where
        C: arty_io_core::IoContext,
        C::ContextOptions: Clone + Send + 'static,
    {
        if let Some(error) = runtime_error(&self.dispatcher) {
            return Err(error);
        }

        if let Some(result) = RuntimeDriverHost::try_context_current(&self.runtime, &self.thread, options) {
            return result;
        }
        if Scheduler::is_current_worker_thread() {
            return Err(Error::io_outside_worker());
        }
        let Some(worker_index) = self.dispatcher.worker_index(self.thread.id()) else {
            return Err(Error::io_outside_worker());
        };
        let options = Some(options.clone());
        self.try_context_from_external_thread(worker_index, options)
    }

    fn try_context_from_external_thread<C>(&self, worker_index: WorkerIndex, options: Option<C::ContextOptions>) -> Result<C, Error>
    where
        C: arty_io_core::IoContext,
        C::ContextOptions: Clone + Send + 'static,
    {
        let (result_tx, result_rx) = channel::unbounded();
        let io = self.clone();
        let access = Box::new(move |_: &Builtins| {
            let result = RuntimeDriverHost::try_context_current(
                &io.runtime,
                &io.thread,
                &options.expect("options are present for external context access"),
            )
            .unwrap_or_else(|| Err(Error::io_outside_worker()));
            _ = result_tx.send(result);
        });
        if !self.dispatcher.access_io(worker_index, access) {
            return Err(runtime_error(&self.dispatcher).unwrap_or_else(Error::runtime_shutting_down));
        }
        result_rx
            .recv()
            .map_err(|disconnected| runtime_error(&self.dispatcher).unwrap_or_else(|| Error::new(disconnected)))?
    }
}

#[cold]
#[inline(never)]
#[expect(clippy::panic, reason = "get intentionally converts returned runtime errors into panics")]
fn panic_context(error: &Error) -> ! {
    panic!("failed to access Arty I/O context: {error}");
}

impl fmt::Debug for Io {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Io").field("thread", &self.thread).finish_non_exhaustive()
    }
}

impl ThreadAware for Io {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        if self.dispatcher.owns(destination) {
            self.thread = destination.clone();
        }
    }
}
