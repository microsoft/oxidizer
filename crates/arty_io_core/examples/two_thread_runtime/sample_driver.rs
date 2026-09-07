// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::Duration;

use arty_io_core::{Driver, DriverContext, DriverInit, DriverProvider, Parker};
use thread_aware_core::{Thread, ThreadAware};

static CREATED_DRIVERS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SampleContext {
    driver_thread: thread::ThreadId,
}

impl SampleContext {
    pub(super) const fn driver_thread(&self) -> thread::ThreadId {
        self.driver_thread
    }
}

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
        SampleDriver {
            context: SampleContext {
                driver_thread: thread::current().id(),
            },
            parker: SampleParker,
        }
    }
}

pub(super) struct SampleDriver {
    context: SampleContext,
    parker: SampleParker,
}

impl Driver for SampleDriver {
    type Context = SampleContext;

    fn context(&self) -> Self::Context {
        self.context.clone()
    }

    fn parker(&self) -> &dyn Parker {
        &self.parker
    }

    fn begin_shutdown(&self) {}

    fn poll_shutdown(&self, _cx: &mut Context<'_>) -> Poll<()> {
        Poll::Ready(())
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
