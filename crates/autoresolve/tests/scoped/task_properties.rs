use autoresolve_macros::resolvable;

use super::task::task::Task;

/// Depends on Task (task-level). Captures the task id.
#[derive(Clone)]
pub struct TaskProperties {
    pub(crate) task_id: u64,
}

#[resolvable]
impl TaskProperties {
    fn new(task: &Task) -> Self {
        Self { task_id: task.id }
    }
}
