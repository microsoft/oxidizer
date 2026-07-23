// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Semantic contracts for route-predicate overlap benchmark fixtures.

#![cfg(not(miri))]
#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]

use alloc_tracker::{Allocator, Session};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("../benches/common/predicate_overlap_scenarios.rs");

#[test]
fn all_overlap_sizes_preserve_winners_and_rejection_precedence() {
    assert_equivalent();
}

fn overlap_request(
    uri: &'static str,
    host: Option<&'static str>,
    content_type: Option<&'static str>,
    accept: Option<&'static str>,
) -> Request<()> {
    let mut request = Request::builder().method("POST").uri(uri);
    if let Some(host) = host {
        request = request.header("host", HeaderValue::from_static(host));
    }
    if let Some(content_type) = content_type {
        request = request.header("content-type", HeaderValue::from_static(content_type));
    }
    if let Some(accept) = accept {
        request = request.header("accept", HeaderValue::from_static(accept));
    }
    request.body(()).expect("extended overlap request metadata is valid")
}

fn run_overlap(size: GroupSize, request: Request<()>) -> Observation {
    match size {
        GroupSize::Two => observe(run_ready(OVERLAP_2.route(request, &()))),
        GroupSize::Eight => observe(run_ready(OVERLAP_8.route(request, &()))),
        GroupSize::ThirtyTwo => observe(run_ready(OVERLAP_32.route(request, &()))),
    }
}

#[test]
fn accept_overlap_preserves_wildcard_quality_specificity_and_field_validation() {
    for size in GroupSize::ALL {
        for accept in [None, Some("*/*"), Some("application/*")] {
            assert_eq!(
                run_overlap(
                    size,
                    overlap_request("/overlap", Some("api.example"), Some("application/json"), accept)
                ),
                (200, Some(0), 1)
            );
        }
        for accept in [
            "application/x-routerama-00;q=0.1, application/x-routerama-01;q=1",
            "application/x-routerama-00;q=0, application/x-routerama-00;q=0.4",
        ] {
            assert_eq!(
                run_overlap(
                    size,
                    overlap_request("/overlap", Some("api.example"), Some("application/json"), Some(accept))
                ),
                (200, Some(0), 1),
                "{}/accept={accept}",
                size.name()
            );
        }
        assert_eq!(
            run_overlap(
                size,
                overlap_request(
                    "/overlap",
                    Some("api.example"),
                    Some("application/json"),
                    Some("*/*;q=1, application/x-routerama-00;q=0"),
                )
            ),
            (200, Some(1), 1)
        );
        assert_eq!(
            run_overlap(
                size,
                overlap_request(
                    "/overlap",
                    Some("api.example"),
                    Some("application/json"),
                    Some("*/*;q=1, application/*;q=0"),
                )
            ),
            (406, None, 0)
        );

        let mut malformed = overlap_request(
            "/overlap",
            Some("api.example"),
            Some("application/json"),
            Some("application/x-routerama-00"),
        );
        malformed.headers_mut().append("accept", HeaderValue::from_static("application/"));
        assert_eq!(run_overlap(size, malformed), (406, None, 0));
    }
}

#[test]
fn overlap_host_and_content_type_preserve_malformed_repeated_and_authority_behavior() {
    for size in GroupSize::ALL {
        assert_eq!(
            run_overlap(
                size,
                overlap_request(
                    "/overlap",
                    Some("API.EXAMPLE"),
                    Some(" Application/JSON ; charset=\"utf-8\" "),
                    Some("application/x-routerama-00"),
                )
            ),
            (200, Some(0), 1)
        );
        assert_eq!(
            run_overlap(
                size,
                overlap_request(
                    "/overlap",
                    Some("api example"),
                    Some("application/json"),
                    Some("application/x-routerama-00"),
                )
            ),
            (404, None, 0)
        );
        assert_eq!(
            run_overlap(
                size,
                overlap_request(
                    "/overlap",
                    Some("api.example"),
                    Some("application/json; charset"),
                    Some("application/x-routerama-00"),
                )
            ),
            (415, None, 0)
        );
        assert_eq!(
            run_overlap(
                size,
                overlap_request("/overlap", Some("api.example"), None, Some("application/x-routerama-00"),)
            ),
            (415, None, 0)
        );

        let mut absolute = overlap_request(
            "http://API.EXAMPLE/overlap",
            Some("wrong.example"),
            Some("application/json"),
            Some("application/x-routerama-00"),
        );
        absolute
            .headers_mut()
            .append("host", HeaderValue::from_static("also-wrong.example"));
        assert_eq!(run_overlap(size, absolute), (200, Some(0), 1));
    }
}

struct PredicateKindOverlap;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl PredicateKindOverlap {
    #[route(POST, "/hosts", host = "z.example", priority = 30)]
    async fn host_first(&self) -> Bytes {
        Bytes::from_static(&[0])
    }

    #[route(POST, "/hosts", host = "a.example", priority = 20)]
    async fn host_middle(&self) -> Bytes {
        Bytes::from_static(&[1])
    }

    #[route(POST, "/hosts", host = "m.example", priority = 10)]
    async fn host_last(&self) -> Bytes {
        Bytes::from_static(&[2])
    }

    #[route(POST, "/consumes", consumes = "text/plain", priority = 30)]
    async fn consumes_first(&self) -> Bytes {
        Bytes::from_static(&[0])
    }

    #[route(POST, "/consumes", consumes = "application/json", priority = 20)]
    async fn consumes_middle(&self) -> Bytes {
        Bytes::from_static(&[1])
    }

    #[route(POST, "/consumes", consumes = "image/png", priority = 10)]
    async fn consumes_last(&self) -> Bytes {
        Bytes::from_static(&[2])
    }
}

static PREDICATE_KIND_OVERLAP: PredicateKindOverlap = PredicateKindOverlap;

fn run_predicate_kind(request: Request<()>) -> Observation {
    observe(run_ready(PREDICATE_KIND_OVERLAP.route(request, &())))
}

#[test]
fn distinct_host_and_consumes_constants_select_in_priority_order() {
    for (host, winner) in [("z.example", 0), ("A.EXAMPLE", 1), ("m.example", 2)] {
        assert_eq!(
            run_predicate_kind(overlap_request("/hosts", Some(host), None, None)),
            (200, Some(winner), 1)
        );
    }
    assert_eq!(
        run_predicate_kind(overlap_request("/hosts", Some("missing.example"), None, None)),
        (404, None, 0)
    );

    for (content_type, winner) in [("text/plain", 0), (" Application/JSON ; charset=utf-8 ", 1), ("image/png", 2)] {
        assert_eq!(
            run_predicate_kind(overlap_request("/consumes", None, Some(content_type), None)),
            (200, Some(winner), 1)
        );
    }
    assert_eq!(
        run_predicate_kind(overlap_request("/consumes", None, Some("application/xml"), None)),
        (415, None, 0)
    );
}

#[derive(Clone, Copy)]
struct AllocationStats {
    allocations: u64,
    bytes: u64,
}

fn allocation_stats(report: &alloc_tracker::Report) -> AllocationStats {
    let (_, operation) = report
        .operations()
        .find(|(name, _)| *name == "measured")
        .expect("the overlap allocation diagnostic records its measured operation");
    AllocationStats {
        allocations: operation.total_allocations_count(),
        bytes: operation.total_bytes_allocated(),
    }
}

#[test]
fn overlap_rejections_stay_allocation_free_and_success_header_cost_is_unchanged() {
    for size in GroupSize::ALL {
        for scenario in Scenario::ALL {
            let _ = std::hint::black_box(run_prepared(prepare(size, scenario)));
            let prepared = std::hint::black_box(prepare(size, scenario));
            let session = Session::new().no_stdout().no_file();
            let operation = session.operation("measured");
            {
                let _span = operation.measure_thread().iterations(1);
                std::hint::black_box(run_prepared(prepared));
            }
            let stats = allocation_stats(&session.to_report());
            let expected = u64::from(scenario.winner(size).is_some()) * 2;
            assert_eq!(
                stats.allocations,
                expected,
                "{}/{} allocated {} times ({} bytes); expected {expected}",
                size.name(),
                scenario.name(),
                stats.allocations,
                stats.bytes
            );
        }
    }
}
