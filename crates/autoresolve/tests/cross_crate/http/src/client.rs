//! `HttpClient` whose `#[resolvable]` constructor pulls cross-crate deps.

use xc_io_driver::driver::IoDriver;
use xc_scheduler::scheduler::Scheduler;

/// Cross-crate fixture stand-in for an HTTP client.
#[derive(Clone, Debug)]
pub struct HttpClient;

#[autoresolve::resolvable]
impl HttpClient {
    /// Constructs an [`HttpClient`] from injected dependencies.
    pub fn new(_io: &IoDriver, _sched: &Scheduler) -> Self {
        Self
    }
}
