// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/metabench_macros/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/metabench_macros/favicon.ico"
)]

//! Procedural macros for the `metabench` crate.
//!
//! **Do not depend on this crate directly.** Use the re-exports from `metabench`
//! instead.

use proc_macro::TokenStream;

/// Defines one logical benchmark for all measurement engines.
///
/// # Grammar
///
/// The attribute accepts either positional arguments:
///
/// ```text
/// benchmark-input ::=
///     IDENTITY ","
///     GROUP_NAME ","
///     STRING_LITERAL
///     ("," benchmark-option)*
///     ","?
/// ```
///
/// or named arguments:
///
/// ```text
/// benchmark-input ::=
///     benchmark-field
///     ("," benchmark-field)*
///     ","?
///
/// benchmark-field ::=
///     "identity" "=" IDENTITY
///   | "group_name" "=" GROUP_NAME
///   | "benchmark_name" "=" STRING_LITERAL
///   | benchmark-option
///
/// benchmark-option ::=
///     "gungraun_config" "=" EXPR
///   | "gungraun_setup" "=" PATH
///   | "gungraun_teardown" "=" PATH
/// ```
///
/// Positional and named arguments cannot be mixed. In the named form, fields
/// may appear in any order. Every field may appear at most once.
///
/// # Required arguments
///
/// - **`identity`** (`IDENTITY` in the positional form) is an identifier for
///   the generated `BenchmarkIdentity` constant. The constant has the same
///   visibility as the annotated function and is passed to [`main!`].
/// - **`group_name`** (`GROUP_NAME`) is a constant expression of type
///   `&'static str` that names the logical benchmark group.
/// - **`benchmark_name`** (`STRING_LITERAL`) is a string literal that names
///   the logical benchmark within the group.
///
/// `IDENTITY`, `GROUP_NAME`, `STRING_LITERAL`, `EXPR`, and `PATH` use the
/// corresponding Rust syntax. The annotated function remains callable under
/// its original name and with its original visibility and signature.
///
/// Use the generated identity to register the same group and benchmark names
/// with Criterion:
///
/// ```ignore
/// use criterion::Criterion;
///
/// const GROUP_NAME: &str = "checksum";
///
/// #[metabench::benchmark(CHECKSUM, GROUP_NAME, "rolling")]
/// fn rolling_checksum() -> u64 {
///     std::hint::black_box([1_u8, 2, 3, 4])
///         .iter()
///         .map(|byte| u64::from(*byte))
///         .sum()
/// }
///
/// fn criterion_benchmarks(criterion: &mut Criterion) {
///     let mut group = criterion.benchmark_group(CHECKSUM.group_name());
///     group.bench_function(CHECKSUM.benchmark_name(), |bencher| {
///         bencher.iter(rolling_checksum);
///     });
///     group.finish();
/// }
/// ```
///
/// # Gungraun options
///
/// These options configure every Gungraun case declared on the annotated
/// function:
///
/// - **`gungraun_config`** evaluates to a
///   `gungraun::LibraryBenchmarkConfig`. Case-level Gungraun configuration can
///   override it.
/// - **`gungraun_setup`** is a path to a setup function. Gungraun passes each
///   case's arguments to this function and passes its return value to the
///   benchmark function.
/// - **`gungraun_teardown`** is a path to a teardown function. Gungraun passes
///   the benchmark function's return value to this function.
///
/// Declare one Gungraun case with `#[bench::CASE(...)]` or several cases with
/// `#[benches::CASES(...)]`. The case name after `::` must be unique within
/// the benchmark. These attributes use Gungraun's native library-benchmark
/// syntax, including the applicable `args`, `iter`, `config`, `setup`, and
/// `teardown` options. Conditional case declarations inside `cfg_attr` are
/// supported.
///
/// ```ignore
/// use gungraun::LibraryBenchmarkConfig;
///
/// fn prepare(size: usize) -> Vec<u8> {
///     vec![1; size]
/// }
///
/// #[metabench::benchmark(
///     identity = CHECKSUM,
///     group_name = "checksum",
///     benchmark_name = "rolling",
///     gungraun_config = LibraryBenchmarkConfig::default(),
///     gungraun_setup = prepare,
/// )]
/// #[bench::small(16)]
/// #[bench::large(4096)]
/// fn rolling_checksum(bytes: Vec<u8>) -> u64 {
///     bytes.iter().map(|byte| u64::from(*byte)).sum()
/// }
/// ```
///
/// For parameterized benchmarks, use the same text for a Criterion parameter
/// ID and the corresponding Gungraun case ID. Metabench then reports both
/// results under one logical case:
///
/// ```ignore
/// use criterion::{BenchmarkId, Criterion};
///
/// #[metabench::benchmark(PARSE, "parser", "parse")]
/// #[bench::short("hello")]
/// #[bench::long("a considerably longer input")]
/// fn parse(input: &str) -> usize {
///     input.split_whitespace().count()
/// }
///
/// fn criterion_benchmarks(criterion: &mut Criterion) {
///     let mut group = criterion.benchmark_group(PARSE.group_name());
///     for (id, input) in [
///         ("short", "hello"),
///         ("long", "a considerably longer input"),
///     ] {
///         group.bench_with_input(
///             BenchmarkId::new(PARSE.benchmark_name(), id),
///             &input,
///             |bencher, input| bencher.iter(|| parse(input)),
///         );
///     }
///     group.finish();
/// }
/// ```
///
/// # Function contract
///
/// The attribute supports free functions with typed parameters and return
/// values, type and lifetime generic parameters, `const fn`, and explicit
/// application binary interfaces such as `extern "C"`. It rejects:
///
/// - `async fn`;
/// - `unsafe fn` (the generated adapter cannot uphold an unsafe function's
///   safety contract; wrap the unsafe call in a safe function that
///   establishes its preconditions and benchmark that instead);
/// - const-generic functions;
/// - methods with a `self` receiver; and
/// - functions with variable argument lists.
///
/// Metabench applies `#[inline(never)]` to the benchmark function so every
/// engine measures a stable function boundary. Non-const functions are
/// measured by allocation tracking and Linux `perf` in addition to Criterion
/// and Gungraun. Benchmark functions declared with `const fn` cannot contain
/// that runtime instrumentation and therefore produce only Criterion and
/// Gungraun measurements.
#[proc_macro_attribute]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
pub fn benchmark(arguments: TokenStream, item: TokenStream) -> TokenStream {
    match metabench_macros_impl::benchmark(arguments.into(), item.into()) {
        Ok(output) => output,
        Err(error) => error.into_compile_error(),
    }
    .into()
}

/// Defines the benchmark target and generates its entry point.
///
/// # Grammar
///
/// The clauses must appear in the order shown. `benchmarks` and `groups` are
/// alternative forms: exactly one is required.
///
/// ```text
/// main-input ::=
///     criterion-clause ","
///     gungraun-clause?
///     target-clause
///     ("," allocator-clause)?
///     ","?
///
/// criterion-clause ::=
///     "criterion" "=" PATH
///   | "criterion" "=" "{"
///         "factory" "=" PATH ","
///         "benchmarks" "=" PATH ","
///         ("unit" "=" STRING_LITERAL ",")?
///     "}"
///
/// gungraun-clause ::=
///     "gungraun" "=" "{"
///         ("config" "=" EXPR ";")?
///         ("setup" "=" EXPR ";")?
///         ("teardown" "=" EXPR ";")?
///     "}" ","
///
/// target-clause ::=
///     "benchmarks" "=" "[" IDENTITY ("," IDENTITY)* ","? "]"
///   | "groups" "=" "{"
///         GROUP_NAME "{"
///             "benchmarks" "=" "[" IDENTITY ("," IDENTITY)* ","? "]"
///             ("," group-option)* ","?
///         "}"
///         ("," GROUP_NAME "{" ... "}")*
///         ","?
///     "}"
///
/// group-option ::=
///     "gungraun_config" "=" EXPR
///   | "gungraun_compare_by_id" "=" BOOLEAN_LITERAL
///   | "gungraun_max_parallel" "=" EXPR
///   | "gungraun_setup" "=" EXPR
///   | "gungraun_teardown" "=" EXPR
///
/// allocator-clause ::= "allocator" "=" PATH
/// ```
///
/// `PATH`, `EXPR`, `IDENTITY`, and `GROUP_NAME` use the corresponding Rust
/// syntax. Every benchmark and group list must be nonempty. Group names must
/// be unique, and each benchmark identity may appear only once in the target.
///
/// # Criterion options
///
/// - **`criterion = benchmark_registration`** uses
///   `criterion::Criterion::default()` and calls the function at
///   `benchmark_registration` with `&mut Criterion`. The report unit is
///   nanoseconds (`"ns"`).
/// - **`criterion.factory`** is a path to a zero-argument function that returns
///   the configured `Criterion` instance. Metabench applies Criterion's command
///   line arguments to the returned instance.
/// - **`criterion.benchmarks`** is a path to the registration function called
///   with `&mut Criterion`.
/// - **`criterion.unit`** is the optional stable base-unit label for a custom
///   Criterion measurement. It must match the unit represented by the
///   measurement's `to_f64` values so reports and baselines compare compatible
///   values. Omit it when the custom measurement has no meaningful unit.
///
/// # Gungraun options
///
/// The optional **`gungraun`** block configures the complete Gungraun suite:
///
/// - **`config`** evaluates to a `gungraun::LibraryBenchmarkConfig` applied to
///   every generated group. Use it to select tools and configure output,
///   sandboxes, limits, flame graphs, and tool arguments.
/// - **`setup`** is an expression run once before all Gungraun groups.
/// - **`teardown`** is an expression run once after all Gungraun groups.
///
/// These fields use Gungraun's native syntax: they are ordered as above and
/// terminated with semicolons.
///
/// # Benchmark and group options
///
/// - **`benchmarks = [IDENTITY, ...]`** is the concise form. It includes the
///   listed identities created by [`benchmark`] and gives each one
///   an independent Gungraun group with default group settings.
/// - **`groups = { ... }`** assigns identities to named Gungraun groups. Use
///   this form when benchmarks share group configuration or lifecycle hooks.
/// - **`benchmarks` inside a group** is the nonempty list of
///   [`benchmark`] identities belonging to that group.
/// - **`gungraun_config`** evaluates to a
///   `gungraun::LibraryBenchmarkConfig` applied to every benchmark in the
///   group. Group configuration is combined with suite and benchmark
///   configuration according to Gungraun's normal precedence rules.
/// - **`gungraun_compare_by_id`** is a boolean literal. When `true`, Gungraun
///   compares benchmark cases whose case IDs match. The default is `false`.
/// - **`gungraun_max_parallel`** controls the number of group benchmarks that
///   Gungraun may execute concurrently when its `--parallel` option is active.
///   `0` means no limit, `1` forces serial execution, and values of at least
///   `2` set the maximum concurrency. The default is no limit.
/// - **`gungraun_setup`** is an expression run once before the group's
///   benchmarks.
/// - **`gungraun_teardown`** is an expression run once after the group's
///   benchmarks.
///
/// # Allocator option
///
/// **`allocator = PATH`** selects the global allocator wrapped by allocation
/// tracking. The path must name a value usable as the underlying
/// `GlobalAlloc`. If omitted, metabench wraps `std::alloc::System`. The macro
/// declares the target's global allocator, so the target must not declare a
/// second one.
///
/// # Example
///
/// ```ignore
/// metabench::main!(
///     criterion = {
///         factory = configured_criterion,
///         benchmarks = criterion_benchmarks,
///         unit = "cycles",
///     },
///     gungraun = {
///         config = suite_config();
///         setup = prepare_suite();
///         teardown = clean_up_suite();
///     },
///     groups = {
///         PARSERS {
///             benchmarks = [PARSE, PARSE_PREFIX],
///             gungraun_config = parser_config(),
///             gungraun_compare_by_id = true,
///             gungraun_max_parallel = 1,
///             gungraun_setup = prepare_parsers(),
///             gungraun_teardown = clean_up_parsers(),
///         },
///     },
///     allocator = MyAllocator,
/// );
/// ```
#[proc_macro]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
pub fn main(input: TokenStream) -> TokenStream {
    match metabench_macros_impl::main(input.into()) {
        Ok(output) => output,
        Err(error) => error.into_compile_error(),
    }
    .into()
}
