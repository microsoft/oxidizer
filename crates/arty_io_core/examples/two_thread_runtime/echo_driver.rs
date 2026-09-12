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
pub(super) struct EchoContext {
    state: Arc<EchoState>,
}

#[derive(Debug)]
struct EchoState {
    driver_thread: thread::ThreadId,
    shutdown_started: AtomicBool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct EchoIoError;

impl EchoContext {
    pub(super) fn perform_io(&self, input: &str) -> Result<String, EchoIoError> {
        // Acquire observes admission closure published by driver shutdown or drop.
        if self.state.shutdown_started.load(Ordering::Acquire) {
            println!("echo I/O operation rejected after shutdown on {:?}", self.state.driver_thread);
            return Err(EchoIoError);
        }

        println!("echo I/O operation handled by {:?}", self.state.driver_thread);
        Ok(input.to_ascii_uppercase())
    }
}

impl PartialEq for EchoContext {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
}

impl Eq for EchoContext {}

impl ThreadAware for EchoContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl IoContext for EchoContext {
    type Provider = EchoProvider;

    fn provider(_context: ProviderContext) -> Self::Provider {
        EchoProvider
    }
}

#[derive(Clone)]
pub(super) struct EchoProvider;

impl ThreadAware for EchoProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for EchoProvider {
    type Context = EchoContext;
    type Driver = EchoDriver;

    fn create(self, _context: DriverContext) -> Self::Driver {
        // The count is diagnostic only and does not synchronize driver creation.
        CREATED_DRIVERS.fetch_add(1, Ordering::Relaxed);
        let driver_thread = thread::current().id();
        println!("initializing echo I/O driver on {driver_thread:?}");

        EchoDriver {
            state: Arc::new(EchoState {
                driver_thread,
                shutdown_started: AtomicBool::new(false),
            }),
        }
    }
}

pub(super) struct EchoDriver {
    state: Arc<EchoState>,
}

impl Drop for EchoDriver {
    fn drop(&mut self) {
        // Release publishes the closed state to contexts that may outlive this driver.
        self.state.shutdown_started.store(true, Ordering::Release);
    }
}

impl Driver for EchoDriver {
    type Context = EchoContext;

    fn context(&self) -> Self::Context {
        EchoContext {
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
        println!("shutting down echo I/O driver on {:?}", self.state.driver_thread);
        println!("echo I/O driver shutdown complete on {:?}", self.state.driver_thread);
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
