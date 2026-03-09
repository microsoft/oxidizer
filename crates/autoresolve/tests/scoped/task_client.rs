use autoresolve_macros::resolvable;

use super::client::Client;
use super::task_properties::TaskProperties;

/// Task + App (via Client): depends on TaskProperties and Client.
#[derive(Clone)]
pub struct TaskClient {
    pub(crate) task_id: u64,
    pub(crate) validator_instance: usize,
    pub(crate) cv_instance: usize,
}

#[resolvable]
impl TaskClient {
    fn new(tp: &TaskProperties, client: &Client) -> Self {
        Self {
            task_id: tp.task_id,
            validator_instance: client.validator_instance,
            cv_instance: client.cv_instance,
        }
    }
}
