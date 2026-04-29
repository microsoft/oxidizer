//! HTTP fixture: defines `HttpClient` whose `#[resolvable]` constructor pulls
//! dependencies from sibling crates `xc_io_driver` and `xc_scheduler`.

#![allow(missing_docs, missing_debug_implementations)]

pub mod client {
    use xc_io_driver::driver::IoDriver;
    use xc_scheduler::scheduler::Scheduler;

    #[derive(Clone)]
    pub struct HttpClient;

    #[autoresolve::resolvable]
    impl HttpClient {
        pub fn new(_io: &IoDriver, _sched: &Scheduler) -> Self {
            Self
        }
    }
}

pub mod request {
    #[derive(Clone)]
    pub struct Request;
}
