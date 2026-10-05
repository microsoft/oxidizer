// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;

use arty_io_core::{
    Cycle, Driver, DriverError, DriverInstance, DriverOptions, DriverProvider, DriverRole, IoContext, ProviderOptions, ShutdownError,
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
    type Context = SampleContext;
    type Driver = SampleDriver;

    fn create(self, options: DriverOptions) -> Result<DriverInstance<Self::Driver, Self::Context>, DriverError> {
        println!("initializing sample driver with {:?} permission", options.allowed_roles());
        Ok(DriverInstance::new(SampleDriver, SampleContext, DriverRole::Primary))
    }
}

pub(super) struct SampleDriver;

impl Driver for SampleDriver {
    fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
        Ok(())
    }

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

    fn create(self, options: DriverOptions) -> Result<DriverInstance<Self::Driver, Self::Context>, DriverError> {
        println!("initializing echo driver with {:?} permission", options.allowed_roles());
        Ok(DriverInstance::new(EchoDriver, EchoContext, DriverRole::Secondary))
    }
}

pub(super) struct EchoDriver;

impl Driver for EchoDriver {
    fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
        Ok(())
    }

    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn shutdown(self) -> Result<(), ShutdownError> {
        println!("shutting down echo driver");
        Ok(())
    }
}
