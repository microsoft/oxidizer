use autoresolve::base;

use crate::Clock;

#[base(helper_module_exported_as = crate::runtime::internal::builtins_helper)]
pub struct Builtins {
    pub clock: Clock,
}