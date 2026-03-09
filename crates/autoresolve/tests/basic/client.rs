use autoresolve_macros::resolvable;

use super::clock::Clock;
use super::validator::Validator;

#[derive(Clone)]
pub struct Client {
    validator: Validator,
    clock: Clock,
}

#[resolvable]
impl Client {
    fn new(validator: &Validator, clock: &Clock) -> Self {
        Self {
            validator: validator.clone(),
            clock: clock.clone(),
        }
    }

    pub(crate) fn number(&self) -> i32 {
        self.validator.number() + self.clock.number()
    }
}
