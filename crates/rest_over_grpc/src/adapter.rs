// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Web-stack adapters bridging the `http` / `http-body` ecosystem to the neutral
//! dispatcher signature.
//!
//! A `rest_over_grpc_build`-generated `dispatch` function has the framework-neutral
//! signature `async fn(&Service, method: &str, target: &str, body: &[u8]) ->
//! HttpResponse`. The helpers here adapt that to concrete server plumbing:
//!
//! - [`transcode_http`] consumes an [`http::Request`] (any [`http_body::Body`],
//!   including hyper's `Incoming`), reads the body, invokes a dispatcher, and
//!   returns an [`http::Response`].
//! - [`RestService`] wraps a dispatcher as a service usable directly with
//!   `hyper-util`/`axum`/`tower` servers. It implements both
//!   [`tower_service::Service`] (feature `tower`) and [`layered::Service`]
//!   (feature `layered`) over the same dispatcher, sharing all logic via
//!   [`transcode_http`].
//!
//! Both take the dispatcher as a closure so the core never names a concrete web
//! stack:
//!
//! ```
//! # fn main() {
//! # #[cfg(feature = "tower")] {
//! use rest_over_grpc::HttpResponse;
//! use rest_over_grpc::adapter::RestService;
//!
//! let _svc = RestService::new(
//!     |_method: http::Method, _uri: http::Uri, _body: bytes::Bytes| async {
//!         HttpResponse::ok_json(b"{}".to_vec())
//!     },
//! );
//! # }
//! # }
//! ```

#[cfg(feature = "tower")]
use core::convert::Infallible;
#[cfg(feature = "tower")]
use core::pin::Pin;
#[cfg(feature = "tower")]
use core::task::{Context, Poll};

use http::{Method, Request, Response, Uri};
use http_body::Body;
use http_body_util::BodyExt;

use crate::HttpResponse;

/// Reads the body of `request` and dispatches it, returning an
/// [`http::Response`] with a `Vec<u8>` body.
///
/// `dispatcher` receives the request method, URI, and collected body bytes as
/// [`Bytes`](bytes::Bytes) (which derefs to `&[u8]`, so no copy is needed to hand
/// the body to a generated `dispatch`). A body that fails to read is treated as
/// empty (the dispatcher will typically then reject it as an invalid request).
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "tower")] {
/// use http_body_util::Full;
/// use rest_over_grpc::HttpResponse;
/// use rest_over_grpc::adapter::transcode_http;
///
/// let request = http::Request::builder()
///     .method(http::Method::GET)
///     .uri("/ok")
///     .body(Full::new(bytes::Bytes::from_static(b"hello")))
///     .expect("valid request");
///
/// let response =
///     futures::executor::block_on(transcode_http(request, |_method, _uri, body| async move {
///         HttpResponse::ok_json(body.to_vec())
///     }));
///
/// assert_eq!(response.status(), http::StatusCode::OK);
/// assert_eq!(response.body(), b"hello");
/// # }
/// # }
/// ```
pub async fn transcode_http<B, D, Fut>(request: Request<B>, dispatcher: D) -> Response<Vec<u8>>
where
    B: Body,
    D: FnOnce(Method, Uri, bytes::Bytes) -> Fut,
    Fut: Future<Output = HttpResponse>,
{
    let (parts, body) = request.into_parts();
    let bytes = collect_body(body).await;
    dispatcher(parts.method, parts.uri, bytes).await.into_http()
}

async fn collect_body<B: Body>(body: B) -> bytes::Bytes {
    // `to_bytes()` yields a contiguous `Bytes`, avoiding a full-body copy.
    body.collect().await.map(http_body_util::Collected::to_bytes).unwrap_or_default()
}

/// A service that transcodes HTTP requests via a dispatcher closure.
///
/// Construct it with [`RestService::new`]; the `dispatcher` closure maps a
/// request's `(method, uri, body)` to a future yielding an [`HttpResponse`]
/// (typically by calling a generated `dispatch` function). It implements both
/// [`tower_service::Service`] (feature `tower`) and [`layered::Service`] (feature
/// `layered`).
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "tower")] {
/// use rest_over_grpc::HttpResponse;
/// use rest_over_grpc::adapter::RestService;
///
/// let _service = RestService::new(
///     |_method: http::Method, _uri: http::Uri, _body: bytes::Bytes| async {
///         HttpResponse::ok_json(br#"{"ok":true}"#.to_vec())
///     },
/// );
/// # }
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct RestService<D> {
    dispatcher: D,
}

impl<D> RestService<D> {
    /// Wraps `dispatcher` as a service.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "tower")] {
    /// use rest_over_grpc::HttpResponse;
    /// use rest_over_grpc::adapter::RestService;
    ///
    /// let _service = RestService::new(
    ///     |_method: http::Method, _uri: http::Uri, _body: bytes::Bytes| async {
    ///         HttpResponse::ok_json(b"{}".to_vec())
    ///     },
    /// );
    /// # }
    /// # }
    /// ```
    pub const fn new(dispatcher: D) -> Self {
        Self { dispatcher }
    }
}

#[cfg(feature = "tower")]
impl<B, D, Fut> tower_service::Service<Request<B>> for RestService<D>
where
    B: Body + Send + 'static,
    B::Data: Send,
    D: Fn(Method, Uri, bytes::Bytes) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = HttpResponse> + Send + 'static,
{
    type Response = Response<Vec<u8>>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let dispatcher = self.dispatcher.clone();
        Box::pin(async move {
            let response = transcode_http(req, dispatcher).await;
            Ok(response)
        })
    }
}

/// A [`layered::Service`] transcoding HTTP requests via the dispatcher closure.
///
/// This mirrors the [`tower_service::Service`] impl exactly — both defer to
/// [`transcode_http`] — but uses `layered`'s `async fn`-with-`&self` model, so it
/// needs no `Clone`, boxed future, or `poll_ready`.
#[cfg(feature = "layered")]
impl<B, D, Fut> layered::Service<Request<B>> for RestService<D>
where
    B: Body + Send + 'static,
    B::Data: Send,
    D: Fn(Method, Uri, bytes::Bytes) -> Fut + Send + Sync,
    Fut: Future<Output = HttpResponse> + Send,
{
    type Out = Response<Vec<u8>>;

    fn execute(&self, input: Request<B>) -> impl Future<Output = Self::Out> + Send {
        // `&D` satisfies `transcode_http`'s `FnOnce` bound; no clone needed.
        transcode_http(input, &self.dispatcher)
    }
}

#[cfg(test)]
mod tests {
    use http_body_util::Full;

    use super::*;
    use crate::Status;

    async fn echo_dispatcher(method: Method, uri: Uri, body: bytes::Bytes) -> HttpResponse {
        if method == Method::GET && uri.path() == "/ok" {
            HttpResponse::ok_json(body.to_vec())
        } else {
            crate::transcode::status_response(&Status::not_found("nope"))
        }
    }

    #[test]
    fn transcode_http_collects_body_and_dispatches() {
        let request = Request::builder()
            .method(Method::GET)
            .uri("/ok")
            .body(Full::new(bytes::Bytes::from_static(b"hello")))
            .expect("valid request");

        let response = futures::executor::block_on(transcode_http(request, echo_dispatcher));
        assert_eq!(response.status(), http::StatusCode::OK);
        assert_eq!(response.body(), b"hello");
    }

    #[cfg(feature = "tower")]
    #[test]
    fn tower_service_dispatches() {
        use tower_service::Service as _;

        let mut service = RestService::new(echo_dispatcher);
        let request = Request::builder()
            .method(Method::GET)
            .uri("/missing")
            .body(Full::new(bytes::Bytes::new()))
            .expect("valid request");

        let response = futures::executor::block_on(service.call(request)).expect("infallible");
        assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    }

    #[cfg(feature = "tower")]
    #[test]
    fn tower_service_is_always_ready() {
        let mut service = RestService::new(echo_dispatcher);
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        assert!(matches!(
            tower_service::Service::<Request<Full<bytes::Bytes>>>::poll_ready(&mut service, &mut cx),
            Poll::Ready(Ok(()))
        ));
    }

    #[cfg(feature = "layered")]
    #[test]
    fn layered_service_dispatches() {
        use layered::Service as _;

        let service = RestService::new(echo_dispatcher);

        let ok = Request::builder()
            .method(Method::GET)
            .uri("/ok")
            .body(Full::new(bytes::Bytes::from_static(b"hi")))
            .expect("valid request");
        let response = futures::executor::block_on(service.execute(ok));
        assert_eq!(response.status(), http::StatusCode::OK);
        assert_eq!(response.body(), b"hi");

        let missing = Request::builder()
            .method(Method::GET)
            .uri("/missing")
            .body(Full::new(bytes::Bytes::new()))
            .expect("valid request");
        let response = futures::executor::block_on(service.execute(missing));
        assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    }
}
