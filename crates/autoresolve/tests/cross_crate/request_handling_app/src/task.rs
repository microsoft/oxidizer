//! Task-tier base scoped on `RequestBase`, plus its resolvable service.

pub use crate::request_base::RequestBase;
use crate::request_service::RequestService;

/// Cross-crate fixture stand-in for a per-task value.
#[derive(Clone, Debug)]
pub struct Task;

/// Cross-crate fixture service resolved at the task tier.
#[derive(Clone, Debug)]
pub struct TaskService {
    /// Indicates that the constructor ran (always `true`).
    pub built: bool,
}

#[autoresolve::resolvable]
impl TaskService {
    fn new(_task_base: &Task, _req_service: &RequestService) -> Self {
        Self { built: true }
    }
}

/// Task-tier base. Scoped beneath [`RequestBase`].
#[derive(Debug)]
#[autoresolve::base(
    scoped(RequestBase),
    helper_module_exported_as = crate::task::task_base_helper
)]
pub struct TaskBase {
    /// Per-task value.
    pub task: Task,
}
