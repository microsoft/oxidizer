// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Privacy and transport-bound coverage for generated exact Tower services.

#![cfg(feature = "tower")]
#![deny(private_bounds, private_interfaces)]

use std::pin::pin;
use std::sync::Arc;

use http_body::Body as _;
use http_body_util::BodyExt as _;
use routerama::response::Body;
use routerama::route::{Request, StatusCode};
use tower_service::Service as _;

#[expect(missing_docs, reason = "public visibility exercises cross-module privacy in this integration test")]
pub mod fixed {
    use std::convert::Infallible;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use bytes::Bytes;
    use http::{HeaderMap, HeaderName, HeaderValue};
    use http_body::{Body as HttpBody, Frame, SizeHint};
    use routerama::response::{Body, IntoResponse, Response};
    use routerama::route::{FromRequestParts, Request, RequestParts, StatusCode, router};

    #[derive(Debug)]
    pub struct Api;

    struct HiddenStream {
        frame: u8,
        fail: bool,
    }

    #[derive(Debug)]
    struct HiddenError;

    impl core::fmt::Display for HiddenError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("private stream failed")
        }
    }

    impl core::error::Error for HiddenError {}

    impl HttpBody for HiddenStream {
        type Data = Bytes;
        type Error = HiddenError;

        fn poll_frame(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            let frame = self.frame;
            self.frame = frame.saturating_add(1);
            Poll::Ready(match (frame, self.fail) {
                (0, _) => Some(Ok(Frame::data(Bytes::from_static(b"data")))),
                (1, true) => Some(Err(HiddenError)),
                (1, false) => {
                    let mut trailers = HeaderMap::new();
                    trailers.insert(HeaderName::from_static("x-private"), HeaderValue::from_static("yes"));
                    Some(Ok(Frame::trailers(trailers)))
                }
                _ => None,
            })
        }

        fn size_hint(&self) -> SizeHint {
            SizeHint::with_exact(4)
        }
    }

    struct HiddenRejection;

    impl IntoResponse for HiddenRejection {
        type Body = HiddenStream;

        fn into_response(self) -> Response<Self::Body> {
            let mut response = Response::new(HiddenStream { frame: 0, fail: false });
            *response.status_mut() = StatusCode::FORBIDDEN;
            response
        }
    }

    struct Guard;

    impl FromRequestParts<'_, ()> for Guard {
        type Rejection = HiddenRejection;

        fn from_request_parts(parts: &RequestParts, _state: &()) -> Result<Self, Self::Rejection> {
            parts.headers.contains_key("x-guard").then_some(Self).ok_or(HiddenRejection)
        }
    }

    #[allow(
        clippy::allow_attributes,
        unknown_lints,
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
    )]
    #[router(state = (), tower)]
    impl Api {
        #[route(GET, "/public")]
        pub async fn public(&self) -> Bytes {
            Bytes::from_static(b"public")
        }

        #[route(GET, "/private")]
        async fn private(&self) -> Response<HiddenStream> {
            Response::new(HiddenStream { frame: 0, fail: false })
        }

        #[route(GET, "/broken")]
        async fn broken(&self) -> Response<HiddenStream> {
            Response::new(HiddenStream { frame: 0, fail: true })
        }

        #[route(GET, "/guarded")]
        async fn guarded(&self, guard: Guard) -> StatusCode {
            let _ = guard;
            StatusCode::NO_CONTENT
        }
    }

    #[must_use]
    pub fn service() -> impl tower_service::Service<
        Request<Body>,
        Response = Response<impl HttpBody<Data = Bytes, Error = impl core::error::Error + Send + Sync + 'static> + Send + 'static>,
        Error = Infallible,
        Future: Send,
    > + Clone
    + Send
    + Sync
    + 'static {
        Api::tower_service::<Body, _, _>(Arc::new(Api), Arc::new(()))
    }
}

mod generic {
    use std::convert::Infallible;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use bytes::Bytes;
    use http_body::{Body as HttpBody, Frame, SizeHint};
    use routerama::response::{Body, IntoResponse, Response};
    use routerama::route::{FromRequestParts, Request, RequestParts, StatusCode, router};

    pub(crate) struct Api;

    struct HiddenBody;

    impl HttpBody for HiddenBody {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Ready(None)
        }

        fn size_hint(&self) -> SizeHint {
            SizeHint::with_exact(0)
        }
    }

    struct HiddenRejection;

    impl IntoResponse for HiddenRejection {
        type Body = HiddenBody;

        fn into_response(self) -> Response<Self::Body> {
            let mut response = Response::new(HiddenBody);
            *response.status_mut() = StatusCode::UNAUTHORIZED;
            response
        }
    }

    struct Guard;

    impl<S: ?Sized> FromRequestParts<'_, S> for Guard {
        type Rejection = HiddenRejection;

        fn from_request_parts(_parts: &RequestParts, _state: &S) -> Result<Self, Self::Rejection> {
            Err(HiddenRejection)
        }
    }

    #[allow(
        clippy::allow_attributes,
        unknown_lints,
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
    )]
    #[router(tower)]
    impl Api {
        #[route(GET, "/")]
        async fn guarded(&self, guard: Guard) -> StatusCode {
            let _ = guard;
            StatusCode::NO_CONTENT
        }
    }

    pub(crate) fn service() -> impl tower_service::Service<
        Request<Body>,
        Response = Response<impl HttpBody<Data = Bytes, Error = impl core::error::Error + Send + Sync + 'static> + Send + 'static>,
        Error = Infallible,
        Future: Send,
    > + Clone
    + Send
    + Sync
    + 'static {
        Api::tower_service::<Body, (), _, _>(Arc::new(Api), Arc::new(()))
    }
}

fn request(path: &'static str) -> Request<Body> {
    Request::get(path).body(Body::empty()).expect("the fixture request is valid")
}

#[test]
fn generated_exact_constructor_is_public_without_exporting_private_response_types() {
    let _service = fixed::Api::tower_service::<Body, _, _>(Arc::new(fixed::Api), Arc::new(()));
}

#[tokio::test(flavor = "current_thread")]
async fn private_handler_body_error_and_rejection_types_do_not_leak() {
    let mut service = fixed::service();

    let public = service.call(request("/public")).await.expect("routing is infallible");
    assert_eq!(
        public.into_body().collect().await.expect("the public body succeeds").to_bytes(),
        b"public"[..]
    );

    let private = service.call(request("/private")).await.expect("routing is infallible");
    assert_eq!(private.body().size_hint().exact(), Some(4));
    let mut body = pin!(private.into_body());
    let first = body
        .frame()
        .await
        .expect("the data frame arrives")
        .expect("the data frame succeeds");
    assert_eq!(first.into_data().expect("the first frame is data"), b"data"[..]);
    let trailers = body
        .frame()
        .await
        .expect("the trailer frame arrives")
        .expect("the trailer frame succeeds")
        .into_trailers()
        .expect("the second frame carries trailers");
    assert_eq!(trailers["x-private"], "yes");

    let broken = service.call(request("/broken")).await.expect("routing is infallible");
    let mut broken = pin!(broken.into_body());
    let _ = broken
        .frame()
        .await
        .expect("the data frame arrives")
        .expect("the data frame succeeds");
    let error = broken
        .frame()
        .await
        .expect("the error frame arrives")
        .expect_err("the private body fails");
    assert!(error.to_string().contains("HiddenStream"), "{error}");

    let rejected = service.call(request("/guarded")).await.expect("routing is infallible");
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "current_thread")]
async fn generic_state_private_rejection_remains_opaque_at_the_tower_boundary() {
    let mut service = generic::service();
    let response = service.call(request("/")).await.expect("routing is infallible");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
