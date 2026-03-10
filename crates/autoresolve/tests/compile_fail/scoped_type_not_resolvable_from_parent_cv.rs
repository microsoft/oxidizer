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

#[base]
mod app_base {
    pub struct AppBase {
        pub scheduler: super::Scheduler,
    }
}

use app_base::AppBase;

#[base(scoped(app_base::AppBase))]
mod request_base {
    pub struct RequestBase {
        pub request: super::Request,
    }
}

fn main() {
    let mut parent = autoresolve::Resolver::new(AppBase { scheduler: Scheduler });

    // CorrelationVector depends on Request (request-scoped) — this must not compile
    // from a Resolver<AppBase>.
    let _ = parent.get::<CorrelationVector>();
}
