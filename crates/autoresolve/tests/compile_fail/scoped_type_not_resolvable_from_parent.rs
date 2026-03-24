use autoresolve_macros::{base, resolvable};

#[derive(Clone)]
pub struct Scheduler;

#[derive(Clone)]
pub struct Request;

#[derive(Clone)]
pub struct Task {
    id: u64,
}

#[derive(Clone)]
struct CorrelationVector;

#[resolvable]
impl CorrelationVector {
    fn new(_request: &Request) -> Self {
        Self
    }
}

#[derive(Clone)]
struct TaskProperties {
    task_id: u64,
}

#[resolvable]
impl TaskProperties {
    fn new(task: &Task) -> Self {
        Self { task_id: task.id }
    }
}

/// Depends on request-scoped CorrelationVector and task-scoped TaskProperties.
/// Should only be resolvable from a task scope, never from the app scope.
#[derive(Clone)]
struct TaskHandler {
    task_id: u64,
}

#[resolvable]
impl TaskHandler {
    fn new(_cv: &CorrelationVector, tp: &TaskProperties) -> Self {
        Self { task_id: tp.task_id }
    }
}

#[base(helper_module_exported_as = crate::app_base_helper)]
pub struct AppBase {
    pub scheduler: Scheduler,
}

pub use crate::AppBase;

#[base(scoped(AppBase), helper_module_exported_as = crate::request_base_helper)]
pub struct RequestBase {
    pub request: Request,
}

pub use crate::RequestBase;

#[base(scoped(RequestBase), helper_module_exported_as = crate::task_base_helper)]
pub struct TaskBase {
    pub task: Task,
}

fn main() {
    let mut parent = autoresolve::Resolver::new(AppBase { scheduler: Scheduler });

    // TaskHandler depends on task-scoped types — this must not compile
    // from a Resolver<AppBase>.
    let _ = parent.get::<TaskHandler>();
}
