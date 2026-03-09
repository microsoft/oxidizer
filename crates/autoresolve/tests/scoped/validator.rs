use std::sync::atomic::Ordering;

use autoresolve_macros::resolvable;

use super::scheduler::Scheduler;

/// Depends on Scheduler (app-level). `instance` records which construction this was.
#[derive(Clone)]
pub struct Validator {
    pub(crate) instance: usize,
}

#[resolvable]
impl Validator {
    fn new(scheduler: &Scheduler) -> Self {
        Self {
            instance: scheduler.counter.fetch_add(1, Ordering::SeqCst) + 1,
        }
    }
}
