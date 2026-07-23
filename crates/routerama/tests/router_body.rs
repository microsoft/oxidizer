// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral coverage for explicit raw and bounded request-body extraction.

#![expect(clippy::panic, reason = "a pending in-memory body is a test invariant violation")]

use std::collections::VecDeque;
use std::fmt;
use std::hint::black_box;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use alloc_tracker::{Allocator, Session};
use bytes::Bytes;
use http_body::{Frame, SizeHint};
use http_body_util::BodyExt as _;
use routerama::route::{BodyRejection, BytesBody, FromRequestBody, HeaderMap, Method, RawBody, Request, StatusCode, TextBody, router};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TestBodyError(&'static str);

impl fmt::Display for TestBodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for TestBodyError {}

struct TestBody {
    frames: VecDeque<Result<Frame<Bytes>, TestBodyError>>,
    size_hint: SizeHint,
    identity: usize,
}

impl TestBody {
    fn from_chunks(chunks: Vec<Bytes>) -> Self {
        Self {
            frames: chunks.into_iter().map(|chunk| Ok(Frame::data(chunk))).collect(),
            size_hint: SizeHint::default(),
            identity: 0,
        }
    }

    fn failed(error: &'static str) -> Self {
        Self {
            frames: [Err(TestBodyError(error))].into(),
            size_hint: SizeHint::default(),
            identity: 0,
        }
    }

    fn with_size_hint(mut self, size_hint: SizeHint) -> Self {
        self.size_hint = size_hint;
        self
    }

    fn with_identity(mut self, identity: usize) -> Self {
        self.identity = identity;
        self
    }
}

impl http_body::Body for TestBody {
    type Data = Bytes;
    type Error = TestBodyError;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.frames.pop_front())
    }

    fn size_hint(&self) -> SizeHint {
        self.size_hint.clone()
    }
}

struct BufferedApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl BufferedApi {
    #[route(POST, "/first")]
    async fn first(&self, #[body] body: BytesBody<5>, method: Method) -> Bytes {
        assert_eq!(method, Method::POST);
        body.into_inner()
    }

    #[route(POST, "/middle")]
    async fn middle(&self, method: Method, #[body] body: BytesBody<5>, headers: HeaderMap) -> Bytes {
        assert_eq!(method, Method::POST);
        assert_eq!(headers["x-marker"], "present");
        body.into_inner()
    }

    #[route(POST, "/last")]
    async fn last(&self, method: Method, headers: HeaderMap, #[body] body: BytesBody<5>) -> Bytes {
        assert_eq!(method, Method::POST);
        assert_eq!(headers["x-marker"], "present");
        body.into_inner()
    }

    #[route(POST, "/text")]
    async fn text(&self, #[body] body: TextBody<5>) -> String {
        body.into_inner()
    }
}

#[tokio::test]
async fn body_markers_are_position_independent_and_handlers_remain_directly_callable() {
    for path in ["/first", "/middle", "/last"] {
        let request = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header("x-marker", "present")
            .body(TestBody::from_chunks(vec![Bytes::from_static(b"body")]))
            .expect("the test request uses valid static metadata");
        let response = BufferedApi.route(request, &()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_bytes(response.into_body()).await, b"body"[..]);
    }

    let direct = BufferedApi.first(BytesBody(Bytes::from_static(b"call")), Method::POST).await;
    assert_eq!(direct, "call");
}

#[tokio::test]
async fn exact_limit_and_multiple_frames_are_accepted() {
    let body = TestBody::from_chunks(vec![Bytes::from_static(b"12"), Bytes::from_static(b"345")]);
    let response = BufferedApi.route(request("/first", body), &()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response.into_body()).await, b"12345"[..]);

    let text = TestBody::from_chunks(vec![Bytes::from_static(b"ru"), Bytes::from_static(b"st")]);
    let response = BufferedApi.route(request("/text", text), &()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response.into_body()).await, b"rust"[..]);
}

#[tokio::test]
async fn a_single_data_frame_is_reused_without_copying() {
    let chunk = Bytes::from(vec![b'x'; 4096]);
    let original = chunk.as_ptr();
    let (parts, ()) = Request::new(()).into_parts();

    let extracted = <BytesBody<4096> as FromRequestBody<(), TestBody>>::from_request_body(
        &parts,
        TestBody::from_chunks(vec![chunk]).with_size_hint(SizeHint::with_exact(4096)),
        &(),
    )
    .await
    .expect("an exact-limit frame is accepted");

    assert_eq!(extracted.as_bytes().as_ptr(), original);
}

#[test]
fn buffering_allocates_only_when_frames_must_be_combined() {
    static CHUNK: [u8; 1024] = [b'x'; 1024];

    let (parts, ()) = Request::new(()).into_parts();
    let single = TestBody::from_chunks(vec![Bytes::from_static(&CHUNK)]).with_size_hint(SizeHint::with_exact(1024));
    let split = TestBody::from_chunks(vec![Bytes::from_static(&CHUNK); 16]).with_size_hint(SizeHint::with_exact(16 * 1024));
    let session = Session::new().no_stdout().no_file();

    let single_operation = session.operation("single");
    {
        let _span = single_operation.measure_thread();
        black_box(run_ready(<BytesBody<1024> as FromRequestBody<(), TestBody>>::from_request_body(
            &parts,
            single,
            &(),
        )))
        .expect("the single frame is accepted");
    }

    let split_operation = session.operation("split");
    {
        let _span = split_operation.measure_thread();
        black_box(run_ready(
            <BytesBody<{ 16 * 1024 }> as FromRequestBody<(), TestBody>>::from_request_body(&parts, split, &()),
        ))
        .expect("the split body is accepted");
    }

    assert_eq!(single_operation.total_bytes_allocated(), 0);
    assert_eq!(split_operation.total_bytes_allocated(), 16 * 1024);
}

fn run_ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the in-memory test body is always ready"),
    }
}

#[tokio::test]
async fn over_limit_is_rejected_even_when_the_size_hint_claims_zero() {
    let lying_hint = SizeHint::with_exact(0);
    let body = TestBody::from_chunks(vec![Bytes::from_static(b"123"), Bytes::from_static(b"456")]).with_size_hint(lying_hint);
    let response = BufferedApi.route(request("/first", body), &()).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let (parts, ()) = Request::new(()).into_parts();
    let rejection = <BytesBody<5> as FromRequestBody<(), TestBody>>::from_request_body(
        &parts,
        TestBody::from_chunks(vec![Bytes::from_static(b"123456")]),
        &(),
    )
    .await
    .expect_err("six bytes must exceed the explicit five-byte limit");
    let BodyRejection::TooLarge(error) = rejection else {
        panic!("expected a size-limit rejection");
    };
    assert_eq!(error.limit(), 5);
    assert_eq!(error.received(), 6);
}

#[tokio::test]
async fn invalid_utf8_is_a_bad_request() {
    let response = BufferedApi
        .route(request("/text", TestBody::from_chunks(vec![Bytes::from_static(b"\xff")])), &())
        .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let (parts, ()) = Request::new(()).into_parts();
    let rejection = <TextBody<5> as FromRequestBody<(), TestBody>>::from_request_body(
        &parts,
        TestBody::from_chunks(vec![Bytes::from_static(b"\xff")]),
        &(),
    )
    .await
    .expect_err("the byte is not valid UTF-8");
    let BodyRejection::InvalidUtf8(error) = rejection else {
        panic!("expected a UTF-8 rejection");
    };
    assert_eq!(error.valid_up_to(), 0);
    assert_eq!(error.error_len(), Some(1));
}

#[tokio::test]
async fn transport_errors_are_preserved_and_become_bad_requests() {
    let response = BufferedApi.route(request("/first", TestBody::failed("disconnected")), &()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let (parts, ()) = Request::new(()).into_parts();
    let rejection = <BytesBody<5> as FromRequestBody<(), TestBody>>::from_request_body(&parts, TestBody::failed("diagnostic"), &())
        .await
        .expect_err("the transport frame must be rejected");
    let BodyRejection::Transport(error) = rejection else {
        panic!("expected a transport rejection");
    };
    assert_eq!(error.error(), &TestBodyError("diagnostic"));
    assert_eq!(error.into_inner(), TestBodyError("diagnostic"));
}

struct RawApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl RawApi {
    #[route(POST, "/raw")]
    async fn raw(&self, method: Method, #[body] body: RawBody<TestBody>) -> StatusCode {
        let body = body.into_inner();
        assert_eq!(method, Method::POST);
        assert_eq!(body.identity, 42);
        assert_eq!(body.frames.len(), 2);
        StatusCode::NO_CONTENT
    }
}

#[tokio::test]
async fn raw_body_preserves_the_unpolled_transport_body() {
    let body = TestBody::from_chunks(vec![Bytes::from_static(b"first"), Bytes::from_static(b"second")]).with_identity(42);
    let response = RawApi.route(request("/raw", body), &()).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

struct LocalBody {
    inner: TestBody,
    _not_send: Rc<()>,
}

impl http_body::Body for LocalBody {
    type Data = Bytes;
    type Error = TestBodyError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn bounded_extraction_does_not_require_a_send_body() {
    let body = LocalBody {
        inner: TestBody::from_chunks(vec![Bytes::from_static(b"local")]),
        _not_send: Rc::new(()),
    };
    let response = BufferedApi.route(request("/first", body), &()).await;

    assert_eq!(body_bytes(response.into_body()).await, b"local"[..]);
}

fn request<B>(path: &str, body: B) -> Request<B> {
    Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("x-marker", "present")
        .body(body)
        .expect("the test request uses valid static metadata")
}

async fn body_bytes<B>(body: B) -> Bytes
where
    B: http_body::Body<Data = Bytes>,
    B::Error: std::fmt::Debug,
{
    body.collect().await.expect("the generated response body succeeds").to_bytes()
}
