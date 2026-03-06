use autoresolve_macros::{base, resolvable};

#[derive(Clone)]
struct Scheduler;

#[derive(Clone)]
struct Request;

#[derive(Clone)]
struct Task {
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
struct AppBase {
    scheduler: Scheduler,
}

#[base(scoped(AppBase))]
struct RequestBase {
    request: Request,
}

#[base(scoped(RequestBase))]
struct TaskBase {
    task: Task,
}

fn main() {
    let mut parent = autoresolve::Resolver::new(AppBase { scheduler: Scheduler });

    // TaskHandler depends on task-scoped types — this must not compile
    // from a Resolver<AppBase>.
    let _ = parent.get::<TaskHandler>();
}
