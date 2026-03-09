use autoresolve_macros::resolvable;

use super::runtime::clock::Clock;
use super::runtime::scheduler::Scheduler;

#[derive(Clone)]
pub struct Config {
    clock: Clock,
    scheduler: Scheduler,
}

#[resolvable]
impl Config {
    fn new(clock: &Clock, scheduler: &Scheduler) -> Self {
        Self {
            clock: clock.clone(),
            scheduler: scheduler.clone(),
        }
    }

    pub(crate) fn number(&self) -> i32 {
        self.clock.number() * 2
    }
}
