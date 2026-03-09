use autoresolve_macros::resolvable;

use super::correlation_vector::CorrelationVector;
use super::task_properties::TaskProperties;

/// Request + Task: depends on CorrelationVector and TaskProperties.
#[derive(Clone)]
pub struct TaskHandler {
    pub(crate) cv_instance: usize,
    pub(crate) task_id: u64,
}

#[resolvable]
impl TaskHandler {
    fn new(cv: &CorrelationVector, tp: &TaskProperties) -> Self {
        Self {
            cv_instance: cv.instance,
            task_id: tp.task_id,
        }
    }
}
