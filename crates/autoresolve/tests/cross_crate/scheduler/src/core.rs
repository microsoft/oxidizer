//! `Builtins` base re-exported by `xc_runtime` and spread into application bases.

use crate::scheduler::Scheduler;
use xc_io_driver::driver::IoDriver;

/// A `#[base]` defined here, intended to be re-exported by `xc_runtime`
/// and then `#[spread]`-flattened into application-level bases.
#[derive(Debug)]
#[autoresolve::base(helper_module_exported_as = crate::core::builtins_helper)]
pub struct Builtins {
    /// Scheduler handle.
    pub scheduler: Scheduler,
    /// I/O driver handle.
    pub io_driver: IoDriver,
}
