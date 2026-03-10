use autoresolve_macros::base;

pub mod task;

#[base(scoped(super::http::request_base::RequestBase))]
pub mod task_base {
    pub struct TaskBase {
        pub task: super::task::Task,
    }
}
