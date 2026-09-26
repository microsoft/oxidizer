# Arty I/O coordinator optimization report

Measured on 2026-09-26.

## Result

The primary-only cycle uses **47.3% fewer instructions**, and the
primary-plus-secondary cycle uses **45.3% fewer instructions**. Idle and
unpublished-work cycles improve by **66.8%** and **64.8%**, respectively.
The corresponding Criterion measurements agree with the direction of these
improvements. Steady-state cycles remain allocation-free.

The baseline is **e06608ce39c058f7cf2c28c3a4bf87813519bc0e**
(`fix(arty_io_core): retire completed work callbacks`). This includes the
behavioral changes and tests fetched during the campaign. Earlier measurements
against `300898f4` were superseded, not mixed into the final comparison.
The optimized implementation is the coordinator change accompanying this report.

Across the 27 measured cases, instruction counts improve in **21**, remain
unchanged in **5**, and increase by **one instruction** in `start_work`
(201 to 202, +0.5%). That small regression is retained alongside the much
larger full-cycle improvements rather than hidden.

## Measurement conditions

| Setting | Value |
| --- | --- |
| OS | Ubuntu 24.04 under WSL2 |
| Kernel | 6.6.87.2-microsoft-standard-WSL2 |
| Host CPU | Intel Xeon Platinum 8370C, 2.80 GHz |
| Rust | 1.96.1, commit `31fca3adb283cc9dfd56b49cdee9a96eb9c96ffd` |
| LLVM | 22.1.2 |
| Target | `x86_64-unknown-linux-gnu`, `target-cpu=x86-64-v3` |
| Build | Workspace bench profile: optimization, fat LTO, one codegen unit |
| Gungraun library and runner | 0.19.4 |
| Valgrind | 3.22.0 |
| Criterion | 100 samples, 1-second warm-up, 5-second measurement per case |

Both implementations used the same expanded benchmark source, dependency lock
file, compiler, profile, and machine. The baseline was rebuilt after the final
benchmark formatting and diagnostic-message changes. Independent repeat runs
reproduced **every instruction count on both sides**.

The benchmark target is
`crates\arty_io_core\benches\arty_io_core_coordination.rs`. Metabench runs the
same operations under Gungraun, Criterion, and allocation tracking. The original
two composite workloads are retained; additional cases isolate cycle boundaries,
token creation and completion, callback retirement, registration, and interruption
with 0, 1, 8, or 32 registrations.

Setup and output teardown are outside the measured operations. The measurements
include fixed benchmark-wrapper overhead; no estimated harness cost was
subtracted. No-op wakers isolate coordination from native notification costs.
Callgrind does not measure kernel waiting, scheduling, or real contention.
Criterion medians below are observations on this shared machine, not portable
latency guarantees; small timing differences should not be interpreted as
significant.

## Complete before/after results

Instruction change is `(after / before - 1) * 100`. Negative is better.
Scenario names omit the common `arty_io_core_coordination/` prefix.

| Scenario | Instructions before | Instructions after | Change | Median before, ns | Median after, ns |
| --- | ---: | ---: | ---: | ---: | ---: |
| cycle/idle | 750 | 249 | -66.8% | 153.63 | 48.47 |
| cycle/primary_one_secondary | 1879 | 1028 | -45.3% | 392.98 | 221.57 |
| cycle/single_primary | 1355 | 714 | -47.3% | 274.72 | 145.05 |
| cycle/unpublished_work | 1269 | 447 | -64.8% | 268.18 | 97.54 |
| operation/begin_cycle/completed | 467 | 173 | -63.0% | 96.23 | 28.31 |
| operation/begin_cycle/idle | 583 | 176 | -69.8% | 118.34 | 28.40 |
| operation/complete_cycle/completed | 266 | 171 | -35.7% | 50.17 | 27.94 |
| operation/complete_cycle/idle | 382 | 174 | -54.5% | 74.60 | 28.67 |
| operation/complete_work/interrupted | 562 | 219 | -61.0% | 111.33 | 39.00 |
| operation/complete_work/registered | 620 | 373 | -39.8% | 114.22 | 65.58 |
| operation/complete_work/unregistered | 571 | 222 | -61.1% | 113.02 | 38.58 |
| operation/drop_work | 537 | 209 | -61.1% | 114.05 | 36.42 |
| operation/drop_work/eight | 591 | 267 | -54.8% | 119.29 | 41.42 |
| operation/drop_work/registered | 559 | 235 | -58.0% | 117.58 | 37.40 |
| operation/drop_work/thirty_two | 735 | 411 | -44.1% | 150.41 | 65.08 |
| operation/is_interrupted/no | 171 | 171 | 0.0% | 27.73 | 29.09 |
| operation/is_interrupted/yes | 171 | 171 | 0.0% | 28.20 | 28.28 |
| operation/on_interrupt/first | 496 | 496 | 0.0% | 47.71 | 47.83 |
| operation/on_interrupt/interrupted | 201 | 201 | 0.0% | 31.80 | 31.56 |
| operation/on_interrupt/spare_capacity | 209 | 209 | 0.0% | 34.40 | 34.21 |
| operation/start_work | 201 | 202 | +0.5% | 43.58 | 43.84 |
| operation/wake_by_ref/eight | 408 | 378 | -7.4% | 72.76 | 60.28 |
| operation/wake_by_ref/empty | 307 | 175 | -43.0% | 65.19 | 28.54 |
| operation/wake_by_ref/interrupted | 191 | 172 | -9.9% | 43.32 | 28.97 |
| operation/wake_by_ref/one | 331 | 301 | -9.1% | 67.41 | 54.37 |
| operation/wake_by_ref/retired | 640 | 610 | -4.7% | 106.76 | 101.52 |
| operation/wake_by_ref/thirty_two | 672 | 642 | -4.5% | 109.93 | 103.82 |

Allocation counts are unchanged in every case. Only first registration allocates:
one 128-byte allocation on both sides. All other measured operations allocate
zero bytes.

Not every secondary simulator metric improves. For example, the 32-registration
wake changes from 1,620 to 1,801 estimated cycles despite fewer instructions
and a lower observed Criterion median. The simulator's cache/layout-sensitive
cycle estimate is not a hardware cycle measurement and was not the optimization
objective.

## Retained changes

- Reuse the acquired state guard across interruption, callback retirement,
  completion accounting, and cycle reset instead of repeatedly unlocking and
  reacquiring it.
- Keep the emptied registration allocation at cycle reset. Interruption has
  already drained the registrations, so another take, clear, and recycle is
  redundant.
- Avoid dispatch/recycling work when there are no registrations.
- Notify the completion condition variable only when the exclusive coordinator
  owner is actually waiting. The waiter flag and pending-work count share the
  existing mutex, including the transition into the condition-variable wait.
- Implement borrowed waking directly, avoiding the default `Wake::wake_by_ref`
  clone-and-drop of the internal standard-library `Arc`.
- Inline hot exported entry points and measured completion helpers. Separate
  blocking waits and callback dispatch from the short nonblocking paths.

The existing performables synchronization primitives, poison recovery,
saturating pending-work arithmetic, work IDs, retirement markers, and callback
dispatch order remain. Callbacks and retired-waker destruction remain outside
the state lock. No unsafe code, dependency, feature, or public signature was
added.

## Experiments not retained

These comparisons use the latest-upstream implementation unless noted.

| Candidate | Measured outcome | Decision |
| --- | --- | --- |
| Inline private state acquisition | Initial pre-upstream experiment increased both full-cycle counts | Reverted |
| Inline private registration | First allocation saves 16 instructions, but steady-state registration is unchanged and the two-driver cycle adds 2 | Reverted |
| Reverse `Vec::drain` callback iteration | Saves 21 instructions at fanout 32, but adds 41 to the primary-only cycle and 39 to the two-driver cycle | Reverted |
| Extract cold poison recovery and inline acquisition | Primary-only 714 to 760; two-driver 1028 to 1085 | Reverted |
| Assign retirement markers with `Option::filter` | Primary-only adds 1; two-driver adds 4; retirement of 32 registrations rises from 411 to 467 | Reverted |
| Explicitly inline `PendingWork::drop` | Primary-only 714 to 720; two-driver 1028 to 1045; no standalone drop improvement | Reverted |

Profiling considered both inclusive and self instruction costs. Repeated
experiments stopped producing worthwhile common-path improvements. Remaining
costs include necessary synchronization, ownership accounting, registration
retirement, callback processing, and benchmark overhead.

This is a practical stopping point, not a proof of a global instruction-count
minimum. More invasive alternatives, such as changing registration indexing,
replacing synchronization, or introducing custom memory representations, would
need separate workload evidence and a larger correctness review. They were not
introduced to chase marginal microbenchmark differences.

## Behavior verification

The latest upstream tests remain intact in behavior, including retirement of
completed/dropped work, releasing other pending work from a callback, blocking
cycle transitions, and old dispatches finishing after a new cycle starts.

Additional regressions cover duplicate/interleaved own registrations versus
peers, deferred destruction of retired wakers, retirement during an already
captured dispatch, state access after an external callback panics, and waking
the blocked owner on both final drop and final completion.

- All 32 crate tests pass on Linux and Windows.
- The 29 public integration tests also pass against the untouched upstream
  implementation with the same added test source.
- The 15 coordinator unit/integration cases pass 200 repeated runs on Linux:
  3,000 test executions.
- Package-scoped and full-workspace Linux Clippy pass for all targets and
  features with warnings denied. Windows all-target/all-feature package checking
  passes.
- Debug and release builds of `arty_io_core` pass on Linux.
- Anvil README regeneration completes without changing any generated README,
  and the Anvil spelling check passes.
- The changed Rust files pass the repository-pinned
  `nightly-2026-05-30` formatter.

Diff-scoped `cargo evaluate` reports zero errors and one deterministic warning:
`m_integration_tests` on the inline test module in `coordinator.rs`. This is a
documented scope mismatch, not an instruction to move these tests. The
[guideline](https://microsoft.github.io/rust-guidelines/guidelines/libs/resilience/#M-INTEGRATION-TESTS)
applies to tests that only touch public API; these unit tests deliberately
inspect the private state lock, poison flag, waiter flag, and pending-work count.
They remain inline without weakening assertions, exposing internals, or
suppressing the evaluator globally.

The Anvil formatting entry point could not resolve `cargo-fmt` through the local
Windows Rust SDK shim even after its setup recipe confirmed installation. The
exact pinned `rustfmt` executable was invoked directly with the repository
configuration instead. No repository tooling was changed to bypass the issue.
The Windows full-workspace Anvil Clippy attempt was blocked by missing
Spectre-mitigated MSVC libraries in an unrelated package; the equivalent
full-workspace Linux lint run passed.
Full-workspace CI, coverage, mutation testing, and contention-performance testing
were not run.

## Reproduction and artifacts

From a Linux checkout with Gungraun runner 0.19.4 and Valgrind installed:

```sh
GUNGRAUN_RUNNER="$PWD/target/coordinator-tools/bin/gungraun-runner" \
cargo bench --locked --package arty_io_core \
  --bench arty_io_core_coordination -- \
  --gungraun --criterion --allocations \
  --criterion-arg=--warm-up-time --criterion-arg=1 \
  --criterion-arg=--measurement-time --criterion-arg=5 \
  --criterion-arg=--sample-size --criterion-arg=100 \
  --output target/coordinator-after --no-baseline --timeout 10m
```

The campaign installed that runner into `target\coordinator-tools` using
`cargo install --locked --version 0.19.4 --root target/coordinator-tools gungraun-runner`,
without replacing the machine's older global runner. If the matching runner is
already on `PATH`, the `GUNGRAUN_RUNNER` assignment is unnecessary.

For the before run, use `e06608ce` with the same expanded benchmark file, without
the coordinator/cycle implementation changes, and select
`target/coordinator-before` as the output. Do not compare the old two-case
benchmark binary with the expanded benchmark binary.

Cargo runs this benchmark from its package directory. The local machine-readable
reports are therefore:

- `crates\arty_io_core\target\coordinator-before.json`
- `crates\arty_io_core\target\coordinator-after.json`
- `crates\arty_io_core\target\coordinator-before-repeat.json`
- `crates\arty_io_core\target\coordinator-after-repeat.json`

Each JSON report links its native Criterion and Callgrind artifacts under
`target\metabench\arty_io_core_coordination`. Copies of the final reports,
comparison data, and raw before/after profiles were also preserved with the
campaign's session artifacts.
