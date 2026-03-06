use autoresolve_macros::{base, resolvable};

#[derive(Clone)]
struct Scheduler;

#[derive(Clone)]
struct Request;

#[derive(Clone)]
struct CorrelationVector;

#[resolvable]
impl CorrelationVector {
    fn new(_request: &Request) -> Self {
        Self
    }
}

#[base]
struct AppBase {
    scheduler: Scheduler,
}

#[base(scoped(AppBase))]
struct RequestBase {
    request: Request,
}

fn main() {
    let mut parent = autoresolve::Resolver::new(AppBase { scheduler: Scheduler });

    // CorrelationVector depends on Request (request-scoped) — this must not compile
    // from a Resolver<AppBase>.
    let _ = parent.get::<CorrelationVector>();
}
