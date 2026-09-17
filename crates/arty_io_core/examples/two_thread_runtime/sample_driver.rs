// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Waker;
use std::thread;
use std::time::Duration;

use arty_io_core::{Driver, DriverContext, DriverProvider, IoContext, ProviderContext, ShutdownError};
use thread_aware_core::{Thread, ThreadAware};

static CREATED_DRIVERS: AtomicUsize = AtomicUsize::new(0);
static SHUTDOWN_DRIVERS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug)]
pub(super) struct SampleContext {
    state: Arc<SampleState>,
}

#[derive(Debug)]
struct SampleState {
    driver_thread: thread::ThreadId,
    operations: AtomicUsize,
    shutdown_started: AtomicBool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SampleIoError;

impl SampleContext {
    pub(super) fn driver_thread(&self) -> thread::ThreadId {
        self.state.driver_thread
    }

    pub(super) fn perform_io(&self, input: usize) -> Result<usize, SampleIoError> {
        // Acquire observes admission closure published by driver shutdown or drop.
        if self.state.shutdown_started.load(Ordering::Acquire) {
            println!("in-memory I/O operation rejected after shutdown on {:?}", self.state.driver_thread);
            return Err(SampleIoError);
        }

        // The count is diagnostic only and does not synchronize other state.
        let operation = self.state.operations.fetch_add(1, Ordering::Relaxed) + 1;
        println!("in-memory I/O operation #{operation} handled by {:?}", self.state.driver_thread);
        Ok(input + 1)
    }

    pub(super) fn operation_count(&self) -> usize {
        // The count is diagnostic only and does not synchronize other state.
        self.state.operations.load(Ordering::Relaxed)
    }
}

impl PartialEq for SampleContext {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
}

impl Eq for SampleContext {}

impl ThreadAware for SampleContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl IoContext for SampleContext {
    type Provider = SampleProvider;

    fn provider(_context: ProviderContext) -> Self::Provider {
        SampleProvider
    }
}

#[derive(Clone)]
pub(super) struct SampleProvider;

impl ThreadAware for SampleProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for SampleProvider {
    type Context = SampleContext;
    type Driver = SampleDriver;

    fn create(self, _context: DriverContext) -> Self::Driver {
        // The count is diagnostic only and does not synchronize driver creation.
        CREATED_DRIVERS.fetch_add(1, Ordering::Relaxed);
        let driver_thread = thread::current().id();
        println!("initializing sample I/O driver on {driver_thread:?}");

        SampleDriver {
            state: Arc::new(SampleState {
                driver_thread,
                operations: AtomicUsize::new(0),
                shutdown_started: AtomicBool::new(false),
            }),
        }
    }
}

pub(super) struct SampleDriver {
    state: Arc<SampleState>,
}

impl Drop for SampleDriver {
    fn drop(&mut self) {
        // Release publishes the closed state to contexts that may outlive this driver.
        self.state.shutdown_started.store(true, Ordering::Release);
    }
}

impl Driver for SampleDriver {
    type Context = SampleContext;

    fn context(&self) -> Self::Context {
        SampleContext {
            state: Arc::clone(&self.state),
        }
    }

    fn process_completions(&mut self, _max_wait: Duration) {}

    fn interruptor(&self) -> Waker {
        Waker::noop().clone()
    }

    fn shutdown(self) -> Result<(), ShutdownError> {
        // Release publishes admission closure before graceful cleanup starts.
        self.state.shutdown_started.store(true, Ordering::Release);
        // The count is diagnostic only and does not synchronize shutdown.
        SHUTDOWN_DRIVERS.fetch_add(1, Ordering::Relaxed);
        println!("shutting down sample I/O driver on {:?}", self.state.driver_thread);
        println!("sample I/O driver shutdown complete on {:?}", self.state.driver_thread);
        Ok(())
    }
}

pub(super) fn created_driver_count() -> usize {
    // The count is diagnostic only and does not synchronize driver creation.
    CREATED_DRIVERS.load(Ordering::Relaxed)
}

pub(super) fn shutdown_driver_count() -> usize {
    // The count is diagnostic only and does not synchronize shutdown.
    SHUTDOWN_DRIVERS.load(Ordering::Relaxed)
}
