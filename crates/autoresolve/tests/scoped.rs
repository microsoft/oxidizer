#![allow(dead_code)] // Test structs exist to exercise the DI graph, not all fields are read.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use autoresolve_macros::{base, resolvable};

// =============================================================================
// Root types — one per scope level, each carrying a construction counter.
// =============================================================================

/// App-level root. The counter is used by [`Validator`] to stamp each instance.
#[derive(Clone)]
struct Scheduler {
    counter: Arc<AtomicUsize>,
}

/// Request-level root. The counter is used by [`CorrelationVector`] to stamp each instance.
#[derive(Clone)]
struct Request {
    counter: Arc<AtomicUsize>,
}

/// Task-level root. Each task carries a unique id.
#[derive(Clone)]
struct Task {
    id: u64,
}

// =============================================================================
// Single-scope dependencies — each captures the counter value at construction.
// =============================================================================

/// Depends on Scheduler (app-level). `instance` records which construction this was.
#[derive(Clone)]
struct Validator {
    instance: usize,
}

#[resolvable]
impl Validator {
    fn new(scheduler: &Scheduler) -> Self {
        Self {
            instance: scheduler.counter.fetch_add(1, Ordering::SeqCst) + 1,
        }
    }
}

/// Depends on Request (request-level). `instance` records which construction this was.
#[derive(Clone)]
struct CorrelationVector {
    instance: usize,
}

#[resolvable]
impl CorrelationVector {
    fn new(request: &Request) -> Self {
        Self {
            instance: request.counter.fetch_add(1, Ordering::SeqCst) + 1,
        }
    }
}

/// Depends on Task (task-level). Captures the task id.
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

// =============================================================================
// Cross-scope dependencies — combine objects from different levels.
// =============================================================================

/// App + Request: depends on Validator and CorrelationVector.
#[derive(Clone)]
struct Client {
    validator_instance: usize,
    cv_instance: usize,
}

#[resolvable]
impl Client {
    fn new(validator: &Validator, cv: &CorrelationVector) -> Self {
        Self {
            validator_instance: validator.instance,
            cv_instance: cv.instance,
        }
    }
}

/// Request + Task: depends on CorrelationVector and TaskProperties.
#[derive(Clone)]
struct TaskHandler {
    cv_instance: usize,
    task_id: u64,
}

#[resolvable]
impl TaskHandler {
    fn new(cv: &CorrelationVector, tp: &TaskProperties) -> Self {
        Self {
            cv_instance: cv.instance,
            task_id: tp.task_id,
        }
    }
}

/// Task + App (via Client): depends on TaskProperties and Client.
#[derive(Clone)]
struct TaskClient {
    task_id: u64,
    validator_instance: usize,
    cv_instance: usize,
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

// =============================================================================
// Base types — three-level hierarchy: App → Request → Task.
// =============================================================================

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

// =============================================================================
// Helpers
// =============================================================================

fn app(counter: Arc<AtomicUsize>) -> autoresolve::Resolver<AppBase> {
    autoresolve::Resolver::new(AppBase {
        scheduler: Scheduler { counter },
    })
}

// =============================================================================
// Tests
// =============================================================================

/// Two-level: children inherit types eagerly resolved in the parent.
#[test]
fn parent_types_inherited_by_children() {
    let counter = Arc::new(AtomicUsize::new(0));
    let mut parent = app(counter.clone());
    parent.get::<Validator>(); // eagerly resolve in parent
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    let shared = parent.into_shared();

    let mut child1 = shared.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });
    let mut child2 = shared.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });

    // Both children see the same Validator instance from the parent.
    assert_eq!(child1.get::<Validator>().instance, 1);
    assert_eq!(child2.get::<Validator>().instance, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// Two-level: children resolve missing parent types locally and those get
/// promoted to the shared parent so siblings share them.
#[test]
fn auto_promotes_to_parent() {
    let counter = Arc::new(AtomicUsize::new(0));
    let parent = app(counter.clone()); // Validator NOT eagerly resolved
    let shared = parent.into_shared();

    let mut child1 = shared.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });
    // child1 resolves Validator — deps all come from parent, so it's promoted.
    assert_eq!(child1.get::<Validator>().instance, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    let mut child2 = shared.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });
    // child2 finds Validator already promoted — no second construction.
    assert_eq!(child2.get::<Validator>().instance, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// Two-level: request-scoped types are independent across siblings.
#[test]
fn child_types_are_independent() {
    let counter = Arc::new(AtomicUsize::new(0));
    let parent = app(counter);
    let shared = parent.into_shared();

    let req_counter1 = Arc::new(AtomicUsize::new(0));
    let req_counter2 = Arc::new(AtomicUsize::new(0));

    let mut child1 = shared.scoped(RequestBase {
        request: Request { counter: req_counter1 },
    });
    let mut child2 = shared.scoped(RequestBase {
        request: Request { counter: req_counter2 },
    });

    // Each child constructs its own CorrelationVector from its own Request.
    assert_eq!(child1.get::<CorrelationVector>().instance, 1);
    assert_eq!(child2.get::<CorrelationVector>().instance, 1);
}

/// Two-level: try_get checks local and ancestor stores.
#[test]
fn try_get_checks_both_stores() {
    let counter = Arc::new(AtomicUsize::new(0));
    let mut parent = app(counter);
    parent.get::<Validator>(); // eagerly resolve

    let shared = parent.into_shared();
    let mut child = shared.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });

    // Validator is in parent.
    assert!(child.try_get::<Validator>().is_some());
    // Request is in child.
    assert!(child.try_get::<Request>().is_some());
    // CorrelationVector is in neither — not yet resolved.
    assert!(child.try_get::<CorrelationVector>().is_none());
    // After resolving, it's available.
    child.get::<CorrelationVector>();
    assert!(child.try_get::<CorrelationVector>().is_some());
}

/// Two-level: Client (depends on Validator + CorrelationVector) — the Validator
/// half comes from the parent, the CorrelationVector half is request-scoped.
/// Sibling children share the Validator instance but have distinct CVs.
#[test]
fn client_shares_validator_across_siblings() {
    let counter = Arc::new(AtomicUsize::new(0));
    let parent = app(counter.clone());
    let shared = parent.into_shared();

    let mut child1 = shared.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });
    let mut child2 = shared.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });

    let c1 = child1.get::<Client>();
    let c2 = child2.get::<Client>();

    // Same Validator instance (promoted to parent), different CV instances.
    assert_eq!(c1.validator_instance, c2.validator_instance);
    assert_eq!(c1.cv_instance, 1);
    assert_eq!(c2.cv_instance, 1);
    // Only one Validator construction.
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// Three-level (app → request → task): Validator (app-dep only) is promoted
/// all the way to the app ancestor. Sibling requests and tasks share it.
#[test]
fn three_level_promotes_validator_to_app() {
    let counter = Arc::new(AtomicUsize::new(0));
    let parent = app(counter.clone());
    let shared = parent.into_shared();

    // Request 1
    let req1 = shared.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });
    let req1_shared = req1.into_shared();

    let mut task1a = req1_shared.scoped(TaskBase { task: Task { id: 100 } });
    assert_eq!(task1a.get::<Validator>().instance, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    let mut task1b = req1_shared.scoped(TaskBase { task: Task { id: 200 } });
    assert_eq!(task1b.get::<Validator>().instance, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // Request 2 (sibling) — still reuses from app ancestor.
    let req2 = shared.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });
    let req2_shared = req2.into_shared();

    let mut task2a = req2_shared.scoped(TaskBase { task: Task { id: 300 } });
    assert_eq!(task2a.get::<Validator>().instance, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// Three-level: CorrelationVector (request-dep only) is promoted to the request
/// ancestor. Sibling tasks share it, but a different request gets its own.
#[test]
fn three_level_promotes_cv_to_request() {
    let counter = Arc::new(AtomicUsize::new(0));
    let parent = app(counter);
    let shared = parent.into_shared();

    let req_counter1 = Arc::new(AtomicUsize::new(0));
    let req_counter2 = Arc::new(AtomicUsize::new(0));

    // Request 1
    let req1 = shared.scoped(RequestBase {
        request: Request {
            counter: req_counter1.clone(),
        },
    });
    let req1_shared = req1.into_shared();

    let mut task1a = req1_shared.scoped(TaskBase { task: Task { id: 100 } });
    assert_eq!(task1a.get::<CorrelationVector>().instance, 1);
    assert_eq!(req_counter1.load(Ordering::SeqCst), 1);

    let mut task1b = req1_shared.scoped(TaskBase { task: Task { id: 200 } });
    assert_eq!(task1b.get::<CorrelationVector>().instance, 1);
    assert_eq!(req_counter1.load(Ordering::SeqCst), 1);

    // Request 2 — must construct its own CorrelationVector.
    let req2 = shared.scoped(RequestBase {
        request: Request {
            counter: req_counter2.clone(),
        },
    });
    let req2_shared = req2.into_shared();

    let mut task2a = req2_shared.scoped(TaskBase { task: Task { id: 300 } });
    assert_eq!(task2a.get::<CorrelationVector>().instance, 1);
    assert_eq!(req_counter2.load(Ordering::SeqCst), 1);
}

/// Three-level: TaskHandler (depends on CV + TaskProperties) stays local because
/// it depends on task-scoped data. Each task gets its own, but they all share
/// the same CorrelationVector instance within a request.
#[test]
fn three_level_task_handler_local_but_shares_cv() {
    let counter = Arc::new(AtomicUsize::new(0));
    let parent = app(counter);
    let shared = parent.into_shared();

    let req_counter = Arc::new(AtomicUsize::new(0));
    let req1 = shared.scoped(RequestBase {
        request: Request {
            counter: req_counter.clone(),
        },
    });
    let req1_shared = req1.into_shared();

    let mut task1 = req1_shared.scoped(TaskBase { task: Task { id: 10 } });
    let mut task2 = req1_shared.scoped(TaskBase { task: Task { id: 20 } });

    let th1 = task1.get::<TaskHandler>();
    let th2 = task2.get::<TaskHandler>();

    // Different task ids, but same CV instance (promoted to request scope).
    assert_eq!(th1.task_id, 10);
    assert_eq!(th2.task_id, 20);
    assert_eq!(th1.cv_instance, th2.cv_instance);
    assert_eq!(req_counter.load(Ordering::SeqCst), 1);
}

/// Three-level: TaskClient (depends on TaskProperties + Client) — Client is
/// promoted to the request scope (it depends on both app and request data).
/// Each task gets its own TaskClient, but all share the same Client within a
/// request. Different requests see the same Validator but different CVs.
#[test]
fn three_level_task_client_shares_client_within_request() {
    let app_counter = Arc::new(AtomicUsize::new(0));
    let parent = app(app_counter.clone());
    let shared = parent.into_shared();

    let req_counter1 = Arc::new(AtomicUsize::new(0));
    // Start at 100 so instance values from different requests are distinguishable.
    let req_counter2 = Arc::new(AtomicUsize::new(100));

    // Request 1
    let req1 = shared.scoped(RequestBase {
        request: Request {
            counter: req_counter1.clone(),
        },
    });
    let req1_shared = req1.into_shared();

    let mut task1a = req1_shared.scoped(TaskBase { task: Task { id: 10 } });
    let mut task1b = req1_shared.scoped(TaskBase { task: Task { id: 20 } });

    let tc1a = task1a.get::<TaskClient>();
    let tc1b = task1b.get::<TaskClient>();

    // Same validator + CV across tasks within request 1.
    assert_eq!(tc1a.validator_instance, tc1b.validator_instance);
    assert_eq!(tc1a.cv_instance, tc1b.cv_instance);
    // Different task ids.
    assert_eq!(tc1a.task_id, 10);
    assert_eq!(tc1b.task_id, 20);
    // Only one Validator and one CV constructed.
    assert_eq!(app_counter.load(Ordering::SeqCst), 1);
    assert_eq!(req_counter1.load(Ordering::SeqCst), 1);

    // Request 2
    let req2 = shared.scoped(RequestBase {
        request: Request {
            counter: req_counter2.clone(),
        },
    });
    let req2_shared = req2.into_shared();

    let mut task2a = req2_shared.scoped(TaskBase { task: Task { id: 30 } });
    let tc2a = task2a.get::<TaskClient>();

    // Same Validator (promoted to app), but different CV (request-scoped).
    assert_eq!(tc2a.validator_instance, tc1a.validator_instance);
    assert_ne!(tc2a.cv_instance, tc1a.cv_instance);
    assert_eq!(tc2a.task_id, 30);
    // Still only one Validator total, but now two CVs (one per request).
    assert_eq!(app_counter.load(Ordering::SeqCst), 1);
    assert_eq!(req_counter2.load(Ordering::SeqCst), 101);
}
