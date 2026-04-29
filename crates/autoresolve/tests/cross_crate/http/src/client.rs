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
