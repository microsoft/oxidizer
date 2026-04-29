use crate::scheduler::Scheduler;
use xc_io_driver::driver::IoDriver;

/// A `#[base]` defined here, intended to be re-exported by `xc_runtime`
/// and then `#[spread]`-flattened into application-level bases.
#[autoresolve::base(helper_module_exported_as = crate::core::builtins_helper)]
pub struct Builtins {
    pub scheduler: Scheduler,
    pub io_driver: IoDriver,
}
