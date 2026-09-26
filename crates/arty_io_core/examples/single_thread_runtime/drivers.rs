// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty_io_core::{
    Cycle, Driver, DriverError, DriverHandle, DriverOptions, DriverProvider, DriverRole, IoContext, ProviderOptions, ShutdownError,
};
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
    const CAN_BE_PRIMARY: bool = true;

    type Context = SampleContext;
    type Driver = SampleDriver;

    fn create(self, options: DriverOptions<'_>) -> Result<(Self::Driver, Self::Context), DriverError> {
        println!("initializing sample driver as {:?}", options.role());
        Ok((SampleDriver { role: options.role() }, SampleContext))
    }
}

pub(super) struct SampleDriver {
    role: DriverRole,
}

impl Driver for SampleDriver {
    fn handle(&self) -> DriverHandle<'_> {
        DriverHandle::new(self)
    }

    fn on_peer_registered(&mut self, peer: DriverHandle<'_>) {
        if peer.handle().is::<EchoDriver>() {
            println!("sample driver discovered echo driver");
        }
    }

    fn execute_cycle(&mut self, _cycle: Cycle<'_>) -> Result<(), DriverError> {
        let _ = self.role;
        Ok(())
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
    const CAN_BE_PRIMARY: bool = false;

    type Context = EchoContext;
    type Driver = EchoDriver;

    fn create(self, options: DriverOptions<'_>) -> Result<(Self::Driver, Self::Context), DriverError> {
        let sample_registered = options.drivers().iter().any(|driver| driver.handle().is::<SampleDriver>());
        println!(
            "initializing echo driver as {:?}; sample driver registered: {sample_registered}",
            options.role()
        );
        Ok((EchoDriver { role: options.role() }, EchoContext))
    }
}

pub(super) struct EchoDriver {
    role: DriverRole,
}

impl Driver for EchoDriver {
    fn handle(&self) -> DriverHandle<'_> {
        DriverHandle::new(self)
    }

    fn on_peer_registered(&mut self, _peer: DriverHandle<'_>) {}

    fn execute_cycle(&mut self, _cycle: Cycle<'_>) -> Result<(), DriverError> {
        let _ = self.role;
        Ok(())
    }

    fn shutdown(self) -> Result<(), ShutdownError> {
        println!("shutting down echo driver");
        Ok(())
    }
}
