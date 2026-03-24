use autoresolve_macros::base;

#[derive(Clone)]
pub struct Scheduler;

#[base(helper_module_exported_as = crate::app_base_helper)]
pub struct AppBase {
    pub scheduler: Scheduler,
}

// Error: scoped() expects a path or ident, not a literal.
#[base(scoped(123), helper_module_exported_as = crate::request_base_helper)]
pub struct RequestBase {
    pub scheduler: Scheduler,
}

fn main() {}
