// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measure `#[event]` parsing and code generation, without preparing its input tokens.

use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

const EXPANSION: &str = "observed_event/expansion";
const CASES: [(&str, Case); 7] = [
    ("unit", Case::Unit),
    ("log_2_fields", Case::Log(2)),
    ("log_16_fields", Case::Log(16)),
    ("log_64_fields", Case::Log(64)),
    ("metrics_only", Case::MetricsOnly),
    ("mixed_optional", Case::MixedOptional),
    ("generic", Case::Generic),
];

#[derive(Clone, Copy)]
enum Case {
    Unit,
    Log(usize),
    MetricsOnly,
    MixedOptional,
    Generic,
}

fn input(case: Case) -> (TokenStream, TokenStream) {
    let item = match case {
        Case::Unit => quote! {
            #[counter]
            struct Heartbeat;
        },
        Case::Log(count) => log_fields(count),
        Case::MetricsOnly => {
            let fields = fields(16);
            quote! {
                #[counter]
                struct Heartbeat { #fields }
            }
        }
        Case::MixedOptional => quote! {
            #[info("{method} completed with {status}")]
            #[counter(name = "http.requests")]
            #[histogram(duration_ms, name = "http.duration", unit = "ms")]
            struct Request<'a> {
                #[data_class(PUBLIC)]
                method: &'a str,
                #[unredacted]
                #[dimension(metric)]
                status: u16,
                #[unredacted]
                duration_ms: f64,
                #[data_class(PRIVATE)]
                #[if_none(drop)]
                user: Option<&'a str>,
                #[if_none("unknown")]
                region: Option<ClassifiedString>,
                #[unredacted]
                #[dimension(log = exclude, metric = "service.version")]
                version: &'static str,
            }
        },
        Case::Generic => quote! {
            #[info]
            struct GenericEvent<'a, T, U, const N: usize>
            where
                T: Clone,
            {
                #[unredacted]
                value: T,
                #[data_class(PRIVATE)]
                classified: &'a U,
                optional: Option<U>,
                #[dimension(log = exclude)]
                unused: [T; N],
            }
        },
    };
    (quote! { "bench.event" }, item)
}

fn fields(count: usize) -> TokenStream {
    (0..count)
        .map(|index| {
            let ident = format_ident!("field_{index}");
            quote! {
                #[unredacted]
                #ident: u64,
            }
        })
        .collect()
}

fn log_fields(count: usize) -> TokenStream {
    let fields = fields(count);
    quote! {
        #[info]
        struct LogEvent { #fields }
    }
}

#[metabench::benchmark(EXPAND, EXPANSION, "event")]
#[bench::unit(input(Case::Unit))]
#[bench::log_2_fields(input(Case::Log(2)))]
#[bench::log_16_fields(input(Case::Log(16)))]
#[bench::log_64_fields(input(Case::Log(64)))]
#[bench::metrics_only(input(Case::MetricsOnly))]
#[bench::mixed_optional(input(Case::MixedOptional))]
#[bench::generic(input(Case::Generic))]
fn expand_event((attr, item): (TokenStream, TokenStream)) -> TokenStream {
    observed_macros_impl::event(black_box(attr), black_box(item)).expect("benchmark inputs are valid event declarations")
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(EXPAND.group_name());
    for (name, case) in CASES {
        group.bench_function(BenchmarkId::new(EXPAND.benchmark_name(), name), |bencher| {
            // Token preparation and output destruction stay outside the measurement.
            bencher.iter_batched(|| input(case), expand_event, BatchSize::SmallInput);
        });
    }
    group.finish();
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [EXPAND]);
