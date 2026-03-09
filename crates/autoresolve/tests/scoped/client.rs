use autoresolve_macros::resolvable;

use super::correlation_vector::CorrelationVector;
use super::validator::Validator;

/// App + Request: depends on Validator and CorrelationVector.
#[derive(Clone)]
pub struct Client {
    pub(crate) validator_instance: usize,
    pub(crate) cv_instance: usize,
}

#[resolvable]
impl Client {
    fn new(validator: &Validator, cv: &CorrelationVector) -> Self {
        Self {
            validator_instance: validator.instance,
            cv_instance: cv.instance,
        }
    }
}
