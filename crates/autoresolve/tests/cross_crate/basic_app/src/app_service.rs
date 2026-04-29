use xc_http::client::HttpClient;
use xc_scheduler::scheduler::Scheduler;

#[derive(Clone)]
pub struct AppService {
    pub built: bool,
}

#[autoresolve::resolvable]
impl AppService {
    pub fn new(_client: &HttpClient, _sched: &Scheduler) -> Self {
        Self { built: true }
    }
}
