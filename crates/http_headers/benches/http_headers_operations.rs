// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared `http_headers` operations compiled into both timing and instruction harnesses.

use std::hint::black_box;

use http::HeaderMap;
use http_headers::Field;
use http_headers::headers::{AcceptRanges, AccessControlAllowMethods, Range, Vary};

#[expect(
    clippy::inline_always,
    reason = "destruction is part of the operation while the operation boundary remains outlined"
)]
#[inline(always)]
fn consume<T>(value: T) {
    drop(black_box(value));
}

#[expect(
    clippy::inline_always,
    reason = "iteration is part of the operation while the operation boundary remains outlined"
)]
#[inline(always)]
fn consume_items(items: impl IntoIterator) -> usize {
    let mut count = 0;
    for item in items {
        black_box(item);
        count += 1;
    }
    black_box(count)
}

#[inline(never)]
pub(crate) fn headers_owned<H: headers::Header>(map: &HeaderMap) {
    let header = headers::HeaderMapExt::typed_try_get::<H>(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    consume(header);
}

#[inline(never)]
pub(crate) fn http_headers_owned<H: Field>(map: &HeaderMap) {
    let header = H::owned(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    consume(header);
}

#[inline(never)]
pub(crate) fn http_headers_borrowed<H: Field>(map: &HeaderMap) {
    let header = H::view(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    consume(header);
}

#[inline(never)]
pub(crate) fn headers_range_owned(map: &HeaderMap) {
    let header = headers::HeaderMapExt::typed_try_get::<headers::Range>(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    consume(header);
}

#[inline(never)]
pub(crate) fn http_headers_range_owned(map: &HeaderMap) {
    let header = Range::owned(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    consume(header);
}

#[inline(never)]
pub(crate) fn http_headers_range_borrowed(map: &HeaderMap) {
    let header = Range::view(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    consume(header);
}

#[inline(never)]
pub(crate) fn headers_accept_ranges(map: &HeaderMap) -> usize {
    let header = headers::HeaderMapExt::typed_try_get::<headers::AcceptRanges>(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    let result = usize::from(header.is_bytes()) | (usize::from(header.is_none()) << 1);
    consume(header);
    black_box(result)
}

#[inline(never)]
pub(crate) fn http_headers_accept_ranges_owned(map: &HeaderMap) -> usize {
    let header = AcceptRanges::owned(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    let result = usize::from(header.units().eq(["bytes"])) | (usize::from(header.is_none()) << 1);
    consume(header);
    black_box(result)
}

#[inline(never)]
pub(crate) fn http_headers_accept_ranges_borrowed(map: &HeaderMap) -> usize {
    let header = AcceptRanges::view(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    let result = usize::from(header.units().eq(["bytes"])) | (usize::from(header.is_none()) << 1);
    consume(header);
    black_box(result)
}

#[inline(never)]
pub(crate) fn headers_allow_methods(map: &HeaderMap) -> usize {
    let header = headers::HeaderMapExt::typed_try_get::<headers::AccessControlAllowMethods>(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    let count = consume_items(header.iter());
    consume(header);
    count
}

#[inline(never)]
pub(crate) fn http_headers_allow_methods_owned(map: &HeaderMap) -> usize {
    let header = AccessControlAllowMethods::owned(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    let count = consume_items(header.iter());
    consume(header);
    count
}

#[inline(never)]
pub(crate) fn http_headers_allow_methods_borrowed(map: &HeaderMap) -> usize {
    let header = AccessControlAllowMethods::view(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    let count = consume_items(header.iter());
    consume(header);
    count
}

#[inline(never)]
pub(crate) fn headers_vary(map: &HeaderMap) -> usize {
    let header = headers::HeaderMapExt::typed_try_get::<headers::Vary>(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    let count = consume_items(header.iter_strs());
    consume(header);
    count
}

#[inline(never)]
pub(crate) fn http_headers_vary_owned(map: &HeaderMap) -> usize {
    let header = Vary::owned(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    let count = consume_items(header.items());
    consume(header);
    count
}

#[inline(never)]
pub(crate) fn http_headers_vary_borrowed(map: &HeaderMap) -> usize {
    let header = Vary::view(black_box(map))
        .expect("fixture must decode")
        .expect("fixture must be present");
    let count = consume_items(header.items());
    consume(header);
    count
}
