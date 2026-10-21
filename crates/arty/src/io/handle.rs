// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt;

use arty_io_core::{DriverError, IoContext};
use arty_io_host::IoRuntime;
use performables::arc::Arc;
use performables::sync::channel;
use thread_aware::{Thread, ThreadAware};

use super::current_host::RuntimeDriverHost;
use crate::runtime::Error;
use crate::runtime::dispatch::{DispatcherClient, WorkerIndex};
use crate::task::{Builtins, Scheduler};

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
        C: IoContext,
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
        C: IoContext,
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
        let access = Box::new(move |_: &Builtins| {
            let result = RuntimeDriverHost::try_context_current(&io.runtime, &io.thread, &options)
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
    dispatcher.is_shutting_down().then(Error::runtime_shutting_down)
}

pub(super) fn fail_runtime(dispatcher: &DispatcherClient, error: DriverError) -> Error {
    let error = Arc::new(error);
    Scheduler::begin_current_executor_shutdown(dispatcher);
    dispatcher.stop_with_error(Error::io_failure(Arc::clone(&error)));
    Error::io_failure(error)
}

#[cold]
#[inline(never)]
#[expect(clippy::panic, reason = "context intentionally converts returned runtime errors into panics")]
fn panic_context(error: &Error) -> ! {
    panic!("failed to access Arty I/O context: {error}");
}
