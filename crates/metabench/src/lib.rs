// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/metabench/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/metabench/favicon.ico")]

//! Unified benchmark reports from [Criterion](https://crates.io/crates/criterion), [Gungraun](https://crates.io/crates/gungraun),
//! Linux `perf`, Intel `VTune`, and
//! allocation tracking.
//!
//! With `metabench`, you:
//!
//! - Write a benchmark once and trivially run it with different benchmark tools. For example,
//!   a benchmark can be run with Criterion to get wall-clock time, run with Gungraun to get
//!   instruction count, and run with allocation tracker to get a count of allocated memory.
//!
//! - Compile your benchmark binaries once, instead of once per benchmark tool. Since benchmark
//!   binaries are usually compiled in release mode, compiling just once can save a lot of time.
//!
//! - Get a unified report showing a consolidated view of the results from the various benchmark tools.
//!
//! # Getting started
//!
//! You usually start by writing a mostly normal Criterion benchmark:
//!
//! ```
//! # mod benchmark {
//! use criterion::Criterion;
//!
//! fn calculate_checksum(input: &[u8]) -> u64 {
//!     input.iter().map(|byte| u64::from(*byte)).sum()
//! }
//!
//! #[metabench::benchmark(CHECKSUM, PARSER_GROUP, "checksum")]
//! fn checksum_benchmark() -> u64 {
//!     calculate_checksum(std::hint::black_box(b"a shared input"))
//! }
//!
//! const PARSER_GROUP: &str = "parser";
//!
//! fn criterion_benchmarks(criterion: &mut Criterion) {
//!     let mut group = criterion.benchmark_group(CHECKSUM.group_name());
//!     group.bench_function(CHECKSUM.benchmark_name(), |bencher| {
//!         bencher.iter(checksum_benchmark);
//!     });
//!     group.finish();
//! }
//!
//! metabench::main!(criterion = criterion_benchmarks, benchmarks = [CHECKSUM],);
//! # }
//! ```
//!
//! You'll see two differences compared to a classic Criterion benchmark:
//!
//! - Benchmark methods are annotated with the `#[benchmark]` attribute.
//! - `metabench::main!` is used instead of `criterion::main!`.
//!
//! That little bit of magic is all that you need to enable your benchmark function to be evaluated
//! by the different benchmark tools. Save the above to `parser.rs` and add the following
//! to your Cargo.toml file:
//!
//! ```toml
//! [[bench]]
//! name = "parser"
//! harness = false
//! ```
//!
//! Run the benchmark with:
//!
//! ```bash
//! cargo bench --bench parser
//! ```
//!
//! This will run for a few seconds and produce a result table on the terminal, along with JSON and
//! markdown reports in your `target` directory.
//!
//! # Measurement engines
//!
//! A **native case** is a case supplied by a benchmark's Gungraun declarations,
//! including its implicit single case when no case attribute is present.
//! **Probes** are the separate allocation, perf, and `VTune` workers that execute
//! these cases; they do not use Criterion registrations.
//!
//! As mentioned, metabench supports five distinct benchmark tools: Criterion, Gungraun, allocation
//! tracking, Linux `perf`, and Intel `VTune`:
//!
//! - **Criterion** reports wall-clock execution time and throughput. Use
//!   ordinary Criterion registration, sampling, plotting, and profiling APIs.
//!
//! - **Gungraun** reports benchmark metrics from Callgrind, Cachegrind, and
//!   DHAT, including instruction counts, cache behavior, branch behavior when
//!   enabled, system calls, and heap activity.
//!
//! - **Allocation tracking** reports allocation count and allocated bytes for
//!   one invocation of every declared native case. Counts
//!   are process-wide, so concurrent threads can contribute allocations.
//!
//! - **Linux `perf`** reports supported hardware and software counters for one
//!   invocation of every declared native case. Collection starts and stops in
//!   a private caller around the workload, excluding input preparation and
//!   returned-value teardown. The collection window can include caller and
//!   control overhead; it is not a claim of exact function-only counters.
//!   Use repeated `--perf-arg` options for additional
//!   `perf stat` arguments.
//!
//! - **Intel `VTune`** reports hardware event counts for one invocation of every
//!   declared native case, using the same private caller
//!   measurement as Linux `perf`. Because `VTune`'s hardware-event collection
//!   is a statistical sampler rather than an exact counter, reported counts
//!   are sampling estimates that can read zero for a short workload, and the
//!   measured window extends slightly beyond the workload to include the
//!   pause round-trip that stops collection; callers who need exact
//!   per-invocation counts should prefer `perf` instead. Use repeated
//!   `--vtune-arg` options for additional `vtune -collect-with runsa`
//!   arguments. Requires a `vtune` installation reachable on `PATH`.
//!
//! Criterion and allocation tracking are always enabled by default; Gungraun is
//! also enabled by default on Linux, where it is available. You can control the
//! specific engines to run from the command-line by passing the
//! `--criterion`, `--gungraun`, `--allocations`, `--perf`, `--vtune`, or `--all-engines` options:
//!
//! ```bash
//! cargo bench --bench parser -- --criterion
//! ```
//!
//! Some limitations around engines:
//!
//! - Gungraun and Linux `perf` are only available on Linux platforms. `VTune` is
//!   available on Linux and Windows, provided a `vtune` installation is
//!   reachable on `PATH`. `VTune`'s integration test exercises the control
//!   protocol and CSV-report parsing on every platform against a fake
//!   `vtune` fixture (see `tests/spawned_benchmark.rs`), rather than a real
//!   `VTune` install.
//!
//! # Workload boundaries
//!
//! The attribute preserves a non-inlined workload symbol without injecting
//! runtime collector checks into the function. Criterion callbacks call it
//! directly, and Gungraun uses its normal adapter. Neither executes the private
//! allocation, perf, or `VTune` collection controls.
//!
//! Allocation and hardware workers reuse the existing `#[bench::...]` and
//! `#[benches::...]` declarations through separate generated adapters. Native
//! suite and group hooks run outside collection. Case argument evaluation and
//! setup precede collection; teardown and returned-value destruction follow it.
//! Calls to annotated functions during setup cannot consume the measurement.
//! Collection guards stop on unwinding.
//!
//! No new registration language or public measurement helper is required. Probes
//! do not run Criterion registration or infer its arbitrary callbacks. A
//! Criterion-only case without a corresponding native declaration has no probe
//! result. Matching case names do not prove matching Criterion inputs or cleanup;
//! those remain author responsibilities.
//!
//! Probe execution reuses native case arguments and lifecycle hooks, not
//! Valgrind configuration such as sandboxes or environment overrides. Configure
//! hardware tools with their own forwarded arguments.
//!
//! Input destruction inside the benchmark function is included; return owned
//! inputs with the output if their destruction should happen outside collection.
//! Functions declared with `const fn` can also have runtime probes.
//!
//! # Configure Criterion
//!
//! Provide a Criterion factory to customize measurements or other Criterion
//! settings. For custom measurements, set `unit` to the stable base unit
//! returned by the measurement's `to_f64` method:
//!
//! ```ignore
//! metabench::main!(
//!     criterion = {
//!         factory = configured_criterion,
//!         benchmarks = criterion_benchmarks,
//!         unit = "cycles",
//!     },
//!     benchmarks = [MEASURE],
//! );
//! ```
//!
//! # Configure Gungraun
//!
//! The installed `gungraun-runner` must match the Gungraun library version
//! (currently 0.20.0). Metabench reads that runner's schema-v7 summaries.
//!
//! Gungraun 0.20 adds batched execution paths that retain prepared inputs until each batch finishes.
//! Do not borrow a temporary in a native case, such as `#[bench::case(&prepare())]`.
//! Prepare an owned input, such as a `Box`, and return its owner with the result
//! to keep input destruction outside measurement.
//! Criterion can continue to borrow its prepared state and select the result
//! from the returned tuple.
//! See [owned native inputs](https://github.com/microsoft/oxidizer/blob/main/docs/benchmarks.md#owned-native-inputs)
//! for the migration recipe.
//!
//! Metabench serializes the perf-control calls in its generated Gungraun adapters within each process.
//! The lock covers the whole native perf callback.
//! Nested sections panic instead of waiting for their own lock.
//! If a callback panics, later perf callbacks fail instead of reusing its shared state.
//! A workload can still use multiple threads.
//! Do not call Gungraun's `perf_enable!` or `perf_disable!` macros directly from workloads or hooks.
//! Direct calls bypass this lock and can race with generated controls.
//! This restriction applies until [Gungraun fixes its shared control state](https://github.com/gungraun/gungraun/issues/778).
//! Callgrind, Cachegrind, DHAT, and the separate Metabench `--perf` engine do not use these controls.
//!
//! Benchmark-level `gungraun_config`, `gungraun_setup`, and
//! `gungraun_teardown` options work with native `#[bench::...]` and
//! `#[benches::...]` cases:
//!
//! ```ignore
//! #[metabench::benchmark(CHECKSUM, PARSER_GROUP, "checksum",
//!     gungraun_config = benchmark_config(),
//!     gungraun_setup = prepare_input,
//!     gungraun_teardown = clean_up,
//! )]
//! fn checksum_benchmark() -> u64 {
//!     calculate_checksum(std::hint::black_box(b"a shared input"))
//! }
//! ```
//!
//! When a function uses `#[bench::case_name(...)]` or
//! `#[benches::case_name(...)]`, Gungraun reports one row per case, identified
//! internally as `<group_name>/<benchmark_name>/<case_name>`. For those rows
//! to line up with Criterion's in the consolidated report, register the
//! matching Criterion benchmark under exactly the same case name, for example
//! with `group.bench_with_input(BenchmarkId::new(CHECKSUM.benchmark_name(), "case_name"), ...)`.
//! A mismatched or missing case name does not error: it silently keeps the
//! engines' rows separate (or drops one engine's row from the merged view)
//! instead of reporting them together.
//!
//! [`main!`] accepts suite-level Gungraun settings and named groups with their
//! own configuration, setup, teardown, comparison, and parallelism:
//!
//! ```ignore
//! metabench::main!(
//!     criterion = criterion_benchmarks,
//!     gungraun = {
//!         config = LibraryBenchmarkConfig::default()
//!             .tool(Dhat::default());
//!         setup = prepare_suite();
//!         teardown = clean_up_suite();
//!     },
//!     groups = {
//!         PARSERS {
//!             benchmarks = [PARSE, PARSE_PREFIX],
//!             gungraun_config = parser_group_config(),
//!             gungraun_compare_by_id = true,
//!             gungraun_max_parallel = 1,
//!             gungraun_setup = prepare_group(),
//!             gungraun_teardown = clean_up_group(),
//!         },
//!     },
//! );
//! ```
//!
//! Use the shorter `benchmarks = [PARSE, PARSE_PREFIX]` form when no shared
//! group settings are needed. Gungraun's library benchmark configuration,
//! tools, sandboxes, limits, flame graphs, delays, and lifecycle hooks remain
//! available through these options.
//!
//! # Reports and baselines
//!
//! Each run prints a consolidated result table to the terminal and writes `report.json` and `report.md`
//! under `target/metabench/<benchmark-target>/`. Reports include benchmark
//! metadata, environment information, measured engines, and their metrics.
//! JSON retains every available metric and includes human-readable display
//! names.
//! Criterion's `Time` is its slope estimate when available, otherwise its mean,
//! with the statistic and matching confidence level and bounds retained in JSON
//! and Markdown. Custom measurement units remain unchanged. `Input bytes` denotes
//! the declared work size, not allocated memory or a throughput rate.
//! Confidence levels must be strictly between zero and one, with finite, ordered
//! bounds. Bootstrap intervals need not contain the point estimate. Markdown
//! encodes report text as literal content, and terminal cells visibly escape
//! controls and untrusted line breaks; JSON preserves the original strings.
//!
//! When the JSON output path already exists, it is used as the baseline before
//! being replaced. `--baseline` selects another report, while `--no-baseline`
//! disables comparison. Report schema 9 distinguishes these estimates from old
//! median-only reports. Old automatic baselines are warned about and skipped;
//! an explicitly selected unsupported schema is an error. Different target,
//! compiler, CPU, profile, or compiler flags also warn and skip comparison.
//! Compiler identity is captured when the executable is built, not from the
//! runtime `RUSTC` override or the compiler currently on `PATH`.
//! These checks do not prove identical hardware, features, inputs, or boundaries.
//! Metrics with compatible names, units, directions, and timing statistics
//! are classified as improved, stable, or regressed. The default regression
//! threshold is five percent; change it with `--regression-threshold`.
//! These percentage labels are not statistical significance or equivalence tests.
//!
//! Smoke runs (`--test`, including a forwarded native `--test`) run Criterion or
//! Gungraun cases briefly without collecting probe metrics or publishing reports.
//! Allocation, perf, and `VTune` smoke selections fall back to Criterion registration,
//! which must be provided even when the target declares native probe cases.
//! An invocation
//! without arguments is also smoke mode, matching Cargo's harness-free test
//! invocation; pass an engine selector to request measurements directly.
//!
//! Gungraun also accepts `--test=true` and `--test=false`.
//! Forward these forms with `--gungraun-arg`.
//! When only Gungraun is selected, arguments after `--` accept these forms too.
//! The true values are `true`, `t`, `yes`, `y`, `on`, and `1`.
//! The false values are `false`, `f`, `no`, `n`, `off`, and `0`.
//! Letter case does not matter.
//!
//! A true value selects smoke mode for the whole run.
//! A false value keeps measurement mode unless another smoke request is present.
//! Top-level `--test` takes priority.
//!
//! A measurement selection matching no cases fails and preserves existing reports.
//! Failed multi-engine runs can write separate `.partial` reports but never
//! replace the complete rolling baseline.
//!
//! Default native artifacts and combined reports follow the Cargo output root
//! of the running executable, including `--target-dir`, target triples, and recognized
//! custom profiles. `CARGO_TARGET_DIR` identifies a target root whose name matches
//! the target triple without overriding a different executable root selected by Cargo.
//! A relocated executable in an unknown layout needs an explicit
//! `CARGO_TARGET_DIR`; metabench does not guess a workspace root.
//! Hidden worker stdout and stderr share a pipe. Each run log retains at most
//! 1 MiB of raw output, keeping the tail with an explicit truncation marker.
//! Failures and timeouts show the final 8 KiB and the log path. Capture is bounded
//! while workers run, and inherited pipe writers cannot delay cleanup indefinitely.
//! `--show-engine-output` instead inherits the terminal without capturing a log.
//!
//! # Command-line options
//!
//! - `--criterion`, `--gungraun`, `--allocations`, `--perf`, `--vtune`, and
//!   `--all-engines`: select measurement engines.
//!
//! - `--criterion-arg ARG`, `--gungraun-arg ARG`, `--perf-arg ARG`,
//!   `--vtune-arg ARG`: forward one argument to an engine; repeat the option
//!   for multiple arguments.
//!
//! - `--output PATH`: write `PATH.json` and `PATH.md`.
//!
//! - `--export-json PATH`, `--export-md PATH`: choose individual report paths.
//!
//! - `--no-output`: print results without writing report files.
//!
//! - `--baseline PATH`, `--no-baseline`: select or disable baseline comparison.
//!
//! - `--regression-threshold PERCENT`: set the regression threshold.
//!
//! - `--list`: use each selected engine's benchmark listing mode without
//!   producing reports.
//!
//! - `--timeout 30s`: limit each engine separately. Durations accept `ms`, `s`,
//!   `m`, or `h`.
//!
//! - `--fail-fast`, `--keep-going`: stop at the first engine failure or run all
//!   selected engines. The default is `--keep-going`.
//!
//! - `--show-engine-output`: display engine output that is normally hidden.
//!
//! - `--help`: print the complete command-line reference.
//!
//! Arguments after `--` are forwarded directly when exactly one engine is
//! selected. The `BENCH_ENGINE` environment variable may select `criterion`,
//! `gungraun`, `perf`, `vtune`, or `allocations` when no engine selector is present.
//! `CRITERION_HOME` and `GUNGRAUN_HOME` select the corresponding engine data
//! directories.
//! Filters remain engine-specific: use Criterion arguments for Criterion and
//! probes, and Gungraun arguments for Gungraun. Probes apply the Criterion name
//! filter (and `--exact`) to declared native case identities, then run a private
//! indexed worker. Criterion timing, profiling, and baseline options do not
//! affect probe execution. A common cross-engine filter is not supported.
//! Native Gungraun symbols include readable registration-identity suffixes and
//! collision-safe hashes; run manifests retain native-to-canonical mappings.
//! Renamed native symbols do not automatically reconnect older native histories.
//!
//! # Custom memory allocator
//!
//! The system allocator
//! is used by default; `main!(..., allocator = MyAllocator)` selects another
//! underlying allocator.
//! Put `allocator` after `benchmarks` or `groups` in [`main!`]. The executable
//! may define only one global allocator. Any existing `alloc_tracker`
//! instrumentation must resolve to the same Cargo package source as the metabench
//! tracker; a registry dependency and a separate path dependency have independent
//! tracking state even if their names and versions match.
//! The generated allocator checks one process-wide atomic flag on allocation,
//! reallocation, and deallocation. Tracking is enabled only in allocation workers; Criterion,
//! Gungraun, perf, and `VTune` bypass the tracking counters and thread-local state.
//! The flag check still adds overhead to allocating workloads; it is not a
//! zero-overhead claim.
//! Both paths use the same underlying allocator instance, including custom
//! allocators. Allocation workers track process-wide totals, with snapshots
//! excluding setup and teardown from reported counts.

#[doc(hidden)]
pub mod __private;

mod allocation;
mod allocator;
mod arguments;
mod artifact;
mod entrypoint;
mod error;
mod gungraun_perf;
mod identity;
mod metric_display;
mod mode;
mod perf;
mod probe;
mod probe_error;
mod report;
mod runner;
mod vtune;
mod worker_log;

#[doc(inline)]
pub use identity::BenchmarkIdentity;
pub use metabench_macros::{benchmark, main};
