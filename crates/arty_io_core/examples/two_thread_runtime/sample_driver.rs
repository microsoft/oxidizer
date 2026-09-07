// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Poll, Waker};
use std::thread;
use std::time::Duration;

use arty_io_core::{Driver, DriverContext, DriverInit, DriverProvider, Parker};
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
    shutdown_complete: AtomicBool,
}

impl SampleContext {
    pub(super) fn driver_thread(&self) -> thread::ThreadId {
        self.state.driver_thread
    }

    pub(super) fn perform_io(&self, input: usize) -> usize {
        let operation = self.state.operations.fetch_add(1, Ordering::Relaxed) + 1;
        println!("in-memory I/O operation #{operation} handled by {:?}", self.state.driver_thread);
        input + 1
    }

    pub(super) fn operation_count(&self) -> usize {
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

impl DriverContext for SampleContext {
    type Provider = SampleProvider;

    fn provider() -> Self::Provider {
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

    fn create(self, _init: DriverInit) -> Self::Driver {
        CREATED_DRIVERS.fetch_add(1, Ordering::Relaxed);
        let driver_thread = thread::current().id();
        println!("initializing sample I/O driver on {driver_thread:?}");

        SampleDriver {
            state: Arc::new(SampleState {
                driver_thread,
                operations: AtomicUsize::new(0),
                shutdown_complete: AtomicBool::new(false),
            }),
            parker: SampleParker,
        }
    }
}

pub(super) struct SampleDriver {
    state: Arc<SampleState>,
    parker: SampleParker,
}

impl Driver for SampleDriver {
    type Context = SampleContext;

    fn context(&self) -> Self::Context {
        SampleContext {
            state: Arc::clone(&self.state),
        }
    }

    fn parker(&self) -> &dyn Parker {
        &self.parker
    }

    fn begin_shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + '_>> {
        SHUTDOWN_DRIVERS.fetch_add(1, Ordering::Relaxed);
        println!("shutting down sample I/O driver on {:?}", self.state.driver_thread);
        Box::pin(std::future::poll_fn(move |_cx| {
            if !self.state.shutdown_complete.swap(true, Ordering::Relaxed) {
                println!("sample I/O driver shutdown complete on {:?}", self.state.driver_thread);
            }

            Poll::Ready(())
        }))
    }
}

struct SampleParker;

impl Parker for SampleParker {
    fn park(&self, _max_wait: Duration) {}

    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }
}

pub(super) fn created_driver_count() -> usize {
    CREATED_DRIVERS.load(Ordering::Relaxed)
}

pub(super) fn shutdown_driver_count() -> usize {
    SHUTDOWN_DRIVERS.load(Ordering::Relaxed)
}
