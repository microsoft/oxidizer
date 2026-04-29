//! Resolvable application service depending on cross-crate deps.

use xc_http::client::HttpClient;
use xc_scheduler::scheduler::Scheduler;

/// Cross-crate fixture service constructed by autoresolve.
#[derive(Clone, Debug)]
pub struct AppService {
    /// Indicates that the constructor ran (always `true`).
    pub built: bool,
}

#[autoresolve::resolvable]
impl AppService {
    /// Constructs an [`AppService`] from injected dependencies.
    pub fn new(_client: &HttpClient, _sched: &Scheduler) -> Self {
        Self { built: true }
    }
}
