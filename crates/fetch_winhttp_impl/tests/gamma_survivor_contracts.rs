// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process scripted response fixtures, without starting a server.

#![cfg(all(windows, feature = "private-test-util"))]

use bytes::Bytes;
use fetch_winhttp_impl::testing::{ResponseFrame, ResponsePlan, ResponseScript, collect_frames};
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};

#[test]
fn gamma_survivor_winhttp_builder_reports_exact_runtime_and_transport() {
    use fetch_winhttp_impl::{HttpClientWinHttpExt, WinHttpDeps};

    let deps = WinHttpDeps::builder(
        tick::Clock::new_frozen(),
        bytesbuf::mem::OpaqueMemory::new(bytesbuf::mem::GlobalPool::new()),
        observed::Sink::noop(),
    )
    .build();
    let builder = fetch::HttpClient::builder_winhttp(deps);
    let debug = format!("{builder:?}");
    assert!(debug.contains("runtime: \"winhttp\""), "{debug}");
    assert!(debug.contains("transport: \"winhttp\""), "{debug}");
}

#[test]
fn gamma_survivor_response_plan_data_headers_trailers_and_stall() {
    let mut trailers = HeaderMap::new();
    trailers.insert("x-trailer", HeaderValue::from_static("finished"));
    let plan = ResponsePlan::chunks([Bytes::from_static(b"ab"), Bytes::from_static(b"cde")])
        .header(HeaderName::from_static("x-repeat"), HeaderValue::from_static("one"))
        .header(HeaderName::from_static("x-repeat"), HeaderValue::from_static("two"))
        .trailers(trailers.clone());
    assert_eq!(plan.body_length(), 5);
    assert_eq!(plan.status, StatusCode::OK);
    assert!(!plan.stall_after_frames);
    assert_eq!(
        plan.headers
            .get_all("x-repeat")
            .iter()
            .map(HeaderValue::as_bytes)
            .collect::<Vec<_>>(),
        [b"one", b"two"]
    );
    assert!(matches!(plan.frames.last(), Some(ResponseFrame::Trailers(value)) if value == &trailers));
    assert!(plan.stall_after_frames().stall_after_frames);
    assert!(!ResponsePlan::ok("x").stall_after_frames);
    assert!(!ResponsePlan::status(StatusCode::BAD_REQUEST).stall_after_frames);
    let _ = ResponseScript::Repeat(ResponsePlan::ok("x"));
}

#[test]
fn gamma_survivor_collect_frames_keeps_data_order_and_trailers() {
    let memory = bytesbuf::mem::OpaqueMemory::new(bytesbuf::mem::GlobalPool::new());
    let builder = fetch::HttpBodyBuilder::new(memory.clone(), &tick::Clock::new_frozen());
    let mut trailers = HeaderMap::new();
    trailers.insert("x-end", HeaderValue::from_static("done"));
    let frames = [
        Ok::<_, fetch::HttpError>(http_body::Frame::data(bytesbuf::BytesView::copied_from_slice(b"ab", &memory))),
        Ok(http_body::Frame::data(bytesbuf::BytesView::copied_from_slice(b"cd", &memory))),
        Ok(http_body::Frame::trailers(trailers.clone())),
    ];
    let body = builder.body(http_body_util::StreamBody::new(futures::stream::iter(frames)));
    let (data, actual_trailers) = futures::executor::block_on(collect_frames(body));
    assert_eq!(data, b"abcd");
    assert_eq!(actual_trailers, Some(trailers));
}
