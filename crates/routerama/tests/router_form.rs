// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral coverage for bounded form extraction through the query codec.

use std::cell::Cell;
use std::collections::VecDeque;
use std::fmt;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http_body::{Frame, SizeHint};
use http_body_util::BodyExt as _;
use routerama::query::{ErrorKind, FromQuery};
use routerama::response::IntoResponse as _;
use routerama::route::form::{Form, FormContentTypeError, FormRejection};
use routerama::route::{FromRequestBody, HeaderMap, Method, Query, Request, StatusCode, router};

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
    polls: Rc<Cell<usize>>,
}

impl TestBody {
    fn from_chunks(chunks: Vec<Bytes>) -> Self {
        Self {
            frames: chunks.into_iter().map(|chunk| Ok(Frame::data(chunk))).collect(),
            size_hint: SizeHint::default(),
            polls: Rc::new(Cell::new(0)),
        }
    }

    fn failed(error: &'static str) -> Self {
        Self {
            frames: [Err(TestBodyError(error))].into(),
            size_hint: SizeHint::default(),
            polls: Rc::new(Cell::new(0)),
        }
    }

    fn with_size_hint(mut self, size_hint: SizeHint) -> Self {
        self.size_hint = size_hint;
        self
    }

    fn poll_counter(&self) -> Rc<Cell<usize>> {
        Rc::clone(&self.polls)
    }
}

impl http_body::Body for TestBody {
    type Data = Bytes;
    type Error = TestBodyError;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        self.polls.set(self.polls.get() + 1);
        Poll::Ready(self.frames.pop_front())
    }

    fn size_hint(&self) -> SizeHint {
        self.size_hint.clone()
    }
}

#[derive(Debug, FromQuery, PartialEq, Eq)]
struct ScalarForm {
    value: String,
}

#[derive(Debug, FromQuery, PartialEq, Eq)]
struct NumberForm {
    count: u32,
}

#[derive(Debug, FromQuery, PartialEq, Eq)]
struct OptionalForm {
    value: Option<String>,
    tag: Vec<String>,
}

#[derive(Debug, FromQuery, PartialEq, Eq)]
struct Submission {
    title: String,
    count: u32,
    note: Option<String>,
    tag: Vec<String>,
}

#[derive(Debug, FromQuery, PartialEq, Eq)]
struct SourceQuery {
    source: String,
}

struct FormApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = ())]
impl FormApi {
    #[route(POST, "/first")]
    async fn first(&self, #[body] form: Form<ScalarForm, 7>, method: Method) -> String {
        assert_eq!(method, Method::POST);
        form.into_inner().value
    }

    #[route(POST, "/middle")]
    async fn middle(&self, method: Method, #[body] form: Form<ScalarForm, 7>, headers: &HeaderMap) -> String {
        assert_eq!(method, Method::POST);
        assert_eq!(headers["x-marker"], "present");
        form.value.clone()
    }

    #[route(POST, "/last")]
    async fn last(&self, method: Method, headers: &HeaderMap, #[body] form: Form<ScalarForm, 7>) -> String {
        assert_eq!(method, Method::POST);
        assert_eq!(headers["x-marker"], "present");
        form.into_inner().value
    }

    #[route(POST, "/empty")]
    async fn empty(&self, #[body] form: Form<OptionalForm, 0>) -> String {
        format!("{}:{}", form.value.as_deref().unwrap_or("none"), form.tag.len())
    }

    #[route(POST, "/submission")]
    async fn submission(&self, #[body] form: Form<Submission, 128>) -> String {
        format!(
            "{}:{}:{}:{}",
            form.title,
            form.count,
            form.note.as_deref().unwrap_or("none"),
            form.tag.join(",")
        )
    }

    #[route(POST, "/decode")]
    async fn decode(&self, #[body] form: Form<ScalarForm, 32>) -> String {
        form.into_inner().value
    }

    #[route(POST, "/number")]
    async fn number(&self, #[body] form: Form<NumberForm, 32>) -> String {
        form.count.to_string()
    }

    #[route(POST, "/combined")]
    async fn combined(&self, query: Query<SourceQuery>, #[body] form: Form<ScalarForm, 7>) -> String {
        format!("{}:{}", query.source, form.value)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn empty_scalar_repeated_optional_and_encoded_fields_decode_without_serde() {
    let empty = FormApi.route(form_request("/empty", TestBody::from_chunks(Vec::new())), &()).await;
    assert_eq!(response_bytes(empty).await, b"none:0"[..]);

    let body = b"title=Rust+Book&count=2&note=hello%20world&tag=fast&tag=100%25";
    let submission = FormApi
        .route(
            form_request("/submission", TestBody::from_chunks(vec![Bytes::copy_from_slice(body)])),
            &(),
        )
        .await;
    assert_eq!(submission.status(), StatusCode::OK);
    assert_eq!(response_bytes(submission).await, b"Rust Book:2:hello world:fast,100%"[..]);
}

#[tokio::test(flavor = "current_thread")]
async fn content_type_is_case_insensitive_and_accepts_legal_parameters() {
    let request = request_with_content_type(
        "/first",
        TestBody::from_chunks(vec![Bytes::from_static(b"value=x")]),
        Some("Application/X-WWW-Form-Urlencoded; charset=\"utf-8\"; version=1"),
    );
    let response = FormApi.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_bytes(response).await, b"x"[..]);
}

#[tokio::test(flavor = "current_thread")]
async fn exact_limit_split_frames_and_every_body_marker_position_are_supported() {
    for (path, chunks) in [
        ("/first", vec![Bytes::from_static(b"value=x")]),
        ("/middle", vec![Bytes::from_static(b"val"), Bytes::from_static(b"ue=x")]),
        ("/last", vec![Bytes::from_static(b"value="), Bytes::from_static(b"x")]),
    ] {
        let response = FormApi.route(form_request(path, TestBody::from_chunks(chunks)), &()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_bytes(response).await, b"x"[..]);
    }

    let direct = FormApi
        .first(
            Form(ScalarForm {
                value: "direct".to_owned(),
            }),
            Method::POST,
        )
        .await;
    assert_eq!(direct, "direct");
}

#[tokio::test(flavor = "current_thread")]
async fn actual_overflow_and_size_hint_overflow_are_rejected_before_decoding() {
    let dishonest =
        TestBody::from_chunks(vec![Bytes::from_static(b"value="), Bytes::from_static(b"xx")]).with_size_hint(SizeHint::with_exact(0));
    let response = FormApi.route(form_request("/first", dishonest), &()).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let hinted = TestBody::from_chunks(vec![Bytes::from_static(b"value=xx")]).with_size_hint(SizeHint::with_exact(8));
    let polls = hinted.poll_counter();
    let (parts, ()) = form_parts();
    let rejection = <Form<ScalarForm, 7> as FromRequestBody<(), TestBody>>::from_request_body(&parts, hinted, &())
        .await
        .expect_err("the size hint proves that the body exceeds seven bytes");
    let FormRejection::Body(routerama::route::BodyRejection::TooLarge(error)) = rejection else {
        panic!("expected a typed size-limit rejection");
    };
    assert_eq!(error.limit(), 7);
    assert_eq!(error.received(), 8);
    assert_eq!(polls.get(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_utf8_encoding_and_scalar_values_are_detailed_bad_requests() {
    let invalid_utf8 = FormApi
        .route(
            form_request("/first", TestBody::from_chunks(vec![Bytes::from_static(b"value=\xff")])),
            &(),
        )
        .await;
    assert_eq!(invalid_utf8.status(), StatusCode::BAD_REQUEST);

    let (parts, ()) = form_parts();
    let rejection = <Form<ScalarForm, 7> as FromRequestBody<(), TestBody>>::from_request_body(
        &parts,
        TestBody::from_chunks(vec![Bytes::from_static(b"value=\xff")]),
        &(),
    )
    .await
    .expect_err("the form body is not UTF-8");
    let FormRejection::InvalidUtf8(error) = rejection else {
        panic!("expected a typed UTF-8 rejection");
    };
    assert_eq!(error.valid_up_to(), 6);
    assert_eq!(error.error_len(), Some(1));

    for (path, encoded, expected_kind) in [
        ("/decode", b"value=%GG".as_slice(), ErrorKind::InvalidEncoding),
        ("/number", b"count=no".as_slice(), ErrorKind::InvalidValue),
    ] {
        let response = FormApi
            .route(
                form_request(path, TestBody::from_chunks(vec![Bytes::copy_from_slice(encoded)])),
                &(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let (parts, ()) = form_parts();
        if path == "/decode" {
            let rejection = <Form<ScalarForm, 32> as FromRequestBody<(), TestBody>>::from_request_body(
                &parts,
                TestBody::from_chunks(vec![Bytes::copy_from_slice(encoded)]),
                &(),
            )
            .await
            .expect_err("the malformed form must fail");
            let FormRejection::Malformed(error) = rejection else {
                panic!("expected a query-codec rejection");
            };
            assert_eq!(error.error().kind(), expected_kind);
            assert_eq!(error.into_inner().kind(), expected_kind);
        } else {
            let rejection = <Form<NumberForm, 32> as FromRequestBody<(), TestBody>>::from_request_body(
                &parts,
                TestBody::from_chunks(vec![Bytes::copy_from_slice(encoded)]),
                &(),
            )
            .await
            .expect_err("the malformed form must fail");
            let FormRejection::Malformed(error) = rejection else {
                panic!("expected a query-codec rejection");
            };
            assert_eq!(error.error().kind(), expected_kind);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn missing_wrong_duplicate_and_malformed_content_types_are_typed_unsupported_media() {
    let missing = request_with_content_type("/first", TestBody::from_chunks(vec![Bytes::from_static(b"value=x")]), None);
    assert_eq!(FormApi.route(missing, &()).await.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    for content_type in ["text/plain", "application/x-www-form-urlencoded; charset"] {
        let body = TestBody::from_chunks(vec![Bytes::from_static(b"value=x")]);
        let polls = body.poll_counter();
        let response = FormApi
            .route(request_with_content_type("/first", body, Some(content_type)), &())
            .await;
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(polls.get(), 0, "content type must be checked before polling");
    }

    let duplicate = Request::post("/first")
        .header("x-marker", "present")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded; charset=utf-8")
        .body(TestBody::from_chunks(vec![Bytes::from_static(b"value=x")]))
        .expect("the test request uses valid static metadata");
    assert_eq!(FormApi.route(duplicate, &()).await.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let (parts, ()) = Request::post("/")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded; charset")
        .body(())
        .expect("the test request uses valid static metadata")
        .into_parts();
    let rejection = <Form<ScalarForm, 7> as FromRequestBody<(), TestBody>>::from_request_body(
        &parts,
        TestBody::from_chunks(vec![Bytes::from_static(b"value=x")]),
        &(),
    )
    .await
    .expect_err("a malformed content type must fail");
    assert!(matches!(
        &rejection,
        FormRejection::UnsupportedMediaType(FormContentTypeError::Malformed(_))
    ));
    assert_eq!(rejection.into_response().status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test(flavor = "current_thread")]
async fn transport_errors_are_retained_and_convert_to_bad_request() {
    let response = FormApi.route(form_request("/first", TestBody::failed("disconnected")), &()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let (parts, ()) = form_parts();
    let rejection = <Form<ScalarForm, 7> as FromRequestBody<(), TestBody>>::from_request_body(&parts, TestBody::failed("diagnostic"), &())
        .await
        .expect_err("the transport failure must be retained");
    let FormRejection::Body(routerama::route::BodyRejection::Transport(error)) = rejection else {
        panic!("expected a typed transport rejection");
    };
    assert_eq!(error.error(), &TestBodyError("diagnostic"));
    assert_eq!(error.into_inner(), TestBodyError("diagnostic"));
}

#[tokio::test(flavor = "current_thread")]
async fn form_feature_supplies_route_and_query_without_requiring_a_send_body() {
    let body = TestBody::from_chunks(vec![Bytes::from_static(b"value=x")]);
    let _not_send = Rc::clone(&body.polls);
    let request = Request::post("/combined?source=query")
        .header("x-marker", "present")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .expect("the test request uses valid static metadata");
    let response = FormApi.route(request, &()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_bytes(response).await, b"query:x"[..]);
}

struct FormCatcher;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router policy methods must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = ())]
impl FormCatcher {
    #[route(POST, "/caught-form")]
    async fn form(&self, #[body] form: Form<NumberForm, 32>) -> String {
        form.count.to_string()
    }

    #[catch(FormRejection<TestBodyError>)]
    async fn catch_form(&self, _rejection: FormRejection<TestBodyError>) -> (StatusCode, &'static str) {
        (StatusCode::UNPROCESSABLE_ENTITY, "form-caught")
    }
}

#[tokio::test(flavor = "current_thread")]
async fn form_rejections_can_use_a_typed_catcher() {
    let request = request_with_content_type(
        "/caught-form",
        TestBody::from_chunks(vec![Bytes::from_static(b"count=invalid")]),
        Some("application/x-www-form-urlencoded"),
    );
    let response = FormCatcher.route(request, &()).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response_bytes(response).await, b"form-caught"[..]);
}

fn form_request(path: &str, body: TestBody) -> Request<TestBody> {
    request_with_content_type(path, body, Some("application/x-www-form-urlencoded"))
}

fn request_with_content_type(path: &str, body: TestBody, content_type: Option<&str>) -> Request<TestBody> {
    let mut request = Request::post(path).header("x-marker", "present");
    if let Some(content_type) = content_type {
        request = request.header(CONTENT_TYPE, content_type);
    }
    request.body(body).expect("the test request uses valid static metadata")
}

fn form_parts() -> (http::request::Parts, ()) {
    Request::post("/")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(())
        .expect("the test request uses valid static metadata")
        .into_parts()
}

async fn response_bytes<B>(response: http::Response<B>) -> Bytes
where
    B: http_body::Body<Data = Bytes>,
    B::Error: fmt::Debug,
{
    response
        .into_body()
        .collect()
        .await
        .expect("the generated response body succeeds")
        .to_bytes()
}
