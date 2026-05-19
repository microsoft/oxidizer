# Multitude Testing and Validation

Since `multitude` contains a lot of tricky unsafe code, much effort went to
ensure it is correct:

* **[Clippy](https://doc.rust-lang.org/stable/clippy/)**. Catches common
  mistakes and enforces idiomatic Rust patterns through hundreds of
  additional lints.

* **[Miri](https://github.com/rust-lang/miri)**. Detects undefined behavior
  and memory safety violations by executing code in an interpreted
  environment.

* **[`cargo-careful`](https://crates.io/crates/cargo-careful)**. Runs tests
  with the standard library's internal safety checks enabled to catch
  violated invariants.

* **[`cargo-mutants`](https://crates.io/crates/cargo-mutants)**. Validates
  test suite depth by injecting synthetic bugs to ensure your tests can
  actually detect them.

* **[`loom`](https://crates.io/crates/loom)**. Exhaustively tests concurrent
  code by exploring all possible thread interleavings to find subtle race
  conditions.

* **[`bolero`](https://crates.io/crates/bolero)**. Combines fuzzing and
  property-based testing to automatically surface edge cases with
  randomized input generation.

For performance, we use complementary
[`criterion`](https://crates.io/crates/criterion) and
[`gungraun`](https://crates.io/crates/gungraun) benchmarks to get
wall-clock time and instruction-level metrics.
