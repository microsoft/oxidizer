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
    fn produce_common_routerama_reserved() {
        let query = direct_common_value();
        let mut output = String::with_capacity(64);
        direct_produce_common(&query, &mut output);
    }

    #[library_benchmark]
    fn produce_common_serde_html_form_reserved() {
        let query = serde_common_value();
        let mut output = String::with_capacity(64);
        serde_html_form_produce_common_reserved(&query, &mut output);
    }

    #[library_benchmark]
    fn produce_common_routerama_allocating() {
        direct_produce_common_allocating(&direct_common_value());
    }

    #[library_benchmark]
    fn produce_common_serde_urlencoded_allocating() {
        serde_urlencoded_produce_common(&serde_common_value());
    }

    #[library_benchmark]
    fn produce_common_serde_html_form_allocating() {
        serde_html_form_produce_common(&serde_common_value());
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
