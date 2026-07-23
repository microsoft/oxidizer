// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(
    clippy::unnecessary_box_returns,
    reason = "boxing setup state keeps large cache moves outside the measured region"
)]

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod linux {
    use gungraun::{library_benchmark, library_benchmark_group};

    include!("common/header_cache_scenarios.rs");

    #[library_benchmark]
    #[bench::run(Box::new(date_headers()))]
    fn date_uncached_bench(headers: Box<HeaderMap>) -> Box<HeaderMap> {
        let _ = date_uncached(&headers);
        headers
    }

    #[library_benchmark]
    #[bench::run(Box::new(warm_date_cache()))]
    fn date_cached_bench(mut state: Box<(HeaderCache, HeaderMap)>) -> Box<(HeaderCache, HeaderMap)> {
        let (cache, headers) = &mut *state;
        let _ = date_cached(cache, headers);
        state
    }

    #[library_benchmark]
    #[bench::run(Box::new(accept_encoding_headers()))]
    fn accept_encoding_uncached_bench(headers: Box<HeaderMap>) -> Box<HeaderMap> {
        let _ = accept_encoding_uncached(&headers);
        headers
    }

    #[library_benchmark]
    #[bench::run(Box::new(warm_accept_encoding_cache()))]
    fn accept_encoding_cached_bench(mut state: Box<(HeaderCache, HeaderMap)>) -> Box<(HeaderCache, HeaderMap)> {
        let (cache, headers) = &mut *state;
        let _ = accept_encoding_cached(cache, headers);
        state
    }

    library_benchmark_group!(
        name = header_cache;
        benchmarks = date_uncached_bench,
        date_cached_bench,
        accept_encoding_uncached_bench,
        accept_encoding_cached_bench
    );
}

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = header_cache);
