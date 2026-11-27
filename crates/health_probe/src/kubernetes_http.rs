// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Kubernetes HTTP readiness adapters over an [`health::Monitor`].
//!
//! Each adapter reads the monitor for every request. `Healthy` responds with status `200 OK` and
//! the JSON body `true`. Any other state responds with `503 Service Unavailable` and the JSON body
//! `false`. Responses carry no component details and disable caching. The request body is ignored.
//! Applications own route registration, HTTP method selection, authentication, and authorization.

use health::{Health, Monitor};
use http::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use http::{Response, StatusCode};

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
mod tower_adapter {
    use std::convert::Infallible;
    use std::future::{Ready, ready};
    use std::task::{Context, Poll};

    use health::Monitor;
    use http::{Request, Response};
    use tower_service::Service;

    #[derive(Clone, Debug)]
    pub(super) struct Readiness {
        pub(super) monitor: Monitor,
    }

    impl<B> Service<Request<B>> for Readiness {
        type Response = Response<&'static str>;
        type Error = Infallible;
        type Future = Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _request: Request<B>) -> Self::Future {
            ready(Ok(super::readiness(self.monitor.state(), |text| text)))
        }
    }
}

/// Returns a Tower readiness service over `monitor`.
///
/// # Examples
///
/// This example uses the `util` feature of `tower` for `oneshot`.
///
/// ```
/// # use std::error::Error;
/// #
/// # fn main() -> Result<(), Box<dyn Error>> {
/// # #[cfg(feature = "http")] {
/// use health::{Aggregator, Health};
/// use health_probe::kubernetes_http::service;
/// use http::{Request, StatusCode};
/// use observed::Sink;
/// use tower::ServiceExt;
///
/// let aggregator = Aggregator::new(Sink::noop());
/// let database = aggregator.component("database");
/// let mut monitor = aggregator.monitor();
/// let _publisher = database.publisher();
/// futures::executor::block_on(async {
///     while monitor.state() != Health::Healthy {
///         monitor.changed().await;
///     }
/// });
///
/// let response = futures::executor::block_on(service(monitor).oneshot(Request::new(())))?;
/// assert_eq!(response.status(), StatusCode::OK);
/// assert_eq!(response.body(), &"true");
/// # }
/// # Ok(())
/// # }
/// ```
#[cfg(any(test, feature = "http"))]
#[cfg_attr(docsrs, doc(cfg(feature = "http")))]
pub fn service<B>(
    monitor: Monitor,
) -> impl tower_service::Service<http::Request<B>, Response = Response<&'static str>, Error = std::convert::Infallible, Future: Send + 'static>
+ Clone
+ Send
+ Sync
+ 'static {
    tower_adapter::Readiness { monitor }
}

/// Returns an [`http_extensions::RequestHandler`] serving readiness over `monitor`.
///
/// # Examples
///
/// This example uses the `test-util` feature of `http_extensions` to build a fake request body.
/// A server passes the incoming request directly to the handler.
///
/// ```
/// # use std::error::Error;
/// #
/// # fn main() -> Result<(), Box<dyn Error>> {
/// # #[cfg(feature = "request-handler")] {
/// use health::Aggregator;
/// use health_probe::kubernetes_http::request_handler;
/// use http::{Request, StatusCode};
/// use http_extensions::HttpBodyBuilder;
/// use layered::Service;
/// use observed::Sink;
///
/// let aggregator = Aggregator::new(Sink::noop());
/// let monitor = aggregator.monitor();
/// let handler = request_handler(monitor);
///
/// let response = futures::executor::block_on(
///     handler.execute(Request::new(HttpBodyBuilder::new_fake().empty())),
/// )?;
/// assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
/// # }
/// # Ok(())
/// # }
/// ```
#[cfg(any(test, feature = "request-handler"))]
#[cfg_attr(docsrs, doc(cfg(feature = "request-handler")))]
pub fn request_handler(monitor: Monitor) -> impl http_extensions::RequestHandler + Clone + 'static {
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
        let response = readiness(monitor.state(), |text| bodies.text(text));
        async move { Ok::<_, HttpError>(response) }
    })
}

#[cfg(test)]
mod tests;
