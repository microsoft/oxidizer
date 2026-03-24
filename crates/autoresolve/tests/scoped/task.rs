use autoresolve_macros::base;

pub mod task;

use task::Task;

pub use crate::http::RequestBase;

#[base(scoped(RequestBase), helper_module_exported_as = crate::task::task_base_helper)]
pub struct TaskBase {
    pub task: Task,
}
