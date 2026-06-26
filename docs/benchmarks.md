# Benchmarks

This chapter covers the conventions for writing Criterion (wall-clock) and
Callgrind (simulated instruction-count) benchmarks. It is the entry point for
benchmark work; the deep references are
[`docs/callgrind-benchmarks.md`](callgrind-benchmarks.md) for the Callgrind
strategy and [`docs/naming.md`](naming.md) for file and identifier naming.

## Benchmark design

Unless otherwise prompted, create single-threaded synchronous Criterion
benchmarks. Use benchmark groups to group related benchmarks that make sense to
compare to each other.

Focus on benchmarking elementary operations, do not create benchmarks with lots of
long-winded logic. We generally want to benchmark a single API call or at most a
sequence of closely coupled API calls.

Only the functionality being benchmarked should be inside the `.iter()` closure,
with the data setup being either done outside (if not per-iteration) or using the
first "payload preparation" callback of `iter_batched()` (if per-iteration).

If multithreaded benchmarks are truly appropriate, use `bench_on_threadpool()` for
them. When using this for multithreaded benchmarks, also run any single-threaded
benchmarks via `bench_on_threadpool()` to ensure that overheads are comparable.

Inside the benchmark closure, use `std::hint::black_box()` to consume output
values from the code being benchmarked, to avoid unrealistic eager optimizations
due to output values that are discarded.

Benchmarks that are meant to be compared to each other must be in the same
benchmark function and in the same benchmark group.

Do not forget to register benchmarks in `Cargo.toml`.

Benchmark file names, Criterion group names, and Callgrind group/function names
follow strict conventions documented in [`docs/naming.md`](naming.md): the file
basename prefixes Criterion group names, Callgrind files require a paired
Criterion file, and Callgrind identifiers mirror Criterion ones with `/`
substituted by `_`.

## Stack pin vs. `Box::pin` on the measured path

Do not use `Box::pin(value)` on the measured path. It allocates a `Box` on the
heap on every iteration, which can easily add 100-200 instructions (or 40-50% of
the measurement) of pure allocator overhead that has nothing to do with the
operation under test. Use `std::pin::pin!(value)` instead — it pins on the stack
with zero allocation. Add a brief inline comment justifying the deviation from the
usual `Box::pin` preference (e.g. "stack-pin to avoid allocator noise on the
measured path"). This is an exception to the workspace-wide rule against the
`pin!` macro (see the examples chapter).

`Box::pin` remains correct in benchmark code that is **not** inside the measured
region:

* Criterion `iter_custom` setup (anything before `Instant::now()`).
* The first ("payload preparation") callback of `iter_batched()`.
* Gungraun setup functions referenced via `#[bench::id(setup_fn())]` — these run
  outside the measured region and pass the result into the bench body by value.
* Helper functions that must return `Pin<Box<T>>` across a function boundary (a
  stack pin would dangle).
* Intentional `Box::pin` baselines where the allocation IS what is being
  measured.

## Callgrind benchmarks

For performance-critical hot paths, complement the Criterion benchmarks with
Callgrind-based instruction-count benchmarks. See
[`docs/callgrind-benchmarks.md`](callgrind-benchmarks.md) for the strategy,
scenario selection, bench file template, the Criterion-pairing convention, and
how to interpret results.
