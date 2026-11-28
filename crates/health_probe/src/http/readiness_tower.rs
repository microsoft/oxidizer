// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tower service integration for HTTP readiness.

use std::convert::Infallible;
use std::future::{Ready, ready};
use std::task::{Context, Poll};

use ::http::{Request, Response};
use health::Monitor;
use tower_service::Service;

#[derive(Clone, Debug)]
struct Readiness {
    monitor: Monitor,
}

impl<B> Service<Request<B>> for Readiness {
    type Response = Response<String>;
    type Error = Infallible;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _request: Request<B>) -> Self::Future {
        ready(Ok(super::readiness(self.monitor.state(), str::to_owned)))
    }
}

/// Returns a Tower readiness service over `monitor`.
///
/// Axum can mount the service directly with
/// [`Router::route_service`](https://docs.rs/axum/latest/axum/struct.Router.html#method.route_service).
///
/// # Examples
///
/// ```
/// # use std::error::Error;
/// #
/// # fn main() -> Result<(), Box<dyn Error>> {
/// # #[cfg(feature = "http-readiness-tower")] {
/// use axum::Router;
/// use axum::body::{Body, to_bytes};
/// use health::Aggregator;
/// use health_probe::http::readiness_tower::service;
/// use http::{Request, StatusCode};
/// use observed::Sink;
/// use tower::ServiceExt;
///
/// let aggregator = Aggregator::new(Sink::noop());
/// let monitor = aggregator.monitor();
///
/// // Create components and retain their publishers in application state.
///
/// let app = Router::new().route_service("/health/ready", service(monitor));
/// let response = futures::executor::block_on(
///     app.oneshot(
///         Request::builder()
///             .uri("/health/ready")
///             .body(Body::empty())?,
///     ),
/// )?;
/// assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
/// let body = futures::executor::block_on(to_bytes(response.into_body(), usize::MAX))?;
/// assert_eq!(body.as_ref(), b"false");
/// # }
/// # Ok(())
/// # }
/// ```
#[cfg_attr(docsrs, doc(cfg(feature = "http-readiness-tower")))]
pub fn service<B>(
    monitor: Monitor,
) -> impl tower_service::Service<::http::Request<B>, Response = Response<String>, Error = Infallible, Future: Send + 'static>
+ Clone
+ Send
+ Sync
+ 'static {
    Readiness { monitor }
}
