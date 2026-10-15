// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty_io_core::{DriverError, DriverRole};
use data_privacy::classified;
use observed::event;

static SYSTEM_METADATA: data_privacy::DataClass = data_privacy::DataClass::new("arty", "SystemMetadata");
static IO_DRIVER_ERROR_MESSAGE: data_privacy::DataClass = data_privacy::DataClass::new("arty", "IoDriverErrorMessage");

#[classified(SYSTEM_METADATA)]
#[derive(Clone, Copy)]
pub(crate) struct IoContextName(pub(crate) &'static str);

#[classified(SYSTEM_METADATA)]
#[derive(Clone)]
pub(crate) struct IoDriverRoleLabel(pub(crate) String);

#[classified(IO_DRIVER_ERROR_MESSAGE)]
#[derive(Clone)]
pub(crate) struct IoDriverErrorMessage(pub(crate) String);

impl IoDriverErrorMessage {
    pub(crate) fn new(error: &DriverError) -> Self {
        Self(error.to_string())
    }
}

impl From<DriverRole> for IoDriverRoleLabel {
    fn from(role: DriverRole) -> Self {
        Self(role.to_string())
    }
}

#[event("arty.rt.io.driver.initialized")]
#[debug("I/O driver initialized")]
#[counter(name = "arty.rt.io.driver.initialized")]
pub(crate) struct IoDriverInitialized {
    #[dimension(log = "io.context")]
    pub(crate) context: IoContextName,
    #[dimension(log = "io.driver.role", metric = "io.driver.role")]
    pub(crate) role: IoDriverRoleLabel,
}

#[event("arty.rt.io.driver.initialization_failed")]
#[error("I/O driver initialization failed")]
#[counter(name = "arty.rt.io.driver.initialization_failed")]
pub(crate) struct IoDriverInitializationFailed {
    #[dimension(log = "io.context")]
    pub(crate) context: IoContextName,
    #[dimension(log = "io.driver.role", metric = "io.driver.role")]
    pub(crate) role: IoDriverRoleLabel,
    #[dimension(log = "error.message")]
    pub(crate) error: IoDriverErrorMessage,
}

#[event("arty.rt.io.driver.cycle_failed")]
#[error("I/O driver cycle failed")]
#[counter(name = "arty.rt.io.driver.cycle_failed")]
pub(crate) struct IoDriverCycleFailed {
    #[dimension(log = "io.context")]
    pub(crate) context: IoContextName,
    #[dimension(log = "io.driver.role", metric = "io.driver.role")]
    pub(crate) role: IoDriverRoleLabel,
    #[dimension(log = "error.message")]
    pub(crate) error: IoDriverErrorMessage,
}

#[event("arty.rt.io.driver.shutdown_failed")]
#[error("I/O driver shutdown failed")]
#[counter(name = "arty.rt.io.driver.shutdown_failed")]
pub(crate) struct IoDriverShutdownFailed {
    #[dimension(log = "io.context")]
    pub(crate) context: IoContextName,
    #[dimension(log = "io.driver.role", metric = "io.driver.role")]
    pub(crate) role: IoDriverRoleLabel,
    #[dimension(log = "error.message")]
    pub(crate) error: IoDriverErrorMessage,
}
