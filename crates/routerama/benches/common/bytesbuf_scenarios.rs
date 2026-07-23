// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared BytesView response, routing, extraction, and template scenarios.
// Payload and service construction stay outside each measured operation.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::io::IoSlice;
use std::pin::{Pin, pin};
use std::task::{Context, Poll, Waker};

use bytesbuf::mem::GlobalPool;
use bytesbuf::{BytesBuf, BytesView};
use http::{Request, Response};
use http_body::{Body as HttpBody, Frame, SizeHint};
use routerama::response::IntoResponse as _;
use routerama::response::bytesbuf::template::{BytesViewTemplate, json_number, json_string};
use routerama::route::bytesbuf::BytesViewBody;
use routerama::route::tower::RouteService;
use routerama::route::{FromRequestBody as _, router};
use tower_service::Service as _;

const SPAN_BYTES: &[u8] = b"0123456789abcdef";
const LIMIT: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpanCount {
    One,
    Three,
    Eight,
    Nine,
    ThirtyTwo,
}

impl SpanCount {
    const ALL: [Self; 5] = [Self::One, Self::Three, Self::Eight, Self::Nine, Self::ThirtyTwo];

    const fn get(self) -> usize {
        match self {
            Self::One => 1,
            Self::Three => 3,
            Self::Eight => 8,
            Self::Nine => 9,
            Self::ThirtyTwo => 32,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::One => "1_span",
            Self::Three => "3_spans",
            Self::Eight => "8_spans",
            Self::Nine => "9_spans",
            Self::ThirtyTwo => "32_spans",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Observation {
    length: usize,
    chunks: usize,
    hash: u64,
}

#[derive(Debug)]
struct PreparedView {
    view: BytesView,
}

fn prepare_view(count: SpanCount) -> PreparedView {
    let memory = GlobalPool::new();
    let mut output = BytesBuf::new();
    for _ in 0..count.get() {
        output.put_bytes(BytesView::copied_from_slice(SPAN_BYTES, &memory));
    }
    PreparedView {
        view: output.consume_all(),
    }
}

fn observe(data: &impl bytes::Buf) -> Observation {
    let mut slices = [IoSlice::new(&[]); 64];
    let chunks = data.chunks_vectored(&mut slices);
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut length = 0;
    for slice in &slices[..chunks] {
        length += slice.len();
        for byte in &**slice {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    Observation { length, chunks, hash }
}

fn expected(count: SpanCount) -> Observation {
    observe(&prepare_view(count).view)
}

fn response_observation<B>(response: Response<B>) -> Observation
where
    B: HttpBody,
    B::Data: bytes::Buf,
    B::Error: core::fmt::Debug,
{
    let body = response.into_body();
    let mut body = pin!(body);
    let frame = match body.as_mut().poll_frame(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Some(Ok(frame))) => frame,
        Poll::Ready(Some(Err(error))) => panic!("prepared response body failed: {error:?}"),
        Poll::Ready(None) => panic!("prepared response yielded no frame"),
        Poll::Pending => panic!("prepared response unexpectedly returned pending"),
    };
    let Ok(data) = frame.into_data() else {
        panic!("prepared response yielded trailers instead of data");
    };
    observe(&data)
}

fn run_direct(prepared: PreparedView) -> Observation {
    response_observation(prepared.view.into_response())
}

fn run_conversion(prepared: PreparedView) -> Observation {
    observe(&prepared.view.to_bytes())
}

#[derive(Clone, Debug)]
struct GeneratedService {
    view: BytesView,
}

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; the compatibility lint is toolchain-dependent"
)]
#[router(state = (), heterogeneous_data, tower)]
impl GeneratedService {
    #[route(GET, "/view")]
    async fn view(&self) -> BytesView {
        self.view.clone()
    }

    #[route(GET, "/bytes")]
    async fn bytes(&self) -> &'static str {
        "bytes"
    }
}

fn request() -> Request<routerama::response::Body> {
    Request::get("/view")
        .body(routerama::response::Body::empty())
        .expect("static benchmark request is valid")
}

#[derive(Debug)]
struct PreparedRoute {
    view: BytesView,
    request: Request<routerama::response::Body>,
}

fn prepare_route(count: SpanCount) -> PreparedRoute {
    PreparedRoute {
        view: prepare_view(count).view,
        request: request(),
    }
}

fn ready<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("in-memory benchmark future unexpectedly returned pending"),
    }
}

fn run_generated(prepared: PreparedRoute) -> Observation {
    let service = GeneratedService { view: prepared.view };
    response_observation(ready(service.route(prepared.request, &())))
}

fn run_exact_tower(prepared: PreparedRoute) -> Observation {
    let service = GeneratedService { view: prepared.view };
    let mut tower = GeneratedService::tower_service::<routerama::response::Body, _, _>(service, ());
    response_observation(ready(tower.call(prepared.request)).expect("routing is infallible"))
}

fn run_boxed_tower(prepared: PreparedRoute) -> Observation {
    let service = GeneratedService { view: prepared.view };
    let mut tower = RouteService::new(
        service,
        (),
        |service: GeneratedService, state: (), request: Request<routerama::response::Body>| async move {
            service.route(request, &state).await
        },
    )
    .send_boxed_body();
    response_observation(ready(tower.call(prepared.request)).expect("routing is infallible"))
}

#[derive(Debug)]
struct ViewFrames {
    frames: VecDeque<BytesView>,
    remaining: usize,
}

impl HttpBody for ViewFrames {
    type Data = BytesView;
    type Error = Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let frame = self.frames.pop_front();
        if let Some(view) = &frame {
            self.remaining -= view.len();
        }
        Poll::Ready(frame.map(|view| Ok(Frame::data(view))))
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.remaining as u64)
    }
}

#[derive(Debug)]
struct PreparedExtraction {
    parts: http::request::Parts,
    body: ViewFrames,
}

fn prepare_extraction(count: SpanCount) -> PreparedExtraction {
    let memory = GlobalPool::new();
    let frames = (0..count.get())
        .map(|_| BytesView::copied_from_slice(SPAN_BYTES, &memory))
        .collect();
    PreparedExtraction {
        parts: Request::new(()).into_parts().0,
        body: ViewFrames {
            frames,
            remaining: count.get() * SPAN_BYTES.len(),
        },
    }
}

fn run_extraction(prepared: PreparedExtraction) -> Observation {
    let extracted = ready(BytesViewBody::<LIMIT>::from_request_body(
        &prepared.parts,
        prepared.body,
        &(),
    ))
        .expect("the prepared body is within the extraction limit");
    observe(extracted.view())
}

#[derive(Debug)]
struct PreparedTemplate {
    memory: GlobalPool,
    template: BytesViewTemplate<3>,
}

fn prepare_template() -> PreparedTemplate {
    let memory = GlobalPool::new();
    let template = BytesViewTemplate::prepare(&memory, [br#"{"id":"#, br#","name":"#, b"}"]);
    PreparedTemplate { memory, template }
}

fn run_template(prepared: PreparedTemplate) -> Observation {
    let view = prepared
        .template
        .render(&prepared.memory, (json_number(42_u64), json_string("routerama")));
    observe(&view)
}

fn assert_equivalent() {
    for count in SpanCount::ALL {
        let expected = expected(count);
        assert_eq!(run_direct(prepare_view(count)), expected);
        let converted = run_conversion(prepare_view(count));
        assert_eq!(
            (converted.length, converted.hash),
            (expected.length, expected.hash)
        );
        assert_eq!(converted.chunks, 1);
        assert_eq!(run_generated(prepare_route(count)), expected);
        assert_eq!(run_exact_tower(prepare_route(count)), expected);
        assert_eq!(run_boxed_tower(prepare_route(count)), expected);
        assert_eq!(run_extraction(prepare_extraction(count)), expected);
    }
    assert_eq!(run_template(prepare_template()).length, br#"{"id":42,"name":"routerama"}"#.len());
}
