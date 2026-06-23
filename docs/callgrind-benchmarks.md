# Callgrind benchmarks

This document describes our strategy for adding Callgrind-based
instruction-count benchmarks alongside the existing wall-clock Criterion
benchmarks. We call them "Callgrind benchmarks" because that is honestly
what they are: each scenario runs once under [Valgrind][valgrind]'s
[Callgrind][callgrind] CPU simulator (driven by the [Gungraun][gungraun]
harness), which executes the benchmark on a *simulated* microarchitecture
and reports instructions executed and simulated cache hits at each level.
The file suffix is `_cg` (short for Callgrind) and the just recipe is
`bench-cg`.

The headline measurement is the **instruction count**, but the cache
simulation (L1 / LL / RAM hits) and the Callgrind-derived "estimated
cycles" are also stable run-to-run and used for spotting changes in memory
access patterns. See "Interpreting results" below for what each number
actually means and what it does not.

## Why two kinds of benchmarks

We use two complementary benchmark mechanisms:

* **Wall-clock benchmarks** ([Criterion][criterion], often via `par_bench` for
  multithreaded shapes) measure real-world latency on real hardware. They
  capture cache effects, branch prediction, contention, and operating system
  jitter that matter for actual users. The downside is that the numbers are
  noisy and machine-dependent.
* **Callgrind benchmarks** run each function exactly once under Valgrind's
  Callgrind CPU simulator. They produce deterministic instruction counts and
  simulated cache-hit counts that are stable run-to-run on the same
  toolchain. The downside is that the simulator is single-threaded, uses a
  fixed cache model with no out-of-order execution or prefetcher, and models
  syscalls as a fixed cost — so it cannot reproduce real contention,
  scheduling, or kernel behavior.

The two are complementary, not redundant. Criterion tells you whether a change
is observably faster or slower for a user. Callgrind counts tell you
**why** — pointing at the specific code-path delta that explains the
wall-clock difference, or at a regression that wall-clock noise might be
hiding.

The pairing rule is therefore **asymmetric**:

> Every Callgrind scenario must have an analogous Criterion scenario covering
> the same operation. The pair gives both signals: "did the wall-clock cost
> change?" and "did the instruction count change?".
>
> The reverse is not required. Criterion scenarios can legitimately exist
> without a Callgrind counterpart when the operation is dominated by
> something Callgrind cannot meaningfully model: multi-threaded contention,
> syscall behavior, allocation, scheduling, or bulk throughput where
> per-instruction resolution adds no signal.

## When to add a Callgrind benchmark

Add a Callgrind benchmark when at least one of these is true:

* The package's value proposition is **low overhead** (an allocator, a pool,
  a metrics primitive, a synchronization primitive, a thread-local cache, a
  small queue or stack). The whole point of the package is "this operation
  should take very few instructions"; a deterministic instruction count is
  the highest-fidelity way to enforce that.
* The hot path runs on **every** call from a downstream consumer (event
  observation, executor wake, channel poll, lookup table read). Any
  regression compounds across millions of calls, but may be invisible to
  Criterion because each individual call is well under a microsecond.
* The operation has **branching shape** that matters (first / last / miss,
  empty / one / many, cached / uncached, idle / dirty). Instruction counts
  make these branches legible at a glance, where Criterion runs them all
  together and reports a mean.
* The package documents performance claims in comments or README (e.g. "we
  use SIMD here", "this avoids allocation", "this is faster than `Box::pin`").
  Instruction counts pin those claims to a number.

Do **not** add a Callgrind benchmark for:

* Operations dominated by I/O, allocation, syscall, or kernel parking cost
  — the simulator models the cost as essentially-free for these. If you do
  benchmark such an operation anyway, document the scope explicitly: "this
  measures wrapper overhead, not the actual cost of the underlying syscall".
* Operations that only matter at scale (throughput, contention, scheduling
  fairness) — these need Criterion + `par_bench`, not instruction counts.
* Blocking waits or any operation that may park a thread. Callgrind
  scenarios must be deterministic and non-blocking. Wait on already-signaled
  state, poll already-ready futures, etc.
* Tests for "is this still correct" — that is what the test suite is for.

Prefer benchmarking the **public API** rather than internal helpers, since
the public API is what callers actually pay for. But this is a soft rule:
when a public API exposes large operation chains, the chain may be too big
to give a clear Callgrind signal, and benchmarking the dominant internal
step that the chain delegates to is often the only way to surface useful
deltas. If you find yourself benchmarking internals frequently, that is a
signal that the internals deserve to be factored out into their own
crate-public surface so they can be benchmarked as first-class operations.

## Scenario selection

Each `_cg.rs` file should cover **2 to 6 logical axes** of the operation
(the product of those axes may produce more than 6 measured cases). Aim for
the smallest set that catches the regressions you care about.

Use this checklist when choosing scenarios:

1. **The default case** — what almost every consumer does. Plain
   `Event::observe_once()`, `pool.insert(v)`, `pool.acquire()`, etc.
2. **Branching extremes** — for any operation with a meaningful branch (hit
   vs miss, first vs last, ready vs pending), include both endpoints.
   Include `miss` cases when applicable, because they are the most common
   regression source.
3. **State / occupancy variants** — clean vs dirty, empty vs populated,
   first-touch vs reused capacity, cached vs invalidated, idle vs updated.
   These are commonly the most regression-sensitive shapes.
4. **Size sensitivity** — when an operation's cost grows with input size (a
   bucket scan, a list walk, a registry scan), include a small and a large
   case to make the per-item cost legible.
5. **Initialization vs steady state** — if the first call differs from later
   calls (lazy init, thread-local first-touch), include both. Use Gungraun's
   `setup` parameter to bring the structure to steady state before the
   measured call.
6. **Sibling variants** — if the same logical operation has multiple
   implementations the user can pick between (sync vs local, pull vs push,
   embedded vs boxed), benchmark each one with the same scenarios so the
   numbers are directly comparable.

What **not** to do:

* Do not benchmark every method of every type. Pick the value-prop hot path
  and skip everything else.
* Do not chain multiple operations into one "realistic workflow" measurement.
  Each benchmark measures **one** operation; if you want to measure a
  sequence, decompose it into per-operation benchmarks plus one composite if
  it adds value.
* Do not measure trivial accessors, `Debug` impls, or constants. The
  compiler is allowed to optimize these to nothing and the noise floor is
  larger than the signal.
* Do not benchmark blocking waits, anything that parks a thread, or anything
  that depends on cross-thread synchronization. The simulator is
  single-threaded and the result will not mean what you think it means.
* Do not benchmark anything that allocates, locks, or syscalls in the hot
  path without first checking that the result is meaningful. The simulator
  models the allocator and the kernel as a fixed cost; comparing benches
  that allocate to benches that do not is misleading.

## Adding a Callgrind benchmark

Each Gungraun bench lives alongside the Criterion benches in its crate:

```
crates/<crate>/benches/<crate>_<name>_cg.rs
```

The `_cg.rs` filename suffix is required for `just bench-cg` discovery. Use
a crate-name prefix on the file name (e.g. `nm_observe_cg.rs`,
`fast_time_clock_cg.rs`) so the resulting bench binary does not collide
with binaries from other crates in the shared `target/.../deps/` directory.

### Cargo.toml

In `crates/<crate>/Cargo.toml`, add a target-gated dev-dependency and a
`[[bench]]` entry:

```toml
[target.'cfg(target_os = "linux")'.dev-dependencies]
gungraun = { workspace = true, features = ["default"] }

[[bench]]
name = "<crate>_<name>_cg"
harness = false
```

The `cfg(target_os = "linux")` gate keeps the Gungraun dependency out of
Windows and macOS resolution entirely — without it, `cargo-machete` and
`cargo-udeps` flag the dependency as unused on non-Linux builds. The
`[[bench]]` entry itself is **not** target-gated; bench tables in `Cargo.toml`
do not support `cfg` attributes. Instead, the bench file gates its own
contents (see next section).

### Bench file template

The Linux-only Gungraun code lives in a single `mod linux { ... }` block so
the file does not need a per-line `#[cfg(target_os = "linux")]` annotation.
A top-level `gungraun::main!(...)` invocation references the groups via the
`pub use linux::*;` re-export so the macro's simple-identifier requirement
on group names is satisfied.

```rust
//! Callgrind benchmarks for <operation> in the `<crate>` crate.
//!
//! Paired with <criterion-bench-name>.rs which covers the same operations
//! under wall-clock measurement.

#![allow(
    missing_docs,
    reason = "no need for API documentation on benchmark code"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Tracking issue drafts live at \
          c:/Source/gungraun-lint-issues/ pending upstream filing."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use gungraun::prelude::*;

    // ... benchmark fns and library_benchmark_group! calls ...
}

#[cfg(target_os = "linux")]
pub use linux::{my_group};

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = my_group
);
```

#### Why `expect` instead of `allow`?

The three lints in the `expect` block are spuriously triggered by Gungraun's
macro expansions and cannot be fixed in our code. We use `expect` rather
than `allow` so that when an upstream fix lands (in either Gungraun or
Clippy), our build immediately surfaces the now-unfulfilled expectation and
we can remove the suppression. Draft GitHub issues for each suppressed lint
live in `c:/Source/gungraun-lint-issues/<lint>/{gungraun.md, clippy.md}`,
to be filed upstream once they have been polished.

### Gungraun syntax gotchas

These are easy to get wrong on the first attempt:

* `gungraun::main!()` generates its own `fn main()`. Invoke it at file
  scope, **not** inside `mod linux`. Inside the module the generated
  function would not become the binary entry point.
* `gungraun::main!(library_benchmark_groups = ...)` accepts simple
  identifiers only, not paths. Re-export the groups at file scope with
  `pub use linux::{group_a, group_b};` so the identifiers resolve.
* `library_benchmark_group!` takes the benchmark function names as a
  bracket-less comma-separated list: `benchmarks = a, b, c` (no square
  brackets around the list).
* `#[bench::id(...)]` and `#[benches::sizes(args = [...], setup = ...)]`
  accept either named setup functions or direct value expressions. Closure
  form (`setup = || ...`) is not supported.
* Doc comments (`///`) on `#[library_benchmark]` functions are rejected.
  Use plain `//` comments instead.

### Pairing with Criterion

For each scenario in the `_cg.rs` file, an analogous Criterion benchmark
must exist in the same package's `benches/` directory. The two need not be
in the same file, and they need not use identical names — but if a future
maintainer cannot trivially identify the Criterion counterpart for a given
Callgrind scenario, the pairing has failed.

Practical guidelines:

* Use the same setup functions (or thread-local initializers) for both, so
  the measured object is in the same state. If the Callgrind scenario uses
  a fresh single-event registry, do not pair it to a Criterion scenario
  that runs against the full thread-local registry of every other benchmark
  in the same file — add a matching controlled-state Criterion scenario.
* Use the same scenario taxonomy: if the Callgrind bench has `hit_first`,
  `hit_last`, `miss`, the Criterion bench should have the same three (under
  matching or similar names).
* Document the pairing in a one-line `//!` doc comment at the top of the
  `_cg.rs` file: "Paired with `<criterion-bench-name>.rs`".
* When you add a new Callgrind scenario, add the matching Criterion
  scenario at the same time. (The reverse — adding Callgrind coverage when
  you add a Criterion scenario — is encouraged but not required; see the
  asymmetric pairing rule above.)

## Running

Callgrind benchmarks require Valgrind and run only on Linux (including
WSL on Windows).

Because each scenario runs once on a simulated CPU rather than wall-clock
time on the real machine, results are deterministic run-to-run and are
**unaffected by other load on the machine**. It is therefore safe to run
`just bench-cg` (or `just package=foo bench-cg`) on a shared workstation
at any time, including as a quick smoke test of a newly added Callgrind
benchmark. This is the headline difference from `just bench`, whose
wall-clock numbers should not be acted on when the machine is contended.

Install once:

```bash
sudo apt install -y valgrind
cargo install gungraun-runner --version 0.19.2 --locked
```

The `gungraun-runner` version must match the `gungraun` library version
pinned in the workspace `Cargo.toml` exactly — `gungraun-runner` enforces
strict string equality on the version and any drift surfaces as a
`VersionMismatch` runtime error. `just install-tools` performs the
equivalent install for you.

Then:

```bash
# Run all Callgrind benchmarks across the workspace.
just bench-cg

# Scope to a single package.
just package=foo bench-cg

# Run a specific bench file by name.
just bench-cg foo_observe_cg
```

On Windows, run the recipe via WSL from the repo root (WSL inherits the
caller's working directory):

```powershell
wsl -e bash -l -c "just bench-cg"
```

The recipe enumerates every `crates/*/benches/*_cg.rs` file and runs
each via `cargo bench -p <crate> --bench <name>`. Subsequent runs automatically
compare against the previous run's baseline in `target/gungraun/` and exit
non-zero if any regression is detected.

## Interpreting results

Each scenario reports:

* **Instructions** — instructions executed in the measured function. This is
  the headline number. Stable run-to-run on the same toolchain.
* **L1 / LL / RAM hits** — simulated cache hits at each level. Useful for
  spotting changes in memory access patterns.
* **Estimated cycles** — a Callgrind-internal weighted sum of the above
  using a fixed cost model. Useful as a rough ranking; do not over-interpret
  small differences. This is **not** a real CPU cycle count: the simulator
  has no out-of-order execution, no realistic branch predictor, no
  prefetcher, and a fixed cache geometry. Treat it as a secondary signal
  behind the instruction count.
* **Bc / Bcm** — conditional branches taken and mispredicted by Callgrind's
  simple 2-bit local predictor. A high `Bcm/Bc` ratio means the predictor
  guessed wrong frequently; on real hardware each misprediction costs
  ~15-20 cycles of pipeline flush. Useful for spotting regressions where
  instruction count is flat but a hot branch has become harder to predict
  (e.g. a state machine gaining a new common-case branch).
* **Bi / Bim** — indirect branches and mispredictions (vtable calls,
  function pointers, jump tables). Almost always 100 % mispredicted on
  cold paths, which is normal: the simulator has no call-site history.
  Care more about how this changes between runs than the absolute count.

Callgrind's predictor is a generic 2-bit local model. It does not match
any specific x86 part. The absolute counts are therefore not directly
comparable to real hardware perf counters — they are a stable proxy.

Two important caveats:

1. The cache model is fixed (typically L1: 64KiB, LL: 8MiB). It is
   comparable run-to-run but not realistic. Cache numbers indicate
   "memory access pattern changed", not "real cache misses changed".
2. The simulator is single-threaded. Contention, atomics, and memory
   ordering effects do not show up. Multithreaded behavior must be covered
   by Criterion + `par_bench`.

### Cross-validate design decisions against Criterion

Callgrind is excellent for *spotting* a delta and *attributing* it to a
specific code path, but the absolute magnitude of that delta on real
hardware is unpredictable. The simulator has no out-of-order execution, no
modern branch predictor, no prefetcher, and a fixed cache geometry — all
of which real CPUs use to absorb (or sometimes amplify) instruction-count
deltas. Two failure modes are common enough to plan for:

* **Absorbed:** Callgrind shows a worrying instruction-count increase that
  wall-clock barely registers. Branch prediction nails the new branches,
  ILP overlaps the extra arithmetic with the surrounding load latency, the
  added comparisons live entirely in registers. A +20% Callgrind delta on
  a hot path can shrink to +2% wall-clock.
* **Amplified:** Callgrind shows a small instruction-count delta but
  wall-clock shows a much larger regression. The new code introduces a
  hard-to-predict branch, a `swap_remove`+`push` pair that touches more
  cache lines than the in-place rotation it replaced, or a store-load
  dependency the OoO engine cannot hide. A +50% Callgrind delta can
  blow up to +100% or worse wall-clock.

Therefore, when a Callgrind delta is driving a *design decision* (which
data structure to use, which fast path to add, whether a regression on
one axis is acceptable in exchange for a win on another), **run the
Criterion counterpart on the same scenario before committing to the
design.** Treat Callgrind as the hypothesis generator and Criterion as
the verifier:

* If wall-clock agrees in direction and rough magnitude, the change is safe
  to ship and the Callgrind number is a fair summary.
* If wall-clock disagrees in direction, the design needs to be revisited
  — the simulated cost was misleading.
* If wall-clock confirms the direction but the magnitude differs sharply,
  document both numbers in the PR description. Reviewers should see the
  real-world cost, not just the simulated one.

This is especially important for *worst-case* / adversarial scenarios. A
typical-case Criterion churn loop will often hide a worst-case regression
because the hot path keeps the data structure in the cheap state — you
need a dedicated adversarial Criterion bench to expose what the
adversarial Callgrind scenario actually costs in cycles.

### Regression handling

Gungraun's auto-diff exits non-zero on any regression, however small. This
is a **trip wire**, not a verdict: a regression may be intentional (a new
feature that costs cycles), benign (a layout change in an unrelated type),
or a genuine bug. The pattern is:

1. Run `just bench-cg` locally before opening a PR. If there is a
   regression, decide whether to accept it.
2. If the regression is intentional, mention it in the PR description with
   the before/after numbers and the rationale.
3. If unintentional, fix it before merging.

We deliberately do **not** treat the trip wire as a CI gate today, because
performance trade-offs require human evaluation, not automated rejection.

## Baselines

Baselines live in `target/gungraun/` and are local to each developer's
machine. They are intentionally not committed.

Even though Callgrind is a deterministic simulator, the simulated program is
still real code linked against the real standard library and Rust runtime on
the host. Toolchain upgrades, distro libc updates, glibc patches, and even
inlining decisions made by the compiler on a different machine shift the
absolute instruction count by a small constant. The deltas within a single
machine remain meaningful for spotting regressions, but the absolute numbers
do not transfer cleanly between machines or even between toolchain bumps on
the same machine.

A future improvement may add a normalized baseline format that can be
committed. Until then, the human review described above is the regression
mechanism.

## Profile output

Each run writes per-scenario profiles to `target/gungraun/`. To inspect a
specific scenario's hottest functions:

```bash
callgrind_annotate target/gungraun/<package>/<bench>/<scenario>/callgrind.out
```

This produces a plain text annotated report and works fine on Windows when
invoked through WSL.

For a GUI view of the call graph, load the same file in
[KCachegrind][kcachegrind] (Linux) or its Qt-based fork QCacheGrind. On
Windows the most convenient options are:

* Install KCachegrind inside the Ubuntu WSL distro
  (`sudo apt install -y kcachegrind`) and launch it via WSLg (Windows 11)
  or an X server such as VcXsrv (Windows 10). The Callgrind output files
  in `target/gungraun/` are accessible to the Linux GUI as Windows paths
  under the WSL mount path corresponding to your checkout (e.g.
  `/mnt/c/path/to/repo/target/gungraun/...` on Windows).
* Install QCacheGrind for Windows natively from
  <https://sourceforge.net/projects/qcachegrindwin/> (an unofficial port).
  Open the same `callgrind.out` files directly from the Windows file
  system. Renders identical output to KCachegrind on Linux.

[criterion]: https://github.com/bheisler/criterion.rs
[valgrind]: https://valgrind.org/
[callgrind]: https://valgrind.org/docs/manual/cl-manual.html
[gungraun]: https://crates.io/crates/gungraun
[kcachegrind]: https://kcachegrind.github.io/
