use autoresolve_macros::base;

use super::clock::Clock;
use super::scheduler::Scheduler;

#[base(helper_module_exported_as = crate::runtime::builtins_helper)]
#[derive(Clone)]
pub struct Builtins {
    pub scheduler: Scheduler,
    pub clock: Clock,
}
