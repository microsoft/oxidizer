use autoresolve_macros::base;

use super::app_base;
use super::http::request_base;

pub mod task;

#[base(scoped(super::request_base::RequestBase))]
pub mod task_base {
    pub struct TaskBase {
        pub task: super::task::Task,
    }
}
