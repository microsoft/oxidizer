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
    fn new(cv: &CorrelationVector, tp: &TaskProperties) -> Self {
        Self { task_id: tp.task_id }
    }
}

#[base]
mod app_base {
    pub struct AppBase {
        pub scheduler: super::Scheduler,
    }
}

use app_base::AppBase;

#[base(scoped(super::app_base::AppBase))]
mod request_base {
    pub struct RequestBase {
        pub request: super::Request,
    }
}

use request_base::RequestBase;

#[base(scoped(super::request_base::RequestBase))]
mod task_base {
    pub struct TaskBase {
        pub task: super::Task,
    }
}

use task_base::TaskBase;

fn main() {
    let mut parent = autoresolve::Resolver::new(AppBase { scheduler: Scheduler });

    // TaskHandler depends on task-scoped types — this must not compile
    // from a Resolver<AppBase>.
    let _ = parent.get::<TaskHandler>();
}
