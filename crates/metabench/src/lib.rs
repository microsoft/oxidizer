// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![forbid(unsafe_code)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/metabench/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/metabench/favicon.ico")]

//!
//! A unified Rust benchmark harness for running one workload through Criterion,
//! Gungraun Callgrind, Linux `perf`, and allocation tracking.
//!
//! ## Benchmark definition
//!
//! Add one harness-free benchmark target:
//!
//! ```toml
//! [[bench]]
//! name = "operations"
//! harness = false
//! ```
//!
//! Define a group type and put benchmark methods in an attributed inherent impl:
//!
//! ```ignore
//! struct MathBenchmarks;
//!
//! #[metabench::benchmarks]
//! impl MathBenchmarks {
//!     #[metabench::benchmark]
//!     fn fibonacci_10k() -> u64 {
//!         (0..10_000_u64).fold(0, |accumulator, value| accumulator ^ value)
//!     }
//! }
//!
//! metabench::main!(groups = [MathBenchmarks], allocator = std::alloc::System,);
//! ```
//!
//! The type name supplies the group name (`MathBenchmarks` becomes `math`).
//! Methods marked with `#[metabench::benchmark]` are benchmarks, and their
//! identifiers supply benchmark names. Stateless methods take no receiver or one
//! shared case reference; stateful methods take `&self` or `&mut self`.
//!
//! The allocator can be replaced with another unit-like `GlobalAlloc`
//! implementation, such as mimalloc. Metabench wraps it with
//! `alloc_tracker::Allocator`, so every mode uses the same allocator.
//!
//! The benchmark targets progress from the least to the most benchmark machinery:
//!
//! | Benchmark target | State | Data cases | Run |
//! |---|---|---|---|
//! | `stateless_no_cases` | No | No | `cargo bench -p metabench --bench stateless_no_cases` |
//! | `stateful_no_cases` | `SimpleFixture` | No | `cargo bench -p metabench --bench stateful_no_cases` |
//! | `stateless_cases` | No | `BenchmarkCases` | `cargo bench -p metabench --bench stateless_cases` |
//! | `stateful_cases` | `Fixture` | Yes | `cargo bench -p metabench --bench stateful_cases` |
//!
//! One executable may list group types from any number of source modules:
//!
//! ```ignore
//! mod hashmap;
//! mod parsing;
//!
//! metabench::main!(
//!     groups = [hashmap::HashMapBenchmarks, parsing::ParsingBenchmarks,],
//!     allocator = std::alloc::System,
//! );
//! ```
//!
//! ## Fixture setup, cases, and cleanup
//!
//! Implement `BenchmarkCases` when stateless methods need parameters:
//!
//! ```rust
//! use metabench::{BenchmarkCase, BenchmarkCases};
//!
//! #[derive(Clone, Copy)]
//! struct SearchCase(&'static str);
//!
//! impl BenchmarkCase for SearchCase {
//!     fn name(&self) -> String {
//!         self.0.to_owned()
//!     }
//! }
//!
//! struct SearchBenchmarks;
//!
//! impl BenchmarkCases for SearchBenchmarks {
//!     type Case = SearchCase;
//!
//!     fn cases() -> impl IntoIterator<Item = Self::Case> {
//!         [SearchCase("short"), SearchCase("a longer input")]
//!     }
//! }
//!
//! #[metabench::benchmarks]
//! impl SearchBenchmarks {
//!     #[metabench::benchmark]
//!     fn count_bytes(case: &SearchCase) -> usize {
//!         case.0.len()
//!     }
//! }
//! ```
//!
//! The case reference is shared so one parameter value can be reused safely
//! across repeated measurements.
//!
//! Implement `SimpleFixture` for fresh single-case state or `Fixture` for
//! data-driven state. Each fixture is created outside measurement, passed to one
//! benchmark method invocation, and dropped outside measurement:
//!
//! ```rust
//! use std::collections::HashMap;
//!
//! use metabench::{BenchmarkCase, Fixture};
//!
//! #[derive(Clone, Copy)]
//! struct HashMapCase {
//!     capacity: usize,
//! }
//!
//! impl BenchmarkCase for HashMapCase {
//!     fn name(&self) -> String {
//!         format!("capacity={}", self.capacity)
//!     }
//! }
//!
//! struct HashMapBenchmarks {
//!     map: HashMap<String, String>,
//! }
//!
//! impl Fixture for HashMapBenchmarks {
//!     type Case = HashMapCase;
//!
//!     fn cases() -> impl IntoIterator<Item = Self::Case> {
//!         [HashMapCase { capacity: 0 }, HashMapCase { capacity: 1_000 }]
//!     }
//!
//!     fn setup(case: &Self::Case) -> Self {
//!         Self {
//!             map: HashMap::with_capacity(case.capacity),
//!         }
//!     }
//! }
//!
//! impl Drop for HashMapBenchmarks {
//!     fn drop(&mut self) {
//!         self.map.clear();
//!     }
//! }
//!
//! #[metabench::benchmarks]
//! impl HashMapBenchmarks {
//!     #[metabench::benchmark]
//!     fn insert_string(&mut self) {
//!         self.map.insert(String::from("key"), String::from("value"));
//!     }
//! }
//! ```
//!
//! This produces `hash_map/insert_string/capacity=0` and
//! `hash_map/insert_string/capacity=1000`. Method outputs are black-boxed and
//! dropped before the fixture is dropped, with both destructions outside the
//! measurement boundary.
//!
//! With no `engines` argument, `#[metabench::benchmarks]` uses
//! `Engines::DEFAULT`: Criterion, Gungraun, and allocation tracking. Native
//! `perf` is excluded because it commonly requires additional host permissions.
//! Available flags are `Engines::CRITERION`, `Engines::GUNGRAUN`,
//! `Engines::PERF`, `Engines::ALLOCATIONS`, `Engines::DEFAULT`, and
//! `Engines::ALL`.
//!
//! Override a benchmark method's displayed name or engines with arguments to
//! `#[metabench::benchmark(...)]`:
//!
//! ```rust
//! # struct MathBenchmarks;
//! #[metabench::benchmarks]
//! impl MathBenchmarks {
//!     #[metabench::benchmark(
//!         name = "fast-path",
//!         engines = metabench::Engines::CRITERION | metabench::Engines::ALLOCATIONS,
//!     )]
//!     fn add() -> u64 {
//!         1 + 1
//!     }
//! }
//! ```
//!
//! ## Engines
//!
//! With no engine filter, metabench automatically runs the union of engines
//! selected by the registered benchmarks:
//!
//! ```text
//! cargo bench --bench operations
//! ```
//!
//! Metabench always places Criterion workers in benchmark mode, including when a
//! benchmark executable is launched through `cargo run`; callers do not need to
//! forward Criterion's internal `--bench` marker.
//!
//! Command-line selectors are optional filters. They never enable an engine that
//! the benchmark excluded:
//!
//! ```text
//! cargo bench --bench operations -- --criterion
//! cargo bench --bench operations -- --criterion --callgrind
//! cargo bench --bench operations -- --all-engines
//! ```
//!
//! Use `--filter` to select complete `group/benchmark/case` identities consistently
//! across every engine:
//!
//! ```text
//! cargo bench -p metabench --bench stateful_cases -- \
//!   --filter 'hash_map/insert_string/*'
//! ```
//!
//! `*` matches any sequence and `?` matches one character. The option is
//! repeatable; a benchmark runs when any supplied pattern matches. A filter that
//! matches no registered benchmark is an error.
//!
//! Use `--list` with the same filters and engine selectors to inspect what would
//! run without launching an engine:
//!
//! ```text
//! cargo bench -p metabench --bench stateful_cases -- \
//!   --list --filter 'hash_map/*'
//! ```
//!
//! Selecting an engine unused by every registered benchmark is an error.
//! Callgrind requires Valgrind and `gungraun-runner` 0.19.4 on `PATH`; metabench
//! pins the matching Gungraun library release. Gungraun uses internal `case_0`,
//! `case_1`, and similar labels, while metabench records a stable identity
//! manifest so child selection and artifact attribution do not depend on
//! reconstructed registration order.
//!
//! The native `perf` mode requires Linux, the `perf` executable, and permission to
//! access user-space PMU events. It starts with counters disabled and uses
//! synchronized control FIFOs to enable them only around the workload call.
//! Allocation mode records process-wide allocated bytes and allocation counts,
//! including allocations on workload-created threads. It invokes each workload
//! once and reports the exact integer byte and allocation-operation totals; setup
//! and cleanup remain outside the tracking span.
//!
//! Metabench first runs as an orchestrator and relaunches the same benchmark
//! executable as a worker. This gives Criterion a sanitized argument list and lets
//! profiling tools launch the exact binary containing the canonical workload
//! symbol.
//!
//! Raw engine output is hidden by default. Metabench prints
//! `Benchmarking <benchmark> under <engine>...` for each workload and then emits one
//! aligned table containing every normalized metric. Use `--show-engine-output`
//! when diagnosing an engine or forwarding options whose output needs inspection.
//!
//! `--timeout` applies a positive duration such as `30s`, `5m`, or `1h` to each
//! engine worker. Metabench continues through independent engine failures by
//! default; `--fail-fast` stops at the first failure and `--keep-going` states the
//! default explicitly.
//!
//! ## Reports and baselines
//!
//! Every successful run writes both unified reports by default:
//!
//! ```text
//! target/metabench/<benchmark-target>/report.json
//! target/metabench/<benchmark-target>/report.md
//! ```
//!
//! After the console Results table, metabench prints the paths of both generated
//! report files.
//!
//! For example, the `stateful_cases` target writes under
//! `target/metabench/stateful_cases/`. Use `--no-output` for a console-only run.
//! Override either format independently when needed:
//!
//! ```text
//! cargo bench --bench operations -- --criterion \
//!   --export-json target/metabench.json \
//!   --export-md target/metabench.md
//!
//! cargo bench --bench operations -- --all-engines \
//!   --export-json target/metabench.json \
//!   --export-md target/metabench.md
//! ```
//!
//! `--output target/results` overrides both paths with
//! `target/results.json` and `target/results.md`. It is shorthand for exporting both
//! files and cannot be combined with
//! the individual export-path options.
//!
//! When the resolved JSON output already exists, metabench reads it before
//! overwriting it and automatically uses it as the baseline. Consequently, each
//! successful run compares against the preceding successful report for that
//! benchmark target. The first run remains uncompared.
//!
//! Use `--baseline` to override the automatic baseline with another metabench JSON
//! report, or `--no-baseline` to ignore an existing report and leave the new
//! results uncompared. These options cannot be combined. The comparison threshold
//! defaults to 5 percent:
//!
//! ```text
//! cargo bench --bench operations -- --all-engines \
//!   --baseline previous.json \
//!   --regression-threshold 3 \
//!   --export-json current.json \
//!   --export-md current.md
//! ```
//!
//! Backend options are scoped and repeatable, so options for one engine are never
//! sent to another:
//!
//! ```text
//! cargo bench --bench operations -- \
//!   --criterion \
//!   --callgrind \
//!   --criterion-arg=--sample-size \
//!   --criterion-arg=50 \
//!   --callgrind-arg=--save-summary=json
//! ```
//!
//! The available routing flags are `--criterion-arg`, `--callgrind-arg`, and
//! `--perf-arg`. Both `--*-arg VALUE` and `--*-arg=VALUE` forms are accepted.
//! Arguments after a standalone `--` remain a convenience for a single selected
//! engine and are rejected as ambiguous when several engines are selected.
//!
//! A workload is classified as regressed if any comparable lower-is-better metric
//! increases by at least the threshold. This classification drives causal
//! regression analysis but is not displayed as a Status column. Instead, the
//! console and Markdown Results tables place a neutral `Benchmark (PRIOR)` row
//! immediately after each benchmark that has a matching baseline entry.
//! At a zero threshold, exactly unchanged values remain stable; a zero baseline
//! that becomes positive is represented as a finite 100 percent regression.
//!
//! JSON reports include `schema_version`, environment metadata, and one entry per
//! registered workload. Schema version 5 stores group, benchmark, and optional
//! case names separately, serializes durations as integer nanoseconds, stores
//! allocation measurements as exact integer totals, and retains one non-recursive
//! snapshot of matching prior measurements.
//! Unsupported schema versions and duplicate baseline identities are rejected
//! before comparison. JSON and Markdown are staged together and an unsuccessful
//! publication restores the preceding successful pair.
//! Markdown reports contain an environment table, one Results table with every
//! collected metric, and causal notes for regressions.
//!
//! Backend artifacts are always collected to produce the console table. Criterion
//! artifacts are isolated through `CRITERION_HOME`, Gungraun 0.19.4 schema-v6
//! summaries are parsed with `gungraun-summary` 6.0.0, `perf` emits JSON Lines,
//! and allocation statistics come from `alloc_tracker`'s public operation
//! statistics. Unless `--no-output` is present, the normalized report is written
//! to the target-specific default paths or their explicit overrides.

mod arguments;
mod artifact;
mod bencher;
mod engines;
mod error;
mod fixture;
mod group;
mod mode;
mod report;
mod runner;

pub(crate) use bencher::Bencher;
pub use engines::Engines;
pub(crate) use error::Error;
pub use fixture::{BenchmarkCase, BenchmarkCases, Fixture, SimpleFixture};
pub(crate) use group::BenchmarkSuite;
pub use metabench_macros::{benchmark, benchmarks};
pub(crate) use mode::Mode;

/// Dependencies used by macro expansions.
#[doc(hidden)]
pub mod __private {
    pub use alloc_tracker::Allocator;
    pub use gungraun;

    pub use crate::bencher::{Bencher, CleanupBencher, SetupBencher};
    pub use crate::error::Error;
    pub use crate::fixture::{BenchmarkGroupDefinition, PreparedOutput};
    pub use crate::group::{BenchmarkGroup, BenchmarkSuite, GungraunCase, gungraun_registry_case, gungraun_registry_len};
    pub use crate::runner::run_with_gungraun;
}

/// Defines the entry point and allocation tracker for a benchmark executable.
///
/// The allocator expression must also be usable as its type, as is the case for
/// unit-like global allocators such as [`std::alloc::System`].
///
/// See the [crate-level benchmark definition example](crate#benchmark-definition)
/// for a complete target using this macro.
#[macro_export]
macro_rules! main {
    (
        groups = [$($group:ty),+ $(,)?],
        allocator = $allocator:path $(,)?
    ) => {
        #[global_allocator]
        static METABENCH_ALLOCATOR: $crate::__private::Allocator<$allocator> =
            $crate::__private::Allocator::new($allocator);

        fn __metabench_register(suite: &mut $crate::__private::BenchmarkSuite) {
            $(
                <$group as $crate::__private::BenchmarkGroupDefinition>::register(
                    suite,
                );
            )+
        }

        mod __metabench_gungraun {
            use $crate::__private::GungraunCase;
            use $crate::__private::gungraun;

            fn registry_indices() -> std::ops::Range<usize> {
                let count = $crate::__private::gungraun_registry_len(
                    super::__metabench_register,
                )
                .unwrap_or_else(|error| {
                    panic!("invalid metabench registry: {error}")
                });
                0..count
            }

            fn select_case(index: usize) -> GungraunCase {
                $crate::__private::gungraun_registry_case(
                    super::__metabench_register,
                    index,
                )
                .unwrap_or_else(|error| {
                    panic!("invalid metabench case {index}: {error}")
                })
            }

            fn callgrind_config() -> gungraun::LibraryBenchmarkConfig {
                let mut callgrind = gungraun::Callgrind::default();
                callgrind.entry_point(gungraun::EntryPoint::Custom(
                    "*metabench::bencher::invoke_*".to_owned(),
                ));
                let mut config = gungraun::LibraryBenchmarkConfig::default();
                config.env_clear(false);
                config.tool(callgrind);
                config
            }

            #[gungraun::library_benchmark(setup = select_case)]
            #[benches::case(iter = registry_indices())]
            fn metabench_adapter(case: GungraunCase) {
                case.run().unwrap_or_else(|error| {
                    panic!("metabench workload failed: {error}")
                });
            }

            gungraun::library_benchmark_group!(
                name = metabench_group;
                benchmarks = metabench_adapter
            );

            gungraun::main!(
                config = callgrind_config();
                library_benchmark_groups = metabench_group
            );

            pub(super) fn run() {
                main();
            }
        }

        fn main() {
            if let Err(error) = $crate::__private::run_with_gungraun(
                __metabench_register,
                __metabench_gungraun::run,
                env!("CARGO_CRATE_NAME"),
            ) {
                eprintln!("metabench: {error}");
                std::process::exit(2);
            }
        }
    };
}
