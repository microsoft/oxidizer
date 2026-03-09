use autoresolve_macros::resolvable;

use super::scheduler::Scheduler;

#[derive(Clone)]
pub struct Validator {
    scheduler: Scheduler,
}

#[resolvable]
impl Validator {
    fn new(scheduler: &Scheduler) -> Self {
        Self {
            scheduler: scheduler.clone(),
        }
    }

    pub(crate) fn number(&self) -> i32 {
        self.scheduler.number()
    }
}
