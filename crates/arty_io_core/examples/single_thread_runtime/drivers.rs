// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::time::{Duration, Instant};

use arty_io_core::{Driver, DriverHandle, DriverOptions, DriverProvider, IoContext, ProviderOptions, ShutdownError};
use thread_aware_core::{Thread, ThreadAware};

#[derive(Clone)]
pub(super) struct SampleContext;

impl ThreadAware for SampleContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl IoContext for SampleContext {
    type Provider = SampleProvider;

    fn provider(_options: ProviderOptions) -> Self::Provider {
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

    fn create(self, _options: DriverOptions<'_>) -> (Self::Driver, Self::Context) {
        println!("initializing sample driver");
        (SampleDriver, SampleContext)
    }
}

pub(super) struct SampleDriver;

impl Driver for SampleDriver {
    fn handle(&self) -> DriverHandle<'_> {
        DriverHandle::new(self)
    }

    fn on_peer_registered(&mut self, peer: DriverHandle<'_>) {
        if peer.handle().is::<EchoDriver>() {
            println!("sample driver discovered echo driver");
        }
    }

    fn process_completions(&mut self, _max_wait: Duration, _cycle_start: Instant) {}

    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn shutdown(self) -> Result<(), ShutdownError> {
        println!("shutting down sample driver");
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct EchoContext;

impl ThreadAware for EchoContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl IoContext for EchoContext {
    type Provider = EchoProvider;

    fn provider(_options: ProviderOptions) -> Self::Provider {
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

    fn create(self, options: DriverOptions<'_>) -> (Self::Driver, Self::Context) {
        let sample_registered = options.drivers().iter().any(|driver| driver.handle().is::<SampleDriver>());
        println!("initializing echo driver; sample driver registered: {sample_registered}");
        (EchoDriver, EchoContext)
    }
}

pub(super) struct EchoDriver;

impl Driver for EchoDriver {
    fn handle(&self) -> DriverHandle<'_> {
        DriverHandle::new(self)
    }

    fn on_peer_registered(&mut self, _peer: DriverHandle<'_>) {}

    fn process_completions(&mut self, _max_wait: Duration, _cycle_start: Instant) {}

    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn shutdown(self) -> Result<(), ShutdownError> {
        println!("shutting down echo driver");
        Ok(())
    }
}
