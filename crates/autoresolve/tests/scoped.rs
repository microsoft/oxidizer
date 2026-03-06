#![allow(dead_code)] // Test structs exist to exercise the DI graph, not all fields are read.

use std::sync::atomic::{AtomicUsize, Ordering};

use autoresolve_macros::{base, composite, resolvable};

// --- "Global" types (application lifetime, shared via parent) ---

#[derive(Clone)]
pub struct Scheduler;

impl Scheduler {
    fn number(&self) -> i32 {
        42
    }
}

#[derive(Clone)]
pub struct Clock;

impl Clock {
    fn number(&self) -> i32 {
        10
    }
}

#[composite(builtins)]
mod builtins {
    #[derive(Clone)]
    pub struct Builtins {
        pub scheduler: super::Scheduler,
        pub clock: super::Clock,
    }
}

use builtins::Builtins;

#[derive(Clone)]
struct Validator {
    scheduler: Scheduler,
}

#[resolvable]
impl Validator {
    fn new(scheduler: &Scheduler) -> Self {
        Self {
            scheduler: scheduler.clone(),
        }
    }

    fn number(&self) -> i32 {
        self.scheduler.number()
    }
}

#[derive(Clone)]
struct Client {
    validator: Validator,
    clock: Clock,
}

#[resolvable]
impl Client {
    fn new(validator: &Validator, clock: &Clock) -> Self {
        Self {
            validator: validator.clone(),
            clock: clock.clone(),
        }
    }

    fn number(&self) -> i32 {
        self.validator.number() + self.clock.number()
    }
}

// --- "Request-scoped" types (per-request lifetime, in child resolver) ---

#[derive(Clone)]
struct RequestContext {
    request_id: u64,
}

#[derive(Clone)]
struct CorrelationVector {
    request_id: u64,
}

#[resolvable]
impl CorrelationVector {
    fn new(ctx: &RequestContext) -> Self {
        Self {
            request_id: ctx.request_id,
        }
    }
}

struct RequestHandler {
    client: Client,
    correlation_vector: CorrelationVector,
}

#[resolvable]
impl RequestHandler {
    fn new(client: &Client, correlation_vector: &CorrelationVector) -> Self {
        Self {
            client: client.clone(),
            correlation_vector: correlation_vector.clone(),
        }
    }
}

// --- Base type and scoped roots declared via #[base] macros ---

#[base]
struct Base {
    #[spread]
    builtins: Builtins,
}

#[base(scoped(Base))]
struct ScopedRoots {
    request_context: RequestContext,
}

// --- "Task-scoped" types (per-task within a request, grandchild resolver) ---

#[derive(Clone)]
struct Task {
    task_id: u64,
}

#[base(scoped(Base))]
struct TaskScopedRoots {
    task: Task,
}

struct TaskScopedClient {
    request_id: u64,
    task_id: u64,
    client_number: i32,
}

#[resolvable]
impl TaskScopedClient {
    fn new(handler: &RequestHandler, task: &Task) -> Self {
        Self {
            request_id: handler.correlation_vector.request_id,
            task_id: task.task_id,
            client_number: handler.client.number(),
        }
    }
}

static COUNTED_CLIENT_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
struct CountedClient {
    number: i32,
}

#[resolvable]
impl CountedClient {
    fn new(validator: &Validator, clock: &Clock) -> Self {
        let count = COUNTED_CLIENT_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst) + 1;
        Self {
            number: validator.number() + clock.number() + count as i32,
        }
    }
}

fn create_parent(builtins: Builtins) -> autoresolve::Resolver<Base> {
    autoresolve::Resolver::new(Base { builtins })
}

#[test]
fn scoped_resolver_inherits_parent_types() {
    let builtins = Builtins {
        scheduler: Scheduler,
        clock: Clock,
    };

    // Create parent resolver with global types and eagerly resolve a global singleton.
    let mut parent = create_parent(builtins);
    parent.get::<Client>(); // eagerly resolve Client (and its deps) in the parent

    let shared = parent.into_shared();

    // Scoped child for request 1
    let mut child1 = shared.scoped();
    child1.insert(RequestContext { request_id: 1 });
    let handler1 = child1.get::<RequestHandler>();
    assert_eq!(handler1.correlation_vector.request_id, 1);
    // Client was resolved in parent — child reads it from there.
    assert_eq!(handler1.client.number(), 42 + 10);

    // Scoped child for request 2
    let mut child2 = shared.scoped();
    child2.insert(RequestContext { request_id: 2 });
    let handler2 = child2.get::<RequestHandler>();
    assert_eq!(handler2.correlation_vector.request_id, 2);
    assert_eq!(handler2.client.number(), 42 + 10);
}

#[test]
fn scoped_resolver_resolves_missing_parent_types_locally() {
    let builtins = Builtins {
        scheduler: Scheduler,
        clock: Clock,
    };

    // Parent only has root types — Client is NOT eagerly resolved.
    let parent = create_parent(builtins);
    let shared = parent.into_shared();

    let mut child = shared.scoped();
    child.insert(RequestContext { request_id: 42 });

    // Client wasn't in parent, but its deps (Scheduler, Clock) are.
    // The child resolves Client locally using parent's root types.
    let handler = child.get::<RequestHandler>();
    assert_eq!(handler.correlation_vector.request_id, 42);
    assert_eq!(handler.client.number(), 42 + 10);
}

#[test]
fn scoped_resolver_child_types_are_independent() {
    let builtins = Builtins {
        scheduler: Scheduler,
        clock: Clock,
    };

    let parent = create_parent(builtins);
    let shared = parent.into_shared();

    // Two children with different request contexts.
    let mut child1 = shared.scoped();
    child1.insert(RequestContext { request_id: 100 });

    let mut child2 = shared.scoped();
    child2.insert(RequestContext { request_id: 200 });

    // Each child resolves its own CorrelationVector from its own RequestContext.
    let cv1 = child1.get::<CorrelationVector>();
    let cv2 = child2.get::<CorrelationVector>();

    assert_eq!(cv1.request_id, 100);
    assert_eq!(cv2.request_id, 200);
}

#[test]
fn scoped_resolver_try_get_checks_both_stores() {
    let builtins = Builtins {
        scheduler: Scheduler,
        clock: Clock,
    };

    let mut parent = create_parent(builtins);
    parent.get::<Validator>(); // eagerly resolve Validator in parent

    let shared = parent.into_shared();
    let mut child = shared.scoped();
    child.insert(RequestContext { request_id: 1 });

    // Validator is in parent — try_get should find it.
    assert!(child.try_get::<Validator>().is_some());
    assert_eq!(child.try_get::<Validator>().unwrap().number(), 42);

    // RequestContext is in child — try_get should find it.
    assert!(child.try_get::<RequestContext>().is_some());

    // CorrelationVector is in neither — try_get returns None.
    assert!(child.try_get::<CorrelationVector>().is_none());

    // After resolving, try_get finds it in child.
    child.get::<CorrelationVector>();
    assert!(child.try_get::<CorrelationVector>().is_some());
}

#[test]
fn scoped_resolver_shares_parent_resolved_types() {
    COUNTED_CLIENT_CONSTRUCTIONS.store(0, Ordering::SeqCst);

    let builtins = Builtins {
        scheduler: Scheduler,
        clock: Clock,
    };

    let mut parent = create_parent(builtins);
    parent.get::<CountedClient>(); // eagerly resolve once in parent
    assert_eq!(COUNTED_CLIENT_CONSTRUCTIONS.load(Ordering::SeqCst), 1);

    let shared = parent.into_shared();

    // Two scoped children both access CountedClient — should NOT construct it again.
    let mut child1 = shared.scoped();
    child1.insert(RequestContext { request_id: 1 });
    let c1 = child1.get::<CountedClient>();
    assert_eq!(c1.number, 42 + 10 + 1);

    let mut child2 = shared.scoped();
    child2.insert(RequestContext { request_id: 2 });
    let c2 = child2.get::<CountedClient>();
    assert_eq!(c2.number, 42 + 10 + 1);

    // Still only one construction — both children read from the shared parent.
    assert_eq!(COUNTED_CLIENT_CONSTRUCTIONS.load(Ordering::SeqCst), 1);
}

#[test]
fn scoped_resolver_supports_nested_scoping() {
    let builtins = Builtins {
        scheduler: Scheduler,
        clock: Clock,
    };

    // Level 0: application scope
    let parent = create_parent(builtins);
    let shared = parent.into_shared();

    // Level 1: request scope
    let mut request_scope = shared.scoped();
    request_scope.insert(RequestContext { request_id: 7 });
    request_scope.get::<RequestHandler>(); // eagerly resolve in request scope

    let request_shared = request_scope.into_shared();

    // Level 2: task scopes within the request
    let mut task1 = request_shared.scoped();
    task1.insert(Task { task_id: 100 });
    let tsc1 = task1.get::<TaskScopedClient>();
    assert_eq!(tsc1.request_id, 7);
    assert_eq!(tsc1.task_id, 100);
    assert_eq!(tsc1.client_number, 42 + 10);

    let mut task2 = request_shared.scoped();
    task2.insert(Task { task_id: 200 });
    let tsc2 = task2.get::<TaskScopedClient>();
    assert_eq!(tsc2.request_id, 7);
    assert_eq!(tsc2.task_id, 200);
    assert_eq!(tsc2.client_number, 42 + 10);
}
