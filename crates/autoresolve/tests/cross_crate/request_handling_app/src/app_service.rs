use xc_http::client::HttpClient;
use xc_scheduler::scheduler::Scheduler;

use crate::app_context::AppContext;

#[derive(Clone)]
pub struct AppService {
    pub built: bool,
}

#[autoresolve::resolvable]
impl AppService {
    pub fn new(_client: &HttpClient, _sched: &Scheduler, _ctx: &AppContext) -> Self {
        Self { built: true }
    }
}
