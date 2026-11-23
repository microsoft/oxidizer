// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP readiness adapters over an [`Aggregator`].
//!
//! Each adapter reads [`Aggregator::state`] for every request. `Healthy` responds with status
//! `200 OK` and the JSON body `true`; any other state responds with
//! `503 Service Unavailable` and the JSON body `false`. Responses carry no component details and
//! disable caching. The request body is ignored. Applications own route registration, HTTP method
//! selection, authentication, and authorization.

use http::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use http::{Response, StatusCode};

use crate::{Aggregator, Health};

fn readiness<B>(state: Health, body: impl FnOnce(&'static str) -> B) -> Response<B> {
    let (status, text) = match state {
        Health::Healthy => (StatusCode::OK, "true"),
        Health::Degraded | Health::Unhealthy => (StatusCode::SERVICE_UNAVAILABLE, "false"),
    };
    let mut response = Response::new(body(text));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(any(test, feature = "http"))]
mod tower_service {
    use std::convert::Infallible;
    use std::future::{Ready, ready};
    use std::task::{Context, Poll};

    use http::{Request, Response};
    use tower::Service;

    use crate::Aggregator;

    #[derive(Clone, Debug)]
    pub(super) struct Readiness {
        pub(super) aggregator: Aggregator,
    }

    impl<B> Service<Request<B>> for Readiness {
        type Response = Response<String>;
        type Error = Infallible;
        type Future = Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _request: Request<B>) -> Self::Future {
            ready(Ok(super::readiness(self.aggregator.state(), String::from)))
        }
    }
}

/// Returns a Tower readiness service over `aggregator`.
///
/// # Examples
///
/// This example uses the `util` feature of `tower` for `oneshot`.
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "http")] {
/// # testing_aids::init_tracing!();
/// use http::{Request, StatusCode};
/// use oxidizer_health::http::service;
/// use oxidizer_health::{Aggregator, Health};
/// use tower::ServiceExt;
///
/// # futures::executor::block_on(async {
/// let mut aggregator = Aggregator::new();
/// let database = aggregator.component("database");
/// let _publisher = database.publisher();
/// while aggregator.state() != Health::Healthy {
///     aggregator.changed().await;
/// }
///
/// let response = service(aggregator).oneshot(Request::new(())).await.unwrap();
/// assert_eq!(response.status(), StatusCode::OK);
/// assert_eq!(response.body(), "true");
/// # });
/// # }
/// # }
/// ```
#[cfg(any(test, feature = "http"))]
pub fn service<B>(
    aggregator: Aggregator,
) -> impl tower::Service<http::Request<B>, Response = Response<String>, Error = std::convert::Infallible, Future: Send + 'static>
+ Clone
+ Send
+ Sync
+ 'static {
    tower_service::Readiness { aggregator }
}

/// Returns an [`http_extensions::RequestHandler`] serving readiness over `aggregator`.
///
/// # Examples
///
/// This example uses the `test-util` feature of `http_extensions` to build a fake request body.
/// A server passes the incoming request directly to the handler.
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "request-handler")] {
/// # testing_aids::init_tracing!();
/// use http::{Request, StatusCode};
/// use http_extensions::HttpBodyBuilder;
/// use layered::Service;
/// use oxidizer_health::Aggregator;
/// use oxidizer_health::http::request_handler;
///
/// # futures::executor::block_on(async {
/// let aggregator = Aggregator::new();
/// let handler = request_handler(aggregator);
///
/// let response = handler
///     .execute(Request::new(HttpBodyBuilder::new_fake().empty()))
///     .await
///     .unwrap();
/// assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
/// # });
/// # }
/// # }
/// ```
#[cfg(any(test, feature = "request-handler"))]
pub fn request_handler(aggregator: Aggregator) -> impl http_extensions::RequestHandler + Clone + 'static {
    use std::task::Waker;

    use bytesbuf::mem::GlobalPool;
    use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest};
    use layered::Execute;
    use tick::runtime::InactiveClock;

    // Text bodies are fully buffered and never consult the clock, so an isolated clock that is
    // never driven is sufficient.
    let (clock, _driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let bodies = HttpBodyBuilder::new(GlobalPool::new(), &clock);
    Execute::new(move |_request: HttpRequest| {
        let response = readiness(aggregator.state(), |text| bodies.text(text));
        async move { Ok::<_, HttpError>(response) }
    })
}
