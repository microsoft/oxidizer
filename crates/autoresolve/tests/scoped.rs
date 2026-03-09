#![allow(dead_code)] // Test structs exist to exercise the DI graph, not all fields are read.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use autoresolve_macros::base;

// Each type lives in its own module (separate file) so the generated code must
// resolve paths across module boundaries — validating that `#[base]` and
// `#[resolvable]` produce correct impls even when not all types are in scope at
// the usage site.

// =============================================================================
// Root types — one per scope level, each carrying a construction counter.
// =============================================================================

mod request;
mod scheduler;
mod task;

// =============================================================================
// Single-scope dependencies — each captures the counter value at construction.
// =============================================================================

mod correlation_vector;
mod task_properties;
mod validator;

// =============================================================================
// Cross-scope dependencies — combine objects from different levels.
// =============================================================================

mod client;
mod task_client;
mod task_handler;

// =============================================================================
// Base types — three-level hierarchy: App → Request → Task.
// =============================================================================

#[base]
mod app_base {
    pub struct AppBase {
        pub scheduler: super::scheduler::Scheduler,
    }
}

use app_base::AppBase;

#[base(scoped(super::app_base::AppBase))]
mod request_base {
    pub struct RequestBase {
        pub request: super::request::Request,
    }
}

use request_base::RequestBase;

#[base(scoped(super::request_base::RequestBase))]
mod task_base {
    pub struct TaskBase {
        pub task: super::task::Task,
    }
}

// Convenience imports for test readability.
use client::Client;
use correlation_vector::CorrelationVector;
use request::Request;
use task::Task;
use task_base::TaskBase;
use task_client::TaskClient;
use task_handler::TaskHandler;
use validator::Validator;

// =============================================================================
// Helpers
// =============================================================================

fn app(counter: Arc<AtomicUsize>) -> autoresolve::Resolver<AppBase> {
    autoresolve::Resolver::new(AppBase {
        scheduler: scheduler::Scheduler { counter },
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

    let mut child1 = parent.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });
    let mut child2 = parent.scoped(RequestBase {
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

    let mut child1 = parent.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });
    // child1 resolves Validator — deps all come from parent, so it's promoted.
    assert_eq!(child1.get::<Validator>().instance, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    let mut child2 = parent.scoped(RequestBase {
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

    let req_counter1 = Arc::new(AtomicUsize::new(0));
    let req_counter2 = Arc::new(AtomicUsize::new(0));

    let mut child1 = parent.scoped(RequestBase {
        request: Request { counter: req_counter1 },
    });
    let mut child2 = parent.scoped(RequestBase {
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

    let mut child = parent.scoped(RequestBase {
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

    let mut child1 = parent.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });
    let mut child2 = parent.scoped(RequestBase {
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

    // Request 1
    let req1 = parent.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });

    let mut task1a = req1.scoped(TaskBase { task: Task { id: 100 } });
    assert_eq!(task1a.get::<Validator>().instance, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    let mut task1b = req1.scoped(TaskBase { task: Task { id: 200 } });
    assert_eq!(task1b.get::<Validator>().instance, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // Request 2 (sibling) — still reuses from app ancestor.
    let req2 = parent.scoped(RequestBase {
        request: Request {
            counter: Arc::new(AtomicUsize::new(0)),
        },
    });

    let mut task2a = req2.scoped(TaskBase { task: Task { id: 300 } });
    assert_eq!(task2a.get::<Validator>().instance, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// Three-level: CorrelationVector (request-dep only) is promoted to the request
/// ancestor. Sibling tasks share it, but a different request gets its own.
#[test]
fn three_level_promotes_cv_to_request() {
    let counter = Arc::new(AtomicUsize::new(0));
    let parent = app(counter);

    let req_counter1 = Arc::new(AtomicUsize::new(0));
    let req_counter2 = Arc::new(AtomicUsize::new(0));

    // Request 1
    let req1 = parent.scoped(RequestBase {
        request: Request {
            counter: req_counter1.clone(),
        },
    });

    let mut task1a = req1.scoped(TaskBase { task: Task { id: 100 } });
    assert_eq!(task1a.get::<CorrelationVector>().instance, 1);
    assert_eq!(req_counter1.load(Ordering::SeqCst), 1);

    let mut task1b = req1.scoped(TaskBase { task: Task { id: 200 } });
    assert_eq!(task1b.get::<CorrelationVector>().instance, 1);
    assert_eq!(req_counter1.load(Ordering::SeqCst), 1);

    // Request 2 — must construct its own CorrelationVector.
    let req2 = parent.scoped(RequestBase {
        request: Request {
            counter: req_counter2.clone(),
        },
    });

    let mut task2a = req2.scoped(TaskBase { task: Task { id: 300 } });
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

    let req_counter = Arc::new(AtomicUsize::new(0));
    let req1 = parent.scoped(RequestBase {
        request: Request {
            counter: req_counter.clone(),
        },
    });

    let mut task1 = req1.scoped(TaskBase { task: Task { id: 10 } });
    let mut task2 = req1.scoped(TaskBase { task: Task { id: 20 } });

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

    let req_counter1 = Arc::new(AtomicUsize::new(0));
    // Start at 100 so instance values from different requests are distinguishable.
    let req_counter2 = Arc::new(AtomicUsize::new(100));

    // Request 1
    let req1 = parent.scoped(RequestBase {
        request: Request {
            counter: req_counter1.clone(),
        },
    });

    let mut task1a = req1.scoped(TaskBase { task: Task { id: 10 } });
    let mut task1b = req1.scoped(TaskBase { task: Task { id: 20 } });

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
    let req2 = parent.scoped(RequestBase {
        request: Request {
            counter: req_counter2.clone(),
        },
    });

    let mut task2a = req2.scoped(TaskBase { task: Task { id: 30 } });
    let tc2a = task2a.get::<TaskClient>();

    // Same Validator (promoted to app), but different CV (request-scoped).
    assert_eq!(tc2a.validator_instance, tc1a.validator_instance);
    assert_ne!(tc2a.cv_instance, tc1a.cv_instance);
    assert_eq!(tc2a.task_id, 30);
    // Still only one Validator total, but now two CVs (one per request).
    assert_eq!(app_counter.load(Ordering::SeqCst), 1);
    assert_eq!(req_counter2.load(Ordering::SeqCst), 101);
}
