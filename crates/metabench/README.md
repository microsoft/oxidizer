<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Metabench Logo" width="96">

# Metabench

[![crate.io](https://img.shields.io/crates/v/metabench.svg)](https://crates.io/crates/metabench)
[![docs.rs](https://docs.rs/metabench/badge.svg)](https://docs.rs/metabench)
[![MSRV](https://img.shields.io/crates/msrv/metabench)](https://crates.io/crates/metabench)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Unified benchmark reports from [Criterion][__link0], [Gungraun][__link1],
Linux `perf`, Intel `VTune`, and
allocation tracking.

With `metabench`, you:

* Write a benchmark once and trivially run it with different benchmark tools. For example,
  a benchmark can be run with Criterion to get wall-clock time, run with Gungraun to get
  instruction count, and run with allocation tracker to get a count of allocated memory.

* Compile your benchmark binaries once, instead of once per benchmark tool. Since benchmark
  binaries are usually compiled in release mode, compiling just once can save a lot of time.

* Get a unified report showing a consolidated view of the results from the various benchmark tools.

## Getting started

You usually start by writing a mostly normal Criterion benchmark:

```rust
use criterion::Criterion;

fn calculate_checksum(input: &[u8]) -> u64 {
    input.iter().map(|byte| u64::from(*byte)).sum()
}

#[metabench::benchmark(CHECKSUM, PARSER_GROUP, "checksum")]
fn checksum_benchmark() -> u64 {
    calculate_checksum(std::hint::black_box(b"a shared input"))
}

const PARSER_GROUP: &str = "parser";

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(CHECKSUM.group_name());
    group.bench_function(CHECKSUM.benchmark_name(), |bencher| {
        bencher.iter(checksum_benchmark);
    });
    group.finish();
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [CHECKSUM],);
```

You’ll see two differences compared to a classic Criterion benchmark:

* Benchmark methods are annotated with the `#[benchmark]` attribute.
* `metabench::main!` is used instead of `criterion::main!`.

That little bit of magic is all that you need to enable your benchmark function to be evaluated
by the different benchmark tools. Save the above to `parser.rs` and add the following
to your Cargo.toml file:

```toml
[[bench]]
name = "parser"
harness = false
```

Run the benchmark with:

```bash
cargo bench --bench parser
```

This will run for a few seconds and produce a result table on the terminal, along with JSON and
markdown reports in your `target` directory.

## Measurement engines

As mentioned, metabench supports five distinct benchmark tools: Criterion, Gungraun, allocation
tracking, Linux `perf`, and Intel `VTune`:

* **Criterion** reports wall-clock execution time and throughput. Use
  ordinary Criterion registration, sampling, plotting, and profiling APIs.

* **Gungraun** reports benchmark metrics from Callgrind, Cachegrind, and
  DHAT, including instruction counts, cache behavior, branch behavior when
  enabled, system calls, and heap activity.

* **Allocation tracking** reports allocation count and allocated bytes for
  one invocation of every discovered Criterion case.

* **Linux `perf`** reports supported hardware and software counters for one
  invocation of every discovered Criterion case. Counters cover only the
  annotated workload. Use repeated `--perf-arg` options for additional
  `perf stat` arguments.

* **Intel `VTune`** reports hardware event counts for one invocation of every
  discovered Criterion case, using the same annotated-workload
  measurement as Linux `perf`. Use repeated `--vtune-arg` options for
  additional `vtune -collect-with runsa` arguments. Requires a `vtune`
  installation reachable on `PATH`.

Criterion and allocation tracking are always enabled by default; Gungraun is
also enabled by default on Linux, where it is available. You can control the
specific engines to run from the command-line by passing the
`--criterion`, `--gungraun`, `--allocations`, `--perf`, `--vtune`, or `--all-engines` options:

```bash
cargo bench --bench parser -- --criterion
```

Some limitations around engines:

* Benchmark functions declared with `const fn` can be measured by Criterion and Gungraun, but not
  by allocation tracking, `perf`, or `VTune`.

* Gungraun and Linux `perf` are only available on Linux platforms. `VTune` is
  available on Linux and Windows, provided a `vtune` installation is
  reachable on `PATH`. `VTune`’s integration test exercises the control
  protocol and CSV-report parsing on every platform against a fake
  `vtune` fixture (see `tests/spawned_benchmark.rs`), rather than a real
  `VTune` install.

## Configure Criterion

Provide a Criterion factory to customize measurements or other Criterion
settings. For custom measurements, set `unit` to the stable base unit
returned by the measurement’s `to_f64` method:

```rust
metabench::main!(
    criterion = {
        factory = configured_criterion,
        benchmarks = criterion_benchmarks,
        unit = "cycles",
    },
    benchmarks = [MEASURE],
);
```

## Configure Gungraun

Benchmark-level `gungraun_config`, `gungraun_setup`, and
`gungraun_teardown` options work with native `#[bench::...]` and
`#[benches::...]` cases:

```rust
#[metabench::benchmark(CHECKSUM, PARSER_GROUP, "checksum",
    gungraun_config = benchmark_config(),
    gungraun_setup = prepare_input,
    gungraun_teardown = clean_up,
)]
fn checksum_benchmark() -> u64 {
    calculate_checksum(std::hint::black_box(b"a shared input"))
}
```

When a function uses `#[bench::case_name(...)]` or
`#[benches::case_name(...)]`, Gungraun reports one row per case, identified
internally as `<group_name>/<benchmark_name>/<case_name>`. For those rows
to line up with Criterion’s in the consolidated report, register the
matching Criterion benchmark under exactly the same case name, for example
with `group.bench_with_input(BenchmarkId::new(CHECKSUM.benchmark_name(), "case_name"), ...)`.
A mismatched or missing case name does not error: it silently keeps the
engines’ rows separate (or drops one engine’s row from the merged view)
instead of reporting them together.

[`main!`][__link2] accepts suite-level Gungraun settings and named groups with their
own configuration, setup, teardown, comparison, and parallelism:

```rust
metabench::main!(
    criterion = criterion_benchmarks,
    gungraun = {
        config = LibraryBenchmarkConfig::default()
            .tool(Dhat::default());
        setup = prepare_suite();
        teardown = clean_up_suite();
    },
    groups = {
        PARSERS {
            benchmarks = [PARSE, PARSE_PREFIX],
            gungraun_config = parser_group_config(),
            gungraun_compare_by_id = true,
            gungraun_max_parallel = 1,
            gungraun_setup = prepare_group(),
            gungraun_teardown = clean_up_group(),
        },
    },
);
```

Use the shorter `benchmarks = [PARSE, PARSE_PREFIX]` form when no shared
group settings are needed. Gungraun’s library benchmark configuration,
tools, sandboxes, limits, flame graphs, delays, and lifecycle hooks remain
available through these options.

## Reports and baselines

Each run prints a consolidated result table to the terminal and writes `report.json` and `report.md`
under `target/metabench/<benchmark-target>/`. Reports include benchmark
metadata, environment information, measured engines, and their metrics.
JSON retains every available metric and includes human-readable display
names.

When the JSON output path already exists, it is used as the baseline before
being replaced. `--baseline` selects another report, while `--no-baseline`
disables comparison. Metrics with compatible names, units, and directions
are classified as improved, stable, or regressed. The default regression
threshold is five percent; change it with `--regression-threshold`.

## Command-line options

* `--criterion`, `--gungraun`, `--allocations`, `--perf`, `--vtune`, and
  `--all-engines`: select measurement engines.

* `--criterion-arg ARG`, `--gungraun-arg ARG`, `--perf-arg ARG`,
  `--vtune-arg ARG`: forward one argument to an engine; repeat the option
  for multiple arguments.

* `--output PATH`: write `PATH.json` and `PATH.md`.

* `--export-json PATH`, `--export-md PATH`: choose individual report paths.

* `--no-output`: print results without writing report files.

* `--baseline PATH`, `--no-baseline`: select or disable baseline comparison.

* `--regression-threshold PERCENT`: set the regression threshold.

* `--list`: use each selected engine’s benchmark listing mode without
  producing reports.

* `--timeout 30s`: limit each engine separately. Durations accept `ms`, `s`,
  `m`, or `h`.

* `--fail-fast`, `--keep-going`: stop at the first engine failure or run all
  selected engines. The default is `--keep-going`.

* `--show-engine-output`: display engine output that is normally hidden.

* `--help`: print the complete command-line reference.

Arguments after `--` are forwarded directly when exactly one engine is
selected. The `BENCH_ENGINE` environment variable may select `criterion`,
`gungraun`, `perf`, `vtune`, or `allocations` when no engine selector is present.
`CRITERION_HOME` and `GUNGRAUN_HOME` select the corresponding engine data
directories.

## Custom memory allocator

The system allocator
is used by default; `main!(..., allocator = MyAllocator)` selects another
underlying allocator.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/metabench">source code</a>.
</sub>

 [__link0]: https://crates.io/crates/criterion
 [__link1]: https://crates.io/crates/gungraun
 [__link2]: `main!`
