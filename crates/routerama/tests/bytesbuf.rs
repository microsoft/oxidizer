// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "bytesbuf")]
//! `bytesbuf` response, extraction, generated-routing, and Tower integration.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Buf as _, Bytes};
use bytesbuf::{BytesBuf, BytesView};
use http::{HeaderMap, HeaderName, HeaderValue, Request};
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::BodyExt as _;
use routerama::response::bytesbuf::BytesViewBody as ResponseBytesViewBody;
#[cfg(feature = "bytesbuf-std")]
use routerama::response::bytesbuf::template::{BytesViewTemplate, html_text, json_number, json_string, unescaped_text};
use routerama::response::{BoxBody, HeterogeneousResult, IntoResponse, SendBoxBody};
#[cfg(all(feature = "json", feature = "bytesbuf-std"))]
use routerama::route::bytesbuf::JsonView;
use routerama::route::bytesbuf::{BytesViewBody, Utf8BytesViewBody};
use routerama::route::{BodyRejection, FromRequestBody, StatusCode, router};
#[cfg(feature = "tower")]
use tower_service::Service as _;

#[derive(Debug)]
struct ViewFrames {
    frames: VecDeque<Result<Frame<BytesView>, Infallible>>,
    size_hint: SizeHint,
}

impl ViewFrames {
    fn new(frames: impl IntoIterator<Item = Frame<BytesView>>) -> Self {
        Self {
            frames: frames.into_iter().map(Ok).collect(),
            size_hint: SizeHint::default(),
        }
    }

    fn with_lower_bound(mut self, lower: u64) -> Self {
        self.size_hint.set_lower(lower);
        self
    }
}

impl HttpBody for ViewFrames {
    type Data = BytesView;
    type Error = Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.frames.pop_front())
    }

    fn is_end_stream(&self) -> bool {
        self.frames.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        self.size_hint.clone()
    }
}

fn view(bytes: &'static [u8]) -> BytesView {
    BytesView::from(Bytes::from_static(bytes))
}

fn fragmented_view() -> BytesView {
    let mut buffer = BytesBuf::new();
    buffer.put_bytes(view(b"three-"));
    buffer.put_bytes(view(b"span-"));
    buffer.put_bytes(view(b"view"));
    buffer.consume_all()
}

fn request_parts() -> http::request::Parts {
    Request::new(()).into_parts().0
}

struct MixedService;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; the compatibility lint is toolchain-dependent"
)]
#[router(state = (), heterogeneous_data)]
impl MixedService {
    #[route(GET, "/view")]
    async fn view(&self) -> BytesView {
        view(b"view-response")
    }

    #[route(GET, "/bytes")]
    async fn bytes(&self) -> &'static str {
        "bytes-response"
    }

    #[route(POST, "/echo")]
    async fn echo(&self, #[body] body: BytesViewBody<64>) -> BytesView {
        body.into_inner()
    }

    #[route(GET, "/fallible")]
    async fn fallible(&self) -> HeterogeneousResult<BytesView, StatusCode> {
        Ok(view(b"fallible-view")).into()
    }

    #[route(GET, "/fragmented")]
    async fn fragmented(&self) -> BytesView {
        fragmented_view()
    }
}

#[cfg(feature = "tower")]
#[derive(Clone, Copy)]
struct MixedTowerService;

#[cfg(feature = "tower")]
#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; the compatibility lint is toolchain-dependent"
)]
#[router(state = (), heterogeneous_data, tower)]
impl MixedTowerService {
    #[route(GET, "/view")]
    async fn view(&self) -> BytesView {
        view(b"tower-view")
    }
}

#[cfg(feature = "mount")]
struct MixedMountService;

#[cfg(feature = "mount")]
#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; the compatibility lint is toolchain-dependent"
)]
#[router(state = (), erased_mounts, heterogeneous_data)]
impl MixedMountService {
    #[route(GET, "/static")]
    async fn static_route(&self) -> &'static str {
        "static"
    }
}

#[cfg(feature = "mount")]
struct ViewMount(BytesView);

#[cfg(feature = "mount")]
impl routerama::route::mount::MountedService<routerama::response::Body, ()> for ViewMount {
    type Response = BytesView;

    fn call<'a>(
        &'a self,
        _request: routerama::route::mount::MountedRequest<'a, routerama::response::Body>,
        _state: &'a (),
    ) -> impl Future<Output = Self::Response> + 'a
    where
        routerama::response::Body: 'a,
    {
        let response = self.0.clone();
        async move { response }
    }
}

#[cfg(all(feature = "bytesbuf-std", feature = "tower"))]
#[derive(Clone)]
struct HttpExtensionsService {
    builder: http_extensions::HttpBodyBuilder,
    response: BytesView,
}

#[cfg(all(feature = "bytesbuf-std", feature = "tower"))]
#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; the compatibility lint is toolchain-dependent"
)]
#[router(state = (), heterogeneous_data, tower)]
impl HttpExtensionsService {
    #[route(GET, "/body")]
    async fn body(&self) -> http::Response<http_extensions::HttpBody> {
        http::Response::new(self.builder.bytes(self.response.clone()))
    }
}

#[tokio::test]
async fn response_body_and_erasure_preserve_bytes_view_frames() {
    let original = view(b"first");
    let original_ptr = original.first_slice().as_ptr();

    let response = original.into_response();
    assert_eq!(response.body().size_hint().exact(), Some(5));
    let frame = response
        .into_body()
        .frame()
        .await
        .expect("the non-empty response yields one frame")
        .expect("the response body is infallible")
        .into_data()
        .expect("the frame contains data");
    assert_eq!(frame.to_vec(), b"first");
    assert_eq!(frame.first_slice().as_ptr(), original_ptr);

    let mut local = BoxBody::<BytesView>::new(ResponseBytesViewBody::new(view(b"local")));
    assert_eq!(
        local
            .frame()
            .await
            .expect("the local body yields one frame")
            .expect("the local body is infallible")
            .into_data()
            .expect("the local frame contains data"),
        view(b"local")
    );

    let mut send = SendBoxBody::<BytesView>::new(ResponseBytesViewBody::new(view(b"send")));
    assert_eq!(
        send.frame()
            .await
            .expect("the send body yields one frame")
            .expect("the send body is infallible")
            .into_data()
            .expect("the send frame contains data"),
        view(b"send")
    );
}

#[tokio::test]
async fn bounded_extraction_preserves_single_and_fragmented_storage() {
    let single = view(b"single");
    let single_ptr = single.first_slice().as_ptr();
    let extracted = BytesViewBody::<6>::from_request_body(&request_parts(), ViewFrames::new([Frame::data(single)]), &())
        .await
        .expect("an exact-limit body succeeds");
    assert_eq!(extracted.view().to_vec(), b"single");
    assert_eq!(extracted.first_slice().as_ptr(), single_ptr);

    let first = view(b"first");
    let second = view(b"second");
    let pointers = [first.first_slice().as_ptr(), second.first_slice().as_ptr()];
    let mut trailers = HeaderMap::new();
    trailers.insert(HeaderName::from_static("x-finished"), HeaderValue::from_static("yes"));
    let extracted = BytesViewBody::<11>::from_request_body(
        &request_parts(),
        ViewFrames::new([Frame::data(first), Frame::trailers(trailers), Frame::data(second)]),
        &(),
    )
    .await
    .expect("fragmented data at the limit succeeds");
    assert_eq!(extracted.view().to_vec(), b"firstsecond");
    let slices: Vec<_> = extracted.slices().map(|(slice, _)| slice.as_ptr()).collect();
    assert_eq!(slices, pointers);
}

#[tokio::test]
async fn bounded_extraction_rejects_hints_and_frames_over_the_limit() {
    let hinted = BytesViewBody::<4>::from_request_body(&request_parts(), ViewFrames::new([]).with_lower_bound(5), &())
        .await
        .expect_err("an excessive lower bound is rejected before polling");
    assert!(matches!(hinted, BodyRejection::TooLarge(error) if error.received() == 5));

    let yielded = BytesViewBody::<4>::from_request_body(&request_parts(), ViewFrames::new([Frame::data(view(b"12345"))]), &())
        .await
        .expect_err("an excessive frame is rejected");
    assert!(matches!(yielded, BodyRejection::TooLarge(error) if error.received() == 5));
}

#[tokio::test]
async fn bounded_extraction_rejects_a_span_flood_before_bookkeeping_outgrows_the_payload() {
    // Chunked transfer-encoding lets a client choose the framing, and every frame
    // appends a span whose bookkeeping costs far more than the byte it carries.
    // Collecting 64 KiB as 65 536 one-byte frames once allocated 6 290 944 bytes,
    // a 96x amplification over the payload. The frame budget is `LIMIT / 64 + 64`,
    // so a 64 KiB limit reads at most 1 088 frames.
    const LIMIT: usize = 64 * 1024;
    const BUDGET: usize = LIMIT / 64 + 64;

    let frames = (0..=BUDGET).map(|_| Frame::data(view(b"x")));
    let rejection = BytesViewBody::<LIMIT>::from_request_body(&request_parts(), ViewFrames::new(frames), &())
        .await
        .expect_err("a one-byte-per-frame flood must be refused by the frame budget");

    assert!(
        matches!(rejection, BodyRejection::TooManyFrames(error) if error.limit() == BUDGET && error.received() == BUDGET + 1),
        "expected a frame-budget rejection, got {rejection:?}"
    );
}

#[tokio::test]
async fn bounded_extraction_accepts_a_body_that_fills_the_frame_budget() {
    const LIMIT: usize = 64 * 1024;
    const BUDGET: usize = LIMIT / 64 + 64;

    let frames = (0..BUDGET).map(|_| Frame::data(view(b"x")));
    let extracted = BytesViewBody::<LIMIT>::from_request_body(&request_parts(), ViewFrames::new(frames), &())
        .await
        .expect("a body exactly at the frame budget is accepted");

    assert_eq!(extracted.view().len(), BUDGET);
}

#[tokio::test]
async fn utf8_validation_handles_sequences_split_across_views() {
    let valid = Utf8BytesViewBody::<4>::from_request_body(
        &request_parts(),
        ViewFrames::new([Frame::data(view(&[0xf0, 0x9f])), Frame::data(view(&[0xa6, 0x80]))]),
        &(),
    )
    .await
    .expect("a split UTF-8 sequence is valid");
    assert_eq!(valid.view().to_vec(), "🦀".as_bytes());

    let invalid = Utf8BytesViewBody::<3>::from_request_body(
        &request_parts(),
        ViewFrames::new([Frame::data(view(&[b'a', 0xe2])), Frame::data(view(&[0x28]))]),
        &(),
    )
    .await
    .expect_err("an invalid continuation byte is rejected");
    assert!(matches!(
        invalid,
        BodyRejection::InvalidUtf8(error) if error.valid_up_to() == 1
    ));
}

#[cfg(all(feature = "json", feature = "bytesbuf-std"))]
#[tokio::test]
async fn owned_json_decodes_across_view_boundaries_without_coalescing() {
    #[derive(Debug, serde::Deserialize, PartialEq, Eq)]
    struct Payload {
        id: u32,
        name: String,
    }

    let mut parts = request_parts();
    parts
        .headers
        .insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let decoded = JsonView::<Payload, 64>::from_request_body(
        &parts,
        ViewFrames::new([Frame::data(view(br#"{"id":42,"na"#)), Frame::data(view(br#"me":"routerama"}"#))]),
        &(),
    )
    .await
    .expect("fragmented JSON decodes through BytesView's reader");

    assert_eq!(
        decoded.into_inner(),
        Payload {
            id: 42,
            name: "routerama".to_owned(),
        }
    );
}

#[tokio::test]
async fn generated_router_composes_bytes_and_bytes_view_without_normalizing() {
    let service = MixedService;

    let view_response = service
        .route(Request::get("/view").body(ViewFrames::new([])).expect("valid request"), &())
        .await;
    assert_eq!(
        view_response
            .into_body()
            .collect()
            .await
            .expect("the generated view response succeeds")
            .to_bytes(),
        b"view-response"[..]
    );

    let bytes_response = service
        .route(Request::get("/bytes").body(ViewFrames::new([])).expect("valid request"), &())
        .await;
    assert_eq!(
        bytes_response
            .into_body()
            .collect()
            .await
            .expect("the generated Bytes response succeeds")
            .to_bytes(),
        b"bytes-response"[..]
    );

    let echo_response = service
        .route(
            Request::post("/echo")
                .body(ViewFrames::new([Frame::data(view(b"echo-")), Frame::data(view(b"response"))]))
                .expect("valid request"),
            &(),
        )
        .await;
    assert_eq!(
        echo_response
            .into_body()
            .collect()
            .await
            .expect("the generated echo response succeeds")
            .to_bytes(),
        b"echo-response"[..]
    );

    let missing = service
        .route(Request::get("/missing").body(ViewFrames::new([])).expect("valid request"), &())
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let fallible = service
        .route(Request::get("/fallible").body(ViewFrames::new([])).expect("valid request"), &())
        .await;
    assert_eq!(
        fallible
            .into_body()
            .collect()
            .await
            .expect("the heterogeneous result body succeeds")
            .to_bytes(),
        b"fallible-view"[..]
    );
}

#[tokio::test]
#[cfg(feature = "bytesbuf-std")]
async fn generated_data_sum_preserves_vectored_bytes_view_access() {
    let response = MixedService
        .route(Request::get("/fragmented").body(ViewFrames::new([])).expect("valid request"), &())
        .await;
    let frame = response
        .into_body()
        .frame()
        .await
        .expect("the response yields one frame")
        .expect("the response frame succeeds");
    let Ok(data) = frame.into_data() else {
        panic!("the frame contains data");
    };

    let mut slices = [std::io::IoSlice::new(&[]), std::io::IoSlice::new(&[]), std::io::IoSlice::new(&[])];
    assert_eq!(data.chunks_vectored(&mut slices), 3);
    assert_eq!(&*slices[0], b"three-");
    assert_eq!(&*slices[1], b"span-");
    assert_eq!(&*slices[2], b"view");
}

#[cfg(feature = "mount")]
#[tokio::test]
async fn erased_mounts_preserve_bytes_view_data() {
    use routerama::response::Response;
    use routerama::route::mount::{ErasedMountRouter, ErasedMountService};

    let mounted_view = fragmented_view();
    let expected_pointer = mounted_view.first_slice().as_ptr();
    let mounted = ErasedMountService::<routerama::response::Body, (), BytesView>::new(ViewMount(mounted_view));
    let mounts = ErasedMountRouter::builder_with_fallback(|status| {
        let mut response = Response::new(BoxBody::new(ResponseBytesViewBody::empty()));
        *response.status_mut() = status;
        response
    })
    .mount("GET", "/mounted", mounted)
    .build()
    .expect("the mount registration is valid");

    let response = MixedMountService
        .route_with_erased_mounts(
            Request::get("/mounted")
                .body(routerama::response::Body::empty())
                .expect("valid request"),
            &(),
            &mounts,
        )
        .await;
    assert_eq!(first_chunk_pointer(response).await, expected_pointer);
}

#[cfg(feature = "bytesbuf-std")]
#[test]
fn prepared_templates_reuse_static_views_and_encode_typed_slots() {
    let memory = bytesbuf::mem::GlobalPool::new();
    let json = BytesViewTemplate::prepare(&memory, [br#"{"id":"#, br#","name":"#, b"}"]);
    let fixed_pointers: Vec<_> = json.fragments().iter().map(|fragment| fragment.first_slice().as_ptr()).collect();

    let rendered = json.render(&memory, (json_number(42_u32), json_string("a\n\"b")));
    assert_eq!(rendered.to_vec(), br#"{"id":42,"name":"a\n\"b"}"#);
    let rendered_pointers: Vec<_> = rendered.slices().map(|(slice, _)| slice.as_ptr()).collect();
    for pointer in fixed_pointers {
        assert!(rendered_pointers.contains(&pointer));
    }

    let plain = BytesViewTemplate::prepare(&memory, [b"hello ", b"!"]);
    assert_eq!(plain.render(&memory, (unescaped_text("routerama"),)).to_vec(), b"hello routerama!");
}

#[cfg(feature = "bytesbuf-std")]
#[test]
fn the_bytes_view_template_offers_an_escaping_html_slot() {
    // A `BytesViewTemplate` built from HTML fragments must have an escaping slot
    // to reach for; the verbatim slot is named `unescaped_text` so that choosing
    // it is explicit at the call site.
    let memory = bytesbuf::mem::GlobalPool::new();
    let page = BytesViewTemplate::prepare(&memory, [b"<p>", b"</p>"]);
    let injection = "<script>alert('x')&\"</script>";

    let escaped = page.render(&memory, (html_text(injection),));
    assert_eq!(
        escaped.to_vec(),
        b"<p>&lt;script&gt;alert(&#39;x&#39;)&amp;&quot;&lt;/script&gt;</p>".to_vec()
    );

    let verbatim = page.render(&memory, (unescaped_text(injection),));
    assert_eq!(verbatim.to_vec(), format!("<p>{injection}</p>").into_bytes());
}

#[cfg(feature = "tower")]
#[tokio::test]
async fn generated_exact_tower_service_preserves_heterogeneous_data() {
    let mut service = MixedTowerService::tower_service::<routerama::response::Body, _, _>(MixedTowerService, ());
    let response = service
        .call(
            Request::get("/view")
                .body(routerama::response::Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("routing is infallible");

    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .expect("the exact Tower body succeeds")
            .to_bytes(),
        b"tower-view"[..]
    );
}

#[cfg(all(feature = "bytesbuf-std", feature = "tower"))]
#[tokio::test]
async fn http_extensions_body_survives_generated_and_boxed_tower_boundaries() {
    use routerama::route::tower::RouteService;

    let response = fragmented_view();
    let expected_pointer = response.first_slice().as_ptr();
    let service = HttpExtensionsService {
        builder: http_extensions::HttpBodyBuilder::new_fake(),
        response,
    };

    let mut exact = HttpExtensionsService::tower_service::<http_extensions::HttpBody, _, _>(service.clone(), ());
    let exact_response = exact
        .call(
            Request::get("/body")
                .body(http_extensions::HttpBodyBuilder::new_fake().empty())
                .expect("valid request"),
        )
        .await
        .expect("routing is infallible");
    assert_eq!(first_chunk_pointer(exact_response).await, expected_pointer);

    let mut boxed = RouteService::new(
        service,
        (),
        |service: HttpExtensionsService, state: (), request: Request<http_extensions::HttpBody>| async move {
            service.route(request, &state).await
        },
    )
    .send_boxed_body();
    let boxed_response = boxed
        .call(
            Request::get("/body")
                .body(http_extensions::HttpBodyBuilder::new_fake().empty())
                .expect("valid request"),
        )
        .await
        .expect("routing is infallible");
    assert_eq!(first_chunk_pointer(boxed_response).await, expected_pointer);
}

#[expect(clippy::panic, reason = "the fixture constructs a one-frame data response")]
async fn first_chunk_pointer<B>(response: http::Response<B>) -> *const u8
where
    B: HttpBody,
    B::Data: bytes::Buf,
    B::Error: core::fmt::Debug,
{
    let body = response.into_body();
    let mut body = core::pin::pin!(body);
    let frame = core::future::poll_fn(|context| body.as_mut().poll_frame(context))
        .await
        .expect("the body yields one frame")
        .expect("the frame succeeds");
    let Ok(data) = frame.into_data() else {
        panic!("the frame contains data");
    };
    data.chunk().as_ptr()
}
