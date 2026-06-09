// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::hint::black_box;
use std::str::FromStr;

use bytes::{Bytes, BytesMut};
use gungraun::{library_benchmark, library_benchmark_group};
use http::header::{HeaderMap, HeaderName, HeaderValue};
use http::{Method, Request, Response, StatusCode, Version};
use jiff::civil::{Date, DateTime, ISOWeekDate, Time};
use jiff::{SignedDuration, Span, Timestamp};
use thread_aware::ThreadAware;
use thread_aware::affinity::{Affinity, pinned_affinities};
use uuid::Uuid;

const N: usize = 1_000;

// ===== setup helpers =====

fn affinity_pair() -> (Affinity, Affinity) {
    let a = pinned_affinities(&[1, 1]);
    (a[0], a[1])
}

/// A `ThreadAware` body whose `relocate` bumps a counter. See the criterion
/// bench for the rationale.
#[derive(Default)]
struct CounterBody(u64);

impl ThreadAware for CounterBody {
    fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
        self.0 = self.0.wrapping_add(1);
    }
}

fn header_map(n: usize) -> HeaderMap {
    let mut m = HeaderMap::with_capacity(n);
    for i in 0..n {
        let name = HeaderName::from_str(&format!("x-bench-header-{i}")).expect("valid header name");
        let value = HeaderValue::from_str(&format!("value-{i}")).expect("valid header value");
        m.insert(name, value);
    }
    m
}

fn request<T>(body: T, header_count: usize) -> Request<T> {
    let mut req = Request::new(body);
    *req.headers_mut() = header_map(header_count);
    req
}

fn response<T>(body: T, header_count: usize) -> Response<T> {
    let mut resp = Response::new(body);
    *resp.headers_mut() = header_map(header_count);
    resp
}

// ===== noop_value =====

macro_rules! noop_bench {
    ($name:ident, $ty:ty, $init:expr) => {
        #[library_benchmark]
        #[bench::run($init, affinity_pair())]
        fn $name(mut v: $ty, sd: (Affinity, Affinity)) -> $ty {
            let (src, dst) = sd;
            for _ in 0..N {
                black_box(&mut v).relocate(black_box(Some(src)), black_box(dst));
            }
            v
        }
    };
}

noop_bench!(noop_value_status_code, StatusCode, StatusCode::OK);
noop_bench!(noop_value_method, Method, Method::GET);
noop_bench!(noop_value_version, Version, Version::HTTP_11);
noop_bench!(noop_value_header_name, HeaderName, HeaderName::from_static("x-bench"));
noop_bench!(noop_value_header_value, HeaderValue, HeaderValue::from_static("bench-value"));
noop_bench!(noop_value_bytes_empty, Bytes, Bytes::new());
noop_bench!(noop_value_bytes_4kb, Bytes, Bytes::from(vec![0_u8; 4096]));
noop_bench!(noop_value_bytes_mut_4kb, BytesMut, BytesMut::from(&[0_u8; 4096][..]));
noop_bench!(noop_value_uuid, Uuid, Uuid::nil());
noop_bench!(noop_value_timestamp, Timestamp, Timestamp::UNIX_EPOCH);
noop_bench!(noop_value_signed_duration, SignedDuration, SignedDuration::ZERO);
noop_bench!(noop_value_span, Span, Span::new());
noop_bench!(noop_value_date, Date, Date::constant(2026, 6, 8));
noop_bench!(noop_value_time, Time, Time::constant(12, 0, 0, 0));
noop_bench!(noop_value_datetime, DateTime, DateTime::constant(2026, 6, 8, 12, 0, 0, 0));
noop_bench!(
    noop_value_iso_week_date,
    ISOWeekDate,
    ISOWeekDate::new(2026, 23, jiff::civil::Weekday::Monday).expect("valid ISO week date")
);

// ===== header_map — cost must be identical across sizes =====

#[library_benchmark]
#[bench::run(header_map(0), affinity_pair())]
fn header_map_empty(mut m: HeaderMap, sd: (Affinity, Affinity)) -> HeaderMap {
    let (src, dst) = sd;
    for _ in 0..N {
        black_box(&mut m).relocate(black_box(Some(src)), black_box(dst));
    }
    m
}

#[library_benchmark]
#[bench::run(header_map(8), affinity_pair())]
fn header_map_populated_8(mut m: HeaderMap, sd: (Affinity, Affinity)) -> HeaderMap {
    let (src, dst) = sd;
    for _ in 0..N {
        black_box(&mut m).relocate(black_box(Some(src)), black_box(dst));
    }
    m
}

#[library_benchmark]
#[bench::run(header_map(32), affinity_pair())]
fn header_map_populated_32(mut m: HeaderMap, sd: (Affinity, Affinity)) -> HeaderMap {
    let (src, dst) = sd;
    for _ in 0..N {
        black_box(&mut m).relocate(black_box(Some(src)), black_box(dst));
    }
    m
}

// ===== request — must be O(1) in header count =====

#[library_benchmark]
#[bench::run(request((), 0), affinity_pair())]
fn request_unit_body_empty_headers(mut req: Request<()>, sd: (Affinity, Affinity)) -> Request<()> {
    let (src, dst) = sd;
    for _ in 0..N {
        black_box(&mut req).relocate(black_box(Some(src)), black_box(dst));
    }
    req
}

#[library_benchmark]
#[bench::run(request((), 32), affinity_pair())]
fn request_unit_body_populated_headers(mut req: Request<()>, sd: (Affinity, Affinity)) -> Request<()> {
    let (src, dst) = sd;
    for _ in 0..N {
        black_box(&mut req).relocate(black_box(Some(src)), black_box(dst));
    }
    req
}

#[library_benchmark]
#[bench::run(request(Bytes::from(vec![0_u8; 4096]), 32), affinity_pair())]
fn request_bytes_body(mut req: Request<Bytes>, sd: (Affinity, Affinity)) -> Request<Bytes> {
    let (src, dst) = sd;
    for _ in 0..N {
        black_box(&mut req).relocate(black_box(Some(src)), black_box(dst));
    }
    req
}

#[library_benchmark]
#[bench::run(request(CounterBody::default(), 32), affinity_pair())]
fn request_counter_body(mut req: Request<CounterBody>, sd: (Affinity, Affinity)) -> Request<CounterBody> {
    let (src, dst) = sd;
    for _ in 0..N {
        black_box(&mut req).relocate(black_box(Some(src)), black_box(dst));
    }
    req
}

// ===== response — same shape as request =====

#[library_benchmark]
#[bench::run(response((), 0), affinity_pair())]
fn response_unit_body_empty_headers(mut resp: Response<()>, sd: (Affinity, Affinity)) -> Response<()> {
    let (src, dst) = sd;
    for _ in 0..N {
        black_box(&mut resp).relocate(black_box(Some(src)), black_box(dst));
    }
    resp
}

#[library_benchmark]
#[bench::run(response((), 32), affinity_pair())]
fn response_unit_body_populated_headers(mut resp: Response<()>, sd: (Affinity, Affinity)) -> Response<()> {
    let (src, dst) = sd;
    for _ in 0..N {
        black_box(&mut resp).relocate(black_box(Some(src)), black_box(dst));
    }
    resp
}

#[library_benchmark]
#[bench::run(response(Bytes::from(vec![0_u8; 4096]), 32), affinity_pair())]
fn response_bytes_body(mut resp: Response<Bytes>, sd: (Affinity, Affinity)) -> Response<Bytes> {
    let (src, dst) = sd;
    for _ in 0..N {
        black_box(&mut resp).relocate(black_box(Some(src)), black_box(dst));
    }
    resp
}

#[library_benchmark]
#[bench::run(response(CounterBody::default(), 32), affinity_pair())]
fn response_counter_body(mut resp: Response<CounterBody>, sd: (Affinity, Affinity)) -> Response<CounterBody> {
    let (src, dst) = sd;
    for _ in 0..N {
        black_box(&mut resp).relocate(black_box(Some(src)), black_box(dst));
    }
    resp
}

library_benchmark_group!(
    name = third_party_group;
    benchmarks =
        noop_value_status_code, noop_value_method, noop_value_version,
        noop_value_header_name, noop_value_header_value,
        noop_value_bytes_empty, noop_value_bytes_4kb, noop_value_bytes_mut_4kb,
        noop_value_uuid,
        noop_value_timestamp, noop_value_signed_duration, noop_value_span,
        noop_value_date, noop_value_time, noop_value_datetime, noop_value_iso_week_date,
        header_map_empty, header_map_populated_8, header_map_populated_32,
        request_unit_body_empty_headers, request_unit_body_populated_headers,
        request_bytes_body, request_counter_body,
        response_unit_body_empty_headers, response_unit_body_populated_headers,
        response_bytes_body, response_counter_body
);
