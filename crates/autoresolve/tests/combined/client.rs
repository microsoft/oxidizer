use autoresolve_macros::resolvable;

use super::scheduler::Scheduler;
use super::telemetry::Telemetry;
use super::validator::Validator;

#[derive(Clone)]
pub struct Client {
    validator: Validator,
    scheduler: Scheduler,
    telemetry: Telemetry,
}

#[resolvable]
impl Client {
    fn new(validator: &Validator, scheduler: &Scheduler, telemetry: &Telemetry) -> Self {
        Self {
            validator: validator.clone(),
            scheduler: scheduler.clone(),
            telemetry: telemetry.clone(),
        }
    }

    pub(crate) fn number(&self) -> i32 {
        self.validator.number() + self.scheduler.number()
    }
}
