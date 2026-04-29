pub use crate::request_base::RequestBase;
use crate::request_service::RequestService;

#[derive(Clone)]
pub struct Task;

#[derive(Clone)]
pub struct TaskService {
    pub built: bool,
}

#[autoresolve::resolvable]
impl TaskService {
    fn new(_task_base: &Task, _req_service: &RequestService) -> Self {
        Self { built: true }
    }
}

#[autoresolve::base(
    scoped(RequestBase),
    helper_module_exported_as = crate::task::task_base_helper
)]
pub struct TaskBase {
    pub task: Task,
}
