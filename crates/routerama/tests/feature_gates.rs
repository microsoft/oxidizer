// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Both directions of every additive feature boundary.
//!
//! The workspace test command enables all features, so a `compile_fail` case
//! that depends on a feature being *absent* is silently compiled out there.
//! Those diagnostics therefore live in this one target, where every feature
//! selection asserts one direction or the other:
//!
//! - with the feature on, the module and its named public types exist; and
//! - with the feature off, naming them produces a diagnostic that says which
//!   Cargo feature is missing.
//!
//! Run the feature-off direction explicitly, because `--all-features` proves
//! only the positive half:
//!
//! ```text
//! cargo test -p routerama --no-default-features --features response --test feature_gates
//! cargo test -p routerama --no-default-features --features route    --test feature_gates
//! cargo test -p routerama --all-features                            --test feature_gates
//! ```
//!
//! The first selection pins the `route`-off diagnostic, the second pins the
//! `mount`-off and `tower`-off diagnostics, and the third proves each module
//! actually appears once its feature is enabled.
//!
//! Each feature-off case names the gated module through a plain `use`, because
//! that diagnostic renders identically on every toolchain. A case routed
//! through macro expansion instead (for example `#[router(state = (),
//! erased_mounts)]` without `mount`) fails through the same gated re-export,
//! but its rendered path and follow-on inference errors differ between stable
//! and nightly, so it cannot be snapshotted.

/// Without `route`, `routerama::route` does not exist and says so.
#[cfg(not(feature = "route"))]
#[test]
#[cfg_attr(miri, ignore)]
fn route_paths_require_the_route_feature() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/response_without_route.rs");
}

/// Without `mount`, `routerama::route::mount` does not exist and says so.
#[cfg(all(feature = "route", not(feature = "mount")))]
#[test]
#[cfg_attr(miri, ignore)]
fn mount_paths_require_the_mount_feature() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/mount_without_feature.rs");
}

/// Without `tower`, `routerama::route::tower` does not exist and says so.
#[cfg(all(feature = "route", not(feature = "tower")))]
#[test]
#[cfg_attr(miri, ignore)]
fn tower_paths_require_the_tower_feature() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/tower_without_feature.rs");
    tests.compile_fail("tests/ui/tower_attribute_without_feature.rs");
}

/// The `route` feature exposes generated dispatch over the `response` types.
#[cfg(feature = "route")]
#[tokio::test]
async fn route_feature_exposes_generated_dispatch() {
    use routerama::response::Body;
    use routerama::route::{Request, StatusCode, router};

    #[derive(Clone, Copy)]
    struct Api;

    #[router]
    impl Api {
        #[route(GET, "/health")]
        async fn health(&self) -> StatusCode {
            core::future::ready(()).await;
            StatusCode::NO_CONTENT
        }
    }

    let response = Api
        .route(Request::get("/health").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

/// The `mount` feature exposes the explicitly erased runtime router.
#[cfg(feature = "mount")]
#[test]
fn mount_feature_exposes_the_erased_mount_router() {
    use routerama::response::Body;
    use routerama::route::mount::{ErasedMountRouter, ErasedMountService, MountedRequest};

    // Naming the public types is the assertion: without the feature the paths
    // below do not resolve, which is what the companion case pins.
    type Router = ErasedMountRouter<Body, ()>;
    type Service = ErasedMountService<Body, ()>;
    type MountRequest<'mount> = MountedRequest<'mount, Body>;

    let router: Router = ErasedMountRouter::builder()
        .mount(
            "GET",
            "/mounted",
            Service::from_async_fn(async |_request: MountRequest<'_>, (): &()| routerama::route::StatusCode::NO_CONTENT),
        )
        .build()
        .expect("the mounted template is valid");
    let _ = router;
}

/// The `tower` feature exposes the transport adapter.
#[cfg(feature = "tower")]
#[test]
fn tower_feature_exposes_the_route_service() {
    use routerama::response::{Body, Response};
    use routerama::route::tower::RouteService;
    use routerama::route::{Request, router};

    #[derive(Clone, Copy)]
    struct Api;

    #[router(tower)]
    impl Api {
        #[route(GET, "/")]
        async fn home(&self) -> Response {
            core::future::ready(()).await;
            Response::new(Body::empty())
        }
    }

    let service = RouteService::new((), (), |(): (), (): (), _request: Request<Body>| async {
        Response::new(Body::empty())
    });
    let _ = service.send_boxed_body();
    let _ = Api::tower_service::<Body, (), _, _>(Api, ());
}
