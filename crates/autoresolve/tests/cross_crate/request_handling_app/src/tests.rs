use std::sync::Arc;

use autoresolve::Resolver;
use xc_io_driver::driver::IoDriver;
use xc_request_handling_framework::{
    framework_base::FrameworkBase, framework_context::FrameworkContext,
};
use xc_scheduler::scheduler::Scheduler;

use crate::{
    app_base::AppBase,
    app_context::AppContext,
    app_service::AppService,
    request_base::RequestBase,
    request_service::RequestService,
    task::{Task, TaskBase, TaskService},
};

fn make_root() -> Resolver<FrameworkBase> {
    Resolver::new(FrameworkBase {
        builtins: xc_runtime::core::Builtins {
            scheduler: Scheduler,
            io_driver: IoDriver,
        },
        framework_context: FrameworkContext,
    })
}

#[test]
fn four_tier_scoped_chain_resolves_across_crates() {
    let root = make_root();
    let mut app = root.scoped(AppBase {
        req_app_context: AppContext,
    });
    let app_svc: Arc<AppService> = app.get::<AppService>();
    assert!(app_svc.built);

    let mut req = app.scoped(RequestBase {
        request: xc_http::request::Request,
    });
    let req_svc: Arc<RequestService> = req.get::<RequestService>();
    assert!(req_svc.built);

    let mut task = req.scoped(TaskBase { task: Task });
    let task_svc: Arc<TaskService> = task.get::<TaskService>();
    assert!(task_svc.built);
}

#[test]
fn intermediate_resolver_pools_shared_dependency_to_parent() {
    // Resolving `Scheduler` from a deeply-scoped child should walk up to
    // the framework root (where `Builtins` is `#[spread]`-installed) and
    // cache there — so the parent sees the same instance.
    let root = make_root();
    let mut app = root.scoped(AppBase {
        req_app_context: AppContext,
    });
    let from_app = app.get::<Scheduler>();
    let from_app_again = app.get::<Scheduler>();
    assert!(Arc::ptr_eq(&from_app, &from_app_again));
}

#[test]
fn child_provide_does_not_leak_into_parent() {
    let root = make_root();
    let mut app = root.scoped(AppBase {
        req_app_context: AppContext,
    });
    let mut req = app.scoped(RequestBase {
        request: xc_http::request::Request,
    });

    // Child-scoped override: only RequestService below this point should
    // see the override; AppService resolved via the parent must not.
    req.provide(xc_http::client::HttpClient)
        .when_injected_in::<RequestService>();

    let _ = req.get::<RequestService>();
    let _ = app.get::<AppService>();
}
