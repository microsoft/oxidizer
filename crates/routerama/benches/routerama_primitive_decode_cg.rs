// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind baselines for percent-encoded primitive path/query decoding.
//!
//! Paired with `routerama_primitive_decode.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod linux {
    use gungraun::prelude::*;

    include!("common/primitive_decode_scenarios.rs");

    macro_rules! benchmark_case {
        ($name:ident, $source:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(prepare())]
            fn $name(fixtures: Fixtures) -> (Fixtures, Observation) {
                let observation = std::hint::black_box(run(&fixtures, Source::$source, Scenario::$scenario));
                (fixtures, observation)
            }
        };
    }

    macro_rules! source_group {
        ($group:ident, $source:ident, $( $name:ident => $scenario:ident ),+ $(,)?) => {
            $(benchmark_case!($name, $source, $scenario);)+
            library_benchmark_group!(
                name = $group;
                benchmarks = $( $name ),+
            );
        };
    }

    source_group!(
        path,
        Path,
        path_signed_success => SignedSuccess,
        path_signed_success_control => SignedSuccessControl,
        path_signed_zero => SignedZero,
        path_signed_plus => SignedPlus,
        path_signed_min => SignedMin,
        path_signed_max => SignedMax,
        path_signed_overflow => SignedOverflow,
        path_unsigned_success => UnsignedSuccess,
        path_unsigned_success_control => UnsignedSuccessControl,
        path_unsigned_zero => UnsignedZero,
        path_unsigned_plus => UnsignedPlus,
        path_unsigned_max => UnsignedMax,
        path_unsigned_overflow => UnsignedOverflow,
        path_bool_success => BoolSuccess,
        path_bool_success_control => BoolSuccessControl,
        path_bool_false => BoolFalse,
        path_bool_invalid => BoolInvalid,
        path_malformed_encoding => MalformedEncoding,
        path_invalid_utf8 => InvalidUtf8,
        path_generic_from_str => GenericFromStr,
        path_signed_unescaped => SignedUnescaped,
        path_signed_unescaped_control => SignedUnescapedControl,
        path_unsigned_unescaped => UnsignedUnescaped,
        path_unsigned_unescaped_control => UnsignedUnescapedControl,
        path_bool_unescaped => BoolUnescaped,
        path_bool_unescaped_control => BoolUnescapedControl,
        path_generic_unescaped => GenericUnescaped,
    );
    source_group!(
        dynamic_path,
        DynamicPath,
        dynamic_path_signed_success => SignedSuccess,
        dynamic_path_signed_success_control => SignedSuccessControl,
        dynamic_path_signed_zero => SignedZero,
        dynamic_path_signed_plus => SignedPlus,
        dynamic_path_signed_min => SignedMin,
        dynamic_path_signed_max => SignedMax,
        dynamic_path_signed_overflow => SignedOverflow,
        dynamic_path_unsigned_success => UnsignedSuccess,
        dynamic_path_unsigned_success_control => UnsignedSuccessControl,
        dynamic_path_unsigned_zero => UnsignedZero,
        dynamic_path_unsigned_plus => UnsignedPlus,
        dynamic_path_unsigned_max => UnsignedMax,
        dynamic_path_unsigned_overflow => UnsignedOverflow,
        dynamic_path_bool_success => BoolSuccess,
        dynamic_path_bool_success_control => BoolSuccessControl,
        dynamic_path_bool_false => BoolFalse,
        dynamic_path_bool_invalid => BoolInvalid,
        dynamic_path_malformed_encoding => MalformedEncoding,
        dynamic_path_invalid_utf8 => InvalidUtf8,
        dynamic_path_generic_from_str => GenericFromStr,
        dynamic_path_signed_unescaped => SignedUnescaped,
        dynamic_path_signed_unescaped_control => SignedUnescapedControl,
        dynamic_path_unsigned_unescaped => UnsignedUnescaped,
        dynamic_path_unsigned_unescaped_control => UnsignedUnescapedControl,
        dynamic_path_bool_unescaped => BoolUnescaped,
        dynamic_path_bool_unescaped_control => BoolUnescapedControl,
        dynamic_path_generic_unescaped => GenericUnescaped,
    );
    source_group!(
        query,
        Query,
        query_signed_success => SignedSuccess,
        query_signed_success_control => SignedSuccessControl,
        query_signed_zero => SignedZero,
        query_signed_plus => SignedPlus,
        query_signed_min => SignedMin,
        query_signed_max => SignedMax,
        query_signed_overflow => SignedOverflow,
        query_unsigned_success => UnsignedSuccess,
        query_unsigned_success_control => UnsignedSuccessControl,
        query_unsigned_zero => UnsignedZero,
        query_unsigned_plus => UnsignedPlus,
        query_unsigned_max => UnsignedMax,
        query_unsigned_overflow => UnsignedOverflow,
        query_bool_success => BoolSuccess,
        query_bool_success_control => BoolSuccessControl,
        query_bool_false => BoolFalse,
        query_bool_invalid => BoolInvalid,
        query_malformed_encoding => MalformedEncoding,
        query_invalid_utf8 => InvalidUtf8,
        query_generic_from_str => GenericFromStr,
        query_signed_unescaped => SignedUnescaped,
        query_signed_unescaped_control => SignedUnescapedControl,
        query_unsigned_unescaped => UnsignedUnescaped,
        query_unsigned_unescaped_control => UnsignedUnescapedControl,
        query_bool_unescaped => BoolUnescaped,
        query_bool_unescaped_control => BoolUnescapedControl,
        query_generic_unescaped => GenericUnescaped,
    );
}

#[cfg(target_os = "linux")]
pub use linux::{dynamic_path, path, query};

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = path, dynamic_path, query);
