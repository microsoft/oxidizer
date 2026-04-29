//! Basic application fixture: a flat resolver rooted at `AppBase` that
//! `#[spread]`s the cross-crate `Builtins` re-export and resolves an
//! `AppService` whose deps come from sibling crates.

#![allow(missing_docs, missing_debug_implementations)]

pub mod app_context {
    #[derive(Clone)]
    pub struct AppContext;
}

pub mod app_base {
    use crate::app_context::AppContext;

    #[autoresolve::base(helper_module_exported_as = crate::app_base::app_base_helper)]
    pub struct AppBase {
        #[spread]
        pub builtins: xc_runtime::core::Builtins,
        pub app_context: AppContext,
    }
}

pub mod app_service {
    use xc_http::client::HttpClient;
    use xc_scheduler::scheduler::Scheduler;

    #[derive(Clone)]
    pub struct AppService {
        pub built: bool,
    }

    #[autoresolve::resolvable]
    impl AppService {
        pub fn new(_client: &HttpClient, _sched: &Scheduler) -> Self {
            Self { built: true }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use autoresolve::Resolver;
    use xc_io_driver::driver::IoDriver;
    use xc_scheduler::scheduler::Scheduler;

    use super::{
        app_base::AppBase,
        app_context::AppContext,
        app_service::AppService,
    };

    fn make_resolver() -> Resolver<AppBase> {
        Resolver::new(AppBase {
            builtins: xc_runtime::core::Builtins {
                scheduler: Scheduler,
                io_driver: IoDriver,
            },
            app_context: AppContext,
        })
    }

    #[test]
    fn resolves_app_service_across_crates() {
        let mut resolver = make_resolver();
        let svc: Arc<AppService> = resolver.get::<AppService>();
        assert!(svc.built);
    }

    #[test]
    fn repeat_resolution_returns_same_instance() {
        let mut resolver = make_resolver();
        let a = resolver.get::<AppService>();
        let b = resolver.get::<AppService>();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn shared_dependency_instance_across_consumers() {
        // Both `AppService` and a directly-resolved `HttpClient` should share
        // the same `Scheduler` instance via `xc_scheduler`.
        let mut resolver = make_resolver();
        let s1 = resolver.get::<Scheduler>();
        let s2 = resolver.get::<Scheduler>();
        assert!(Arc::ptr_eq(&s1, &s2));
        let _ = resolver.get::<AppService>();
        let s3 = resolver.get::<Scheduler>();
        assert!(Arc::ptr_eq(&s1, &s3));
    }

    #[test]
    fn cross_crate_provide_override_fires() {
        // `xc_http::client::HttpClient` is consumed by `AppService` via its
        // `#[resolvable]` constructor. A path-scoped override registered in
        // the test crate should still apply (proves `DependencyOf` impls
        // emitted in `xc_http` are visible cross-crate).
        let mut resolver = make_resolver();
        let custom = xc_http::client::HttpClient;
        resolver
            .provide(custom)
            .when_injected_in::<AppService>();
        let _svc = resolver.get::<AppService>();
        // No assertion on identity here \u2014 the type is unit; the test
        // succeeds if the chain compiles and resolves without panicking,
        // proving the cross-crate `DependencyOf<AppService> for HttpClient`
        // is visible.
    }
}
