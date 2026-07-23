// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Privacy coverage for request-parts rejections in generated route entries.
//!
//! A rejection type that is private to the module defining a router must never
//! reach that router's public entry signature: callers outside the module
//! cannot name it, and rustc rejects a call that infers a generic parameter as
//! a private type. Each router below keeps its extractor, its rejection, and
//! the rejection's response body private, and every call happens outside the
//! defining module.

#![deny(private_bounds, private_interfaces)]

use routerama::response::Body;
use routerama::route::{Request, StatusCode};

mod generic_state {
    use core::convert::Infallible;
    use core::pin::Pin;
    use core::task::{Context, Poll};

    use bytes::Bytes;
    use http_body::{Frame, SizeHint};
    use routerama::response::{IntoResponse, Response};
    use routerama::route::{FromRequestParts, RequestParts, StatusCode, router};

    /// A response body that no caller outside this module can name.
    #[derive(Debug, Default)]
    struct HiddenBody;

    impl http_body::Body for HiddenBody {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Ready(None)
        }

        fn is_end_stream(&self) -> bool {
            true
        }

        fn size_hint(&self) -> SizeHint {
            SizeHint::with_exact(0)
        }
    }

    #[derive(Debug)]
    struct HiddenRejection;

    impl IntoResponse for HiddenRejection {
        type Body = HiddenBody;

        fn into_response(self) -> Response<Self::Body> {
            let mut response = Response::new(HiddenBody);
            *response.status_mut() = StatusCode::FORBIDDEN;
            response
        }
    }

    struct Guard;

    impl<S: ?Sized> FromRequestParts<'_, S> for Guard {
        type Rejection = HiddenRejection;

        fn from_request_parts(parts: &RequestParts, _state: &S) -> Result<Self, Self::Rejection> {
            if parts.headers.contains_key("x-guard") {
                Ok(Self)
            } else {
                Err(HiddenRejection)
            }
        }
    }

    pub(crate) struct Api;

    #[allow(
        clippy::allow_attributes,
        unknown_lints,
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
    )]
    #[router]
    impl Api {
        #[route(GET, "/")]
        async fn home(&self, guard: Guard) -> StatusCode {
            let Guard = guard;
            StatusCode::NO_CONTENT
        }
    }
}

mod fixed_state {
    use routerama::response::{Body, IntoResponse, Response};
    use routerama::route::{FromRequestParts, RequestParts, StatusCode, router};

    #[derive(Debug)]
    struct HiddenRejection;

    impl IntoResponse for HiddenRejection {
        type Body = Body;

        fn into_response(self) -> Response<Self::Body> {
            StatusCode::FORBIDDEN.into_response()
        }
    }

    struct Guard;

    impl FromRequestParts<'_, ()> for Guard {
        type Rejection = HiddenRejection;

        fn from_request_parts(parts: &RequestParts, _state: &()) -> Result<Self, Self::Rejection> {
            if parts.headers.contains_key("x-guard") {
                Ok(Self)
            } else {
                Err(HiddenRejection)
            }
        }
    }

    pub(crate) struct Api;

    #[allow(
        clippy::allow_attributes,
        unknown_lints,
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
    )]
    #[router(state = ())]
    impl Api {
        #[route(GET, "/")]
        async fn home(&self, guard: Guard) -> StatusCode {
            let Guard = guard;
            StatusCode::NO_CONTENT
        }
    }
}

fn request(guarded: bool) -> Request<Body> {
    let builder = Request::builder().method("GET").uri("/");
    let builder = if guarded { builder.header("x-guard", "1") } else { builder };
    builder.body(Body::empty()).expect("the fixture request is well formed")
}

#[tokio::test(flavor = "current_thread")]
async fn a_generic_state_router_hides_its_rejection_from_outside_callers() {
    let api = generic_state::Api;

    let accepted = api.route(request(true), &()).await;
    assert_eq!(accepted.status(), StatusCode::NO_CONTENT);

    let rejected = api.route(request(false), &()).await;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "current_thread")]
async fn a_fixed_state_router_hides_its_rejection_from_outside_callers() {
    let api = fixed_state::Api;

    let accepted = api.route(request(true), &()).await;
    assert_eq!(accepted.status(), StatusCode::NO_CONTENT);

    let rejected = api.route(request(false), &()).await;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
}
