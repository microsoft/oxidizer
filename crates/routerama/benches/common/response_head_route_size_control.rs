// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared generated-router workload for response-head compile/size controls.
// Each target defines `Api` and `API` before including this file.

use std::future::Future;
use std::task::{Context, Poll, Waker};

use http::Request;

const PATHS: [&str; 4] = ["/headers/0", "/headers/1", "/headers/4", "/headers/16"];
const COUNTS: [usize; 4] = [0, 1, 4, 16];

#[expect(
    clippy::panic,
    reason = "a pending generated size-control response is a benchmark invariant violation"
)]
fn run_ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the generated response-head size control completes in one poll"),
    }
}

fn main() {
    for (path, expected) in PATHS.into_iter().zip(COUNTS) {
        let request = Request::get(path)
            .body(routerama::response::Body::empty())
            .expect("the response-head size-control request is valid");
        let response = run_ready(API.route(request, &()));
        assert_eq!(response.headers().len(), expected);
        std::hint::black_box(response);
    }
}
