// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the `tower`/`layered` HTTP adapters.

#![cfg(any(feature = "tower", feature = "layered"))]

use http::{Method, Request, Uri};
use http_body_util::{BodyExt as _, Full};
use rest_over_grpc::handling::Status;
use rest_over_grpc::serving::{RestBody, RestService, serve_http_fn};
use rest_over_grpc::transcoding::{HttpResponse, TranscodeResponse};

/// Collects an adapter response body ([`RestBody`], an [`http_body::Body`]) into
/// bytes for assertions.
fn body_bytes(response: http::Response<RestBody>) -> bytes::Bytes {
    futures::executor::block_on(response.into_body().collect())
        .expect("body collects")
        .to_bytes()
}

async fn echo_transcoder(method: Method, uri: Uri, _headers: http::HeaderMap, body: bytes::Bytes) -> HttpResponse {
    if method == Method::GET && uri.path() == "/ok" {
        HttpResponse::ok_json(body.to_vec())
    } else {
        HttpResponse::from_status(&Status::not_found("nope"))
    }
}

#[test]
fn serve_http_fn_collects_body_and_transcodes() {
    let request = Request::builder()
        .method(Method::GET)
        .uri("/ok")
        .body(Full::new(bytes::Bytes::from_static(b"hello")))
        .expect("valid request");

    let response = futures::executor::block_on(serve_http_fn(request, echo_transcoder));
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(body_bytes(response).as_ref(), b"hello");
}

#[cfg(feature = "tower")]
#[test]
fn tower_service_transcodes() {
    use tower_service::Service as _;

    let mut service = RestService::new(EchoTranscode);
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
    use core::task::{Context, Poll};

    let mut service = RestService::new(EchoTranscode);
    let mut cx = Context::from_waker(futures::task::noop_waker_ref());
    assert!(matches!(
        tower_service::Service::<Request<Full<bytes::Bytes>>>::poll_ready(&mut service, &mut cx),
        Poll::Ready(Ok(()))
    ));
}

#[cfg(feature = "layered")]
#[test]
fn layered_service_transcodes() {
    use layered::Service as _;

    let service = RestService::new(EchoTranscode);

    let ok = Request::builder()
        .method(Method::GET)
        .uri("/ok")
        .body(Full::new(bytes::Bytes::from_static(b"hi")))
        .expect("valid request");
    let response = futures::executor::block_on(service.execute(ok));
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(body_bytes(response).as_ref(), b"hi");

    let missing = Request::builder()
        .method(Method::GET)
        .uri("/missing")
        .body(Full::new(bytes::Bytes::new()))
        .expect("valid request");
    let response = futures::executor::block_on(service.execute(missing));
    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
}

/// A hand-written [`Transcode`] used to exercise the transcode-taking adapters.
#[derive(Clone)]
struct EchoTranscode;

impl rest_over_grpc::transcoding::Transcode for EchoTranscode {
    fn try_transcode(
        &self,
        method: &str,
        target: &str,
        _headers: http::HeaderMap,
        body: &[u8],
    ) -> impl core::future::Future<Output = Option<TranscodeResponse>> + Send {
        let hit = method == "GET" && target.starts_with("/ok");
        let body = body.to_vec();
        async move { hit.then(|| HttpResponse::ok_json(body).into()) }
    }
}

#[test]
fn serve_http_wires_a_transcode_impl() {
    use rest_over_grpc::serving::serve_http;

    let ok = Request::builder()
        .method(Method::GET)
        .uri("/ok?x=1")
        .body(Full::new(bytes::Bytes::from_static(b"hi")))
        .expect("valid request");
    let response = futures::executor::block_on(serve_http(ok, &EchoTranscode));
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(body_bytes(response).as_ref(), b"hi");

    let missing = Request::builder()
        .method(Method::GET)
        .uri("/nope")
        .body(Full::new(bytes::Bytes::new()))
        .expect("valid request");
    let response = futures::executor::block_on(serve_http(missing, &EchoTranscode));
    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
}

#[cfg(feature = "tower")]
#[test]
fn rest_service_wraps_a_transcode_impl() {
    use tower_service::Service as _;

    let mut service = RestService::new(EchoTranscode);
    let request = Request::builder()
        .method(Method::GET)
        .uri("/ok")
        .body(Full::new(bytes::Bytes::from_static(b"hey")))
        .expect("valid request");
    let response = futures::executor::block_on(service.call(request)).expect("infallible");
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(body_bytes(response).as_ref(), b"hey");
}

/// An `Arc`-wrapped transcoder still satisfies the adapters via the blanket impl.
#[test]
fn arc_transcode_forwards() {
    use rest_over_grpc::serving::serve_http;

    let transcoder = std::sync::Arc::new(EchoTranscode);
    let request = Request::builder()
        .method(Method::GET)
        .uri("/ok")
        .body(Full::new(bytes::Bytes::from_static(b"arc")))
        .expect("valid request");
    let response = futures::executor::block_on(serve_http(request, &transcoder));
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(body_bytes(response).as_ref(), b"arc");
}

/// A request body that errors mid-read is surfaced as `400 Bad Request` rather
/// than silently transcoded as an empty body.
#[test]
fn body_read_failure_becomes_bad_request() {
    let request = Request::builder()
        .method(Method::GET)
        .uri("/ok")
        .body(FailingBody)
        .expect("valid request");

    let response = futures::executor::block_on(serve_http_fn(request, echo_transcoder));
    assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);
}

/// A body within the configured cap transcodes normally; one over the cap is
/// rejected with `413 Payload Too Large` before it reaches the transcoder.
#[cfg(feature = "tower")]
#[test]
fn rest_service_enforces_max_body_bytes() {
    use tower_service::Service as _;

    let mut service = RestService::new(EchoTranscode).with_max_body_bytes(4);

    let within = Request::builder()
        .method(Method::GET)
        .uri("/ok")
        .body(Full::new(bytes::Bytes::from_static(b"abcd")))
        .expect("valid request");
    let response = futures::executor::block_on(service.call(within)).expect("infallible");
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(body_bytes(response).as_ref(), b"abcd");

    let over = Request::builder()
        .method(Method::GET)
        .uri("/ok")
        .body(Full::new(bytes::Bytes::from_static(b"abcde")))
        .expect("valid request");
    let response = futures::executor::block_on(service.call(over)).expect("infallible");
    assert_eq!(response.status(), http::StatusCode::PAYLOAD_TOO_LARGE);
}

/// A body whose first frame is an error, exercising the read-failure path.
/// A body read failure through the buffered `serve_http` path (uncapped) is a
/// `400`, exercising `collect_body`'s uncapped read-error branch.
#[test]
fn serve_http_surfaces_body_read_failure() {
    use rest_over_grpc::serving::serve_http;

    let request = Request::builder()
        .method(Method::GET)
        .uri("/ok")
        .body(FailingBody)
        .expect("valid request");
    let response = futures::executor::block_on(serve_http(request, &EchoTranscode));
    assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);
}

/// A body read failure while a size cap is in effect exercises the capped
/// frame-by-frame reader's error branch (distinct from the uncapped path).
#[cfg(feature = "tower")]
#[test]
fn capped_service_surfaces_body_read_failure() {
    use tower_service::Service as _;

    let mut service = RestService::new(EchoTranscode).with_max_body_bytes(64);
    let request = Request::builder()
        .method(Method::GET)
        .uri("/ok")
        .body(FailingBody)
        .expect("valid request");
    let response = futures::executor::block_on(service.call(request)).expect("infallible");
    assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);
}

/// A body whose first frame is an error, exercising the read-failure path.
struct FailingBody;

impl http_body::Body for FailingBody {
    type Data = bytes::Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: core::pin::Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        core::task::Poll::Ready(Some(Err(std::io::Error::other("body read failed"))))
    }
}

/// A body that yields one data frame then a trailer frame, exercising the capped
/// reader's "skip non-data frame" branch (trailers carry no body bytes).
struct DataThenTrailer(u8);

impl http_body::Body for DataThenTrailer {
    type Data = bytes::Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: core::pin::Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let step = this.0;
        this.0 = step + 1;
        let frame = match step {
            0 => http_body::Frame::data(bytes::Bytes::from_static(b"ok")),
            1 => http_body::Frame::trailers(http::HeaderMap::new()),
            _ => return core::task::Poll::Ready(None),
        };
        core::task::Poll::Ready(Some(Ok(frame)))
    }
}

/// A capped read whose body includes a trailer frame reads the data and ignores
/// the trailer, transcoding successfully.
#[cfg(feature = "tower")]
#[test]
fn capped_service_skips_trailer_frames() {
    use tower_service::Service as _;

    let mut service = RestService::new(EchoTranscode).with_max_body_bytes(64);
    let request = Request::builder()
        .method(Method::GET)
        .uri("/ok")
        .body(DataThenTrailer(0))
        .expect("valid request");
    let response = futures::executor::block_on(service.call(request)).expect("infallible");
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(body_bytes(response).as_ref(), b"ok");
}
