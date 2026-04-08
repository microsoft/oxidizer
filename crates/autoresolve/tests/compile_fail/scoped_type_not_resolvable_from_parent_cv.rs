use autoresolve_macros::{base, resolvable};

#[derive(Clone)]
pub struct Scheduler;

#[derive(Clone)]
pub struct Request;

#[derive(Clone)]
struct CorrelationVector;

#[resolvable]
impl CorrelationVector {
    fn new(_request: &Request) -> Self {
        Self
    }
}

#[base(helper_module_exported_as = crate::app_base_helper)]
pub struct AppBase {
    pub scheduler: Scheduler,
}

#[base(scoped(AppBase), helper_module_exported_as = crate::request_base_helper)]
pub struct RequestBase {
    pub request: Request,
}

fn main() {
    let mut parent = autoresolve::Resolver::new(AppBase { scheduler: Scheduler });

    // CorrelationVector depends on Request (request-scoped) — this must not compile
    // from a Resolver<AppBase>.
    let _ = parent.get::<CorrelationVector>();
}
