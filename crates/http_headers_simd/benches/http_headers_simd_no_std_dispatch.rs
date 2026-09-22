// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Scanner crossover benchmarks for the `no_std` dispatch path.
//!
//! Run with:
//! `cargo bench -p http_headers_simd --no-default-features --bench http_headers_simd_no_std_dispatch`.

use std::hint::black_box;

use criterion::Criterion;

const NO_STD_DISPATCH: &str = "http_headers_simd_no_std_dispatch/dispatch";

macro_rules! unary_case {
    ($identity:ident, $name:ident, $benchmark_name:literal, $length:expr, $scanner:path, $output:ty) => {
        #[metabench::benchmark($identity, NO_STD_DISPATCH, $benchmark_name)]
        fn $name() -> $output {
            let bytes = black_box([b'a'; $length]);
            black_box($scanner(&bytes))
        }
    };
}

macro_rules! equality_case {
    ($identity:ident, $name:ident, $benchmark_name:literal, $length:expr) => {
        #[metabench::benchmark($identity, NO_STD_DISPATCH, $benchmark_name)]
        fn $name() -> bool {
            let left = black_box([b'a'; $length]);
            let right = black_box([b'A'; $length]);
            black_box(http_headers_simd::eq_ignore_ascii_case(&left, &right))
        }
    };
}

macro_rules! scanner_cases {
    (
        $macro_name:ident,
        $scanner:path,
        $output:ty,
        [
            $(($identity:ident, $name:ident, $benchmark_name:literal, $length:expr)),+ $(,)?
        ]
    ) => {
        $(
            $macro_name!($identity, $name, $benchmark_name, $length, $scanner, $output);
        )+
    };
}

scanner_cases!(
    unary_case,
    http_headers_simd::is_token,
    bool,
    [
        (TOKEN_15, token_15, "token_15", 15),
        (TOKEN_16, token_16, "token_16", 16),
        (TOKEN_17, token_17, "token_17", 17),
        (TOKEN_31, token_31, "token_31", 31),
        (TOKEN_32, token_32, "token_32", 32),
        (TOKEN_33, token_33, "token_33", 33),
        (TOKEN_47, token_47, "token_47", 47),
        (TOKEN_48, token_48, "token_48", 48),
    ]
);
scanner_cases!(
    unary_case,
    http_headers_simd::is_token68,
    bool,
    [
        (TOKEN68_15, token68_15, "token68_15", 15),
        (TOKEN68_16, token68_16, "token68_16", 16),
        (TOKEN68_17, token68_17, "token68_17", 17),
        (TOKEN68_31, token68_31, "token68_31", 31),
        (TOKEN68_32, token68_32, "token68_32", 32),
        (TOKEN68_33, token68_33, "token68_33", 33),
        (TOKEN68_47, token68_47, "token68_47", 47),
        (TOKEN68_48, token68_48, "token68_48", 48),
    ]
);
scanner_cases!(
    unary_case,
    http_headers_simd::is_field_value,
    bool,
    [
        (FIELD_VALUE_15, field_value_15, "field_value_15", 15),
        (FIELD_VALUE_16, field_value_16, "field_value_16", 16),
        (FIELD_VALUE_17, field_value_17, "field_value_17", 17),
        (FIELD_VALUE_31, field_value_31, "field_value_31", 31),
        (FIELD_VALUE_32, field_value_32, "field_value_32", 32),
        (FIELD_VALUE_33, field_value_33, "field_value_33", 33),
        (FIELD_VALUE_47, field_value_47, "field_value_47", 47),
        (FIELD_VALUE_48, field_value_48, "field_value_48", 48),
    ]
);
scanner_cases!(
    unary_case,
    http_headers_simd::find_interesting,
    Option<usize>,
    [
        (INTERESTING_15, interesting_15, "interesting_15", 15),
        (INTERESTING_16, interesting_16, "interesting_16", 16),
        (INTERESTING_17, interesting_17, "interesting_17", 17),
        (INTERESTING_31, interesting_31, "interesting_31", 31),
        (INTERESTING_32, interesting_32, "interesting_32", 32),
        (INTERESTING_33, interesting_33, "interesting_33", 33),
        (INTERESTING_47, interesting_47, "interesting_47", 47),
        (INTERESTING_48, interesting_48, "interesting_48", 48),
    ]
);

equality_case!(EQUALITY_15, equality_15, "equality_15", 15);
equality_case!(EQUALITY_16, equality_16, "equality_16", 16);
equality_case!(EQUALITY_17, equality_17, "equality_17", 17);
equality_case!(EQUALITY_31, equality_31, "equality_31", 31);
equality_case!(EQUALITY_32, equality_32, "equality_32", 32);
equality_case!(EQUALITY_33, equality_33, "equality_33", 33);
equality_case!(EQUALITY_47, equality_47, "equality_47", 47);
equality_case!(EQUALITY_48, equality_48, "equality_48", 48);

#[metabench::benchmark(DISPATCH_512, NO_STD_DISPATCH, "dispatch_512")]
fn dispatch_512() -> usize {
    let left = black_box([b'a'; 512]);
    let right = black_box([b'A'; 512]);
    usize::from(http_headers_simd::is_token(&left))
        + usize::from(http_headers_simd::is_token68(&left))
        + usize::from(http_headers_simd::is_field_value(&left))
        + usize::from(http_headers_simd::eq_ignore_ascii_case(&left, &right))
        + http_headers_simd::find_interesting(&left).unwrap_or(left.len())
}

macro_rules! criterion_cases {
    ($group:ident, [$(($identity:ident, $name:ident)),+ $(,)?]) => {
        $(
            $group.bench_function($identity.benchmark_name(), |bencher| {
                bencher.iter($name);
            });
        )+
    };
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut dispatch = criterion.benchmark_group(NO_STD_DISPATCH);
    criterion_cases!(
        dispatch,
        [
            (TOKEN_15, token_15),
            (TOKEN_16, token_16),
            (TOKEN_17, token_17),
            (TOKEN_31, token_31),
            (TOKEN_32, token_32),
            (TOKEN_33, token_33),
            (TOKEN_47, token_47),
            (TOKEN_48, token_48),
            (TOKEN68_15, token68_15),
            (TOKEN68_16, token68_16),
            (TOKEN68_17, token68_17),
            (TOKEN68_31, token68_31),
            (TOKEN68_32, token68_32),
            (TOKEN68_33, token68_33),
            (TOKEN68_47, token68_47),
            (TOKEN68_48, token68_48),
            (FIELD_VALUE_15, field_value_15),
            (FIELD_VALUE_16, field_value_16),
            (FIELD_VALUE_17, field_value_17),
            (FIELD_VALUE_31, field_value_31),
            (FIELD_VALUE_32, field_value_32),
            (FIELD_VALUE_33, field_value_33),
            (FIELD_VALUE_47, field_value_47),
            (FIELD_VALUE_48, field_value_48),
            (EQUALITY_15, equality_15),
            (EQUALITY_16, equality_16),
            (EQUALITY_17, equality_17),
            (EQUALITY_31, equality_31),
            (EQUALITY_32, equality_32),
            (EQUALITY_33, equality_33),
            (EQUALITY_47, equality_47),
            (EQUALITY_48, equality_48),
            (INTERESTING_15, interesting_15),
            (INTERESTING_16, interesting_16),
            (INTERESTING_17, interesting_17),
            (INTERESTING_31, interesting_31),
            (INTERESTING_32, interesting_32),
            (INTERESTING_33, interesting_33),
            (INTERESTING_47, interesting_47),
            (INTERESTING_48, interesting_48),
            (DISPATCH_512, dispatch_512),
        ]
    );
    dispatch.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        TOKEN_15,
        TOKEN_16,
        TOKEN_17,
        TOKEN_31,
        TOKEN_32,
        TOKEN_33,
        TOKEN_47,
        TOKEN_48,
        TOKEN68_15,
        TOKEN68_16,
        TOKEN68_17,
        TOKEN68_31,
        TOKEN68_32,
        TOKEN68_33,
        TOKEN68_47,
        TOKEN68_48,
        FIELD_VALUE_15,
        FIELD_VALUE_16,
        FIELD_VALUE_17,
        FIELD_VALUE_31,
        FIELD_VALUE_32,
        FIELD_VALUE_33,
        FIELD_VALUE_47,
        FIELD_VALUE_48,
        EQUALITY_15,
        EQUALITY_16,
        EQUALITY_17,
        EQUALITY_31,
        EQUALITY_32,
        EQUALITY_33,
        EQUALITY_47,
        EQUALITY_48,
        INTERESTING_15,
        INTERESTING_16,
        INTERESTING_17,
        INTERESTING_31,
        INTERESTING_32,
        INTERESTING_33,
        INTERESTING_47,
        INTERESTING_48,
        DISPATCH_512,
    ],
);
