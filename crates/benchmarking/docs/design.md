# Benchmarking design

## Purpose

The `benchmarking` crate provides the standard timing boundary for Criterion
benchmarks that use `Bencher::iter_custom()`. It keeps allocation-tracked
benchmarks consistent while allowing each benchmark to define its own operation,
prepared state, and allocation measurement.

## Sample timing

`time_sample()` executes a synchronous callback once per requested iteration.
`time_sample_async()` does the same for sequential asynchronous operations and
provides the zero-based iteration index to the callback.

Both helpers time only callback execution. Callback outputs pass through
`std::hint::black_box()` and are dropped before the next iteration, matching the
expected behavior for benchmarks without prepared per-iteration state.

## Prepared inputs

`time_sample_with_inputs()` accepts a complete vector with one input per
Criterion iteration. Callers prepare this vector before entering any allocation
or timing scope.

The helper invokes the measurement callback once with the exact input count,
borrows each input mutably once, and retains every output until the timed region
has ended. The returned measurement guard is dropped before inputs or outputs,
so teardown is excluded from both elapsed time and allocation measurement.

The helper does not select a Criterion `BatchSize` or divide a sample into
chunks. Benchmarks that need a different memory policy must use Criterion
directly and document their timing and lifetime boundaries.

## Empty samples

Every helper treats an empty sample as zero work and returns `Duration::ZERO`.
No benchmark or measurement callback is invoked.

## Design tenets

- One allocation measurement represents one Criterion sample.
- Per-iteration setup remains outside timing and allocation measurement.
- Prepared input and output teardown remains outside measurement.
- Shared helpers contain timing and optimization-barrier behavior.
- Benchmark-specific code defines only setup, measurement selection, and the
  operation under test.
