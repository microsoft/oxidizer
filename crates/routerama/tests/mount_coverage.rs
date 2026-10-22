// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Debug/Default/Clone boilerplate and fallback-construction paths for the
//! mount router types, which the main `mount.rs` test file exercises through
//! behavior rather than through their standalone trait impls.

#![cfg(feature = "mount")]

use http::StatusCode;
use routerama::response::{Body, BoxBody, Response, SendBoxBody};
use routerama::route::Request;
use routerama::route::mount::{
    ErasedMountRouter, ErasedMountService, MountedCaptureError, MountedRequest, SendErasedMountRouter, SendErasedMountService,
    SendMountedService,
};

struct NoContent;

impl SendMountedService<Body, ()> for NoContent {
    type Response = StatusCode;

    async fn call<'a>(&'a self, _request: MountedRequest<'a, Body>, _state: &'a ()) -> Self::Response
    where
        Body: 'a,
    {
        core::future::ready(()).await;
        StatusCode::NO_CONTENT
    }
}

fn erased_fallback(status: StatusCode) -> Response<BoxBody> {
    let mut response = Response::new(BoxBody::new(Body::empty()));
    *response.status_mut() = status;
    response
}

fn send_fallback(status: StatusCode) -> Response<SendBoxBody> {
    let mut response = Response::new(SendBoxBody::new(Body::empty()));
    *response.status_mut() = status;
    response
}

#[tokio::test(flavor = "current_thread")]
async fn erased_mount_types_expose_non_exhaustive_debug_impls() {
    let service = ErasedMountService::<Body, ()>::from_async_fn(async |request: MountedRequest<'_, Body>, _state: &()| {
        assert!(format!("{request:?}").contains("MountedRequest"));
        StatusCode::NO_CONTENT
    });
    assert!(format!("{service:?}").contains("ErasedMountService"));

    let builder = ErasedMountRouter::<Body, ()>::builder().mount("GET", "/items", service);
    assert!(format!("{builder:?}").contains("ErasedMountRouterBuilder"));

    let mounts = builder.build().expect("mount registration is valid");
    assert!(format!("{mounts:?}").contains("ErasedMountRouter"));

    let response = mounts
        .route(Request::get("/items").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test(flavor = "current_thread")]
async fn cloning_a_mount_router_shares_the_route_table() {
    let service =
        ErasedMountService::<Body, ()>::from_async_fn(async |_request: MountedRequest<'_, Body>, _state: &()| StatusCode::NO_CONTENT);
    let mounts = ErasedMountRouter::<Body, ()>::builder()
        .mount("GET", "/items", service)
        .build()
        .expect("mount registration is valid");

    let cloned = mounts.clone();
    for router in [&mounts, &cloned] {
        let response = router
            .route(Request::get("/items").body(Body::empty()).expect("valid request"), &())
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn erased_mount_router_supports_a_custom_fallback_builder() {
    let mounts = ErasedMountRouter::<Body, ()>::builder_with_fallback(erased_fallback)
        .build()
        .expect("an empty mount table is valid");
    let response = mounts
        .route(Request::get("/missing").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn send_erased_mount_types_expose_non_exhaustive_debug_and_clone_impls() {
    let service = SendErasedMountService::<Body, ()>::new(NoContent);
    let cloned = service.clone();
    assert!(format!("{cloned:?}").contains("SendErasedMountService"));

    let builder = SendErasedMountRouter::<Body, ()>::builder();
    assert!(format!("{builder:?}").contains("SendErasedMountRouterBuilder"));

    let mounts = SendErasedMountRouter::<Body, ()>::builder()
        .mount("GET", "/items", service)
        .build()
        .expect("mount registration is valid");
    assert!(format!("{mounts:?}").contains("SendErasedMountRouter"));

    let cloned = mounts.clone();
    assert_eq!(format!("{cloned:?}"), format!("{mounts:?}"));
    assert!(format!("{cloned:?}").contains("SendErasedMountRouter"));
}

#[tokio::test(flavor = "current_thread")]
async fn send_erased_mount_router_supports_a_custom_fallback_builder_and_reports_misses() {
    let mounts = SendErasedMountRouter::<Body, ()>::builder_with_fallback(send_fallback)
        .build()
        .expect("an empty mount table is valid");
    let response = mounts
        .route(Request::get("/missing").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "current_thread")]
async fn send_erased_mount_router_default_fallback_reports_a_miss() {
    let mounts = SendErasedMountRouter::<Body, ()>::builder()
        .build()
        .expect("an empty mount table is valid");
    let response = mounts
        .route(Request::get("/missing").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn mounted_capture_error_display_reports_each_variant() {
    assert_eq!(MountedCaptureError::Missing.to_string(), "mounted route capture is missing");
    assert_eq!(
        MountedCaptureError::Undecodable.to_string(),
        "mounted route capture could not be percent-decoded"
    );
    assert_eq!(
        MountedCaptureError::Invalid.to_string(),
        "mounted route capture could not be parsed"
    );
}
