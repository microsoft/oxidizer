// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod linux {
    use gungraun::{library_benchmark, library_benchmark_group};

    include!("common/query_scenarios.rs");

    #[library_benchmark]
    fn parse_common_routerama() {
        direct_parse_common();
    }

    #[library_benchmark]
    fn parse_common_serde_urlencoded() {
        serde_urlencoded_parse_common();
    }

    #[library_benchmark]
    fn parse_common_serde_html_form() {
        serde_html_form_parse_common();
    }

    #[library_benchmark]
    fn parse_escaped_routerama() {
        direct_parse_escaped();
    }

    #[library_benchmark]
    fn parse_escaped_serde_urlencoded() {
        serde_urlencoded_parse_escaped();
    }

    #[library_benchmark]
    fn parse_escaped_serde_html_form() {
        serde_html_form_parse_escaped();
    }

    #[library_benchmark]
    fn parse_repeated_routerama() {
        direct_parse_repeated();
    }

    #[library_benchmark]
    fn parse_repeated_serde_html_form() {
        serde_html_form_parse_repeated();
    }

    #[library_benchmark]
    fn parse_long_routerama() {
        direct_parse_long();
    }

    #[library_benchmark]
    fn parse_long_serde_urlencoded() {
        serde_urlencoded_parse_long();
    }

    #[library_benchmark]
    fn parse_long_serde_html_form() {
        serde_html_form_parse_long();
    }

    #[library_benchmark]
    #[bench::run(direct_common_value(), String::with_capacity(64))]
    fn produce_common_routerama_reserved(query: DirectCommon<'static>, mut output: String) -> (DirectCommon<'static>, String) {
        direct_produce_common(&query, &mut output);
        (query, output)
    }

    #[library_benchmark]
    #[bench::run(serde_common_value(), String::with_capacity(64))]
    fn produce_common_serde_html_form_reserved(query: SerdeCommon<'static>, mut output: String) -> (SerdeCommon<'static>, String) {
        serde_html_form_produce_common_reserved(&query, &mut output);
        (query, output)
    }

    #[library_benchmark]
    #[bench::run(direct_common_value())]
    fn produce_common_routerama_allocating(query: DirectCommon<'static>) -> DirectCommon<'static> {
        direct_produce_common_allocating(&query);
        query
    }

    #[library_benchmark]
    #[bench::run(serde_common_value())]
    fn produce_common_serde_urlencoded_allocating(query: SerdeCommon<'static>) -> SerdeCommon<'static> {
        serde_urlencoded_produce_common(&query);
        query
    }

    #[library_benchmark]
    #[bench::run(serde_common_value())]
    fn produce_common_serde_html_form_allocating(query: SerdeCommon<'static>) -> SerdeCommon<'static> {
        serde_html_form_produce_common(&query);
        query
    }

    library_benchmark_group!(
        name = query_codecs;
        benchmarks =
            parse_common_routerama,
            parse_common_serde_urlencoded,
            parse_common_serde_html_form,
            parse_escaped_routerama,
            parse_escaped_serde_urlencoded,
            parse_escaped_serde_html_form,
            parse_repeated_routerama,
            parse_repeated_serde_html_form,
            parse_long_routerama,
            parse_long_serde_urlencoded,
            parse_long_serde_html_form,
            produce_common_routerama_reserved,
            produce_common_serde_html_form_reserved,
            produce_common_routerama_allocating,
            produce_common_serde_urlencoded_allocating,
            produce_common_serde_html_form_allocating
    );
}

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = query_codecs);
