# Benchmarking implementation

## Clock boundaries

The helpers use `std::time::Instant`. The clock starts immediately before the
first benchmark callback and is read immediately after the final callback.
Empty samples return before reading the clock.

## Optimization barriers

Every callback result passes through `std::hint::black_box()`. Synchronous and
asynchronous samples discard each result after the barrier. Prepared-input
samples push results into a preallocated vector so they remain alive until the
sample ends.

## Prepared-input ownership

`time_sample_with_inputs()` takes ownership of the prepared input vector. It
allocates output storage before opening the caller-provided measurement guard.
The operation receives each input through a mutable reference, preserving input
ownership for the entire sample.

After reading the clock, the helper explicitly drops values in this order:

1. The measurement guard.
2. The output vector.
3. The input vector.

This order keeps all setup and teardown outside the measurement scope.

## Measurement abstraction

The measurement callback is generic over its return type. The helper relies only
on Rust RAII, so it can host `alloc_tracker` thread or process spans without
depending on `alloc_tracker` in normal builds.

## Verification

Unit tests verify callback counts, asynchronous iteration indexes, empty-sample
behavior, and the ordering between measurement completion and input/output
destruction. Rustdoc examples compile the standard Criterion and
`alloc_tracker` integration patterns.
