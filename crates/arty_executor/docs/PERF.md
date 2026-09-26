# Executor instruction-count campaign

Measured on 2026-09-26. The retained changes reduce instructions by **7.25% for
one spawn/complete**, **8.72% for a 10,000-task ready burst**, **9.33% for a cycle
with 32 awakened tasks**, and **13.31% for wake-queue overflow processing**.
All 21 steady-state cases remain allocation-free.

## Comparison boundary

Before: `e2dac7b800f77a33a741c4d28394828176b2795c`, containing the migrated
metabench harness and the unchanged executor implementation from
`ca3a3720afafb51d79b405aa08b3220f4a9c80c0`.

After: `52dc76a53731208564b9b0654b5b199d58224d8a`.

Both revisions use the **same benchmark source, dependencies, compiler, and
profile**. The improvements below do not include changes caused by replacing
the old benchmark harness. In particular, preliminary measurements exposed
metrics registration and queue growth inside some nominally warm cases; those
were moved into setup before either reported revision was measured.

The runtime changes are limited to:

- Swapping the empty active queue with the new-task queue instead of copying
  every task between reusable buffers.
- Borrowing the polling waker instead of creating and dropping an owned,
  reference-counted waker on every poll. Cloned wakers still use the original
  atomic ownership protocol; debug diagnostic wakers still own a counted clone.
- Visiting the wake queue's contiguous slices in FIFO order and clearing it
  once, with an empty-queue fast path.

Public APIs, cycle boundaries, reentrant registration, first-notification
ordering, duplicate-wake handling, owner notifications, per-wake metrics,
panic containment, and shutdown lifetime requirements are unchanged. No atomic
ordering, defensive check, metrics emission, or runtime dependency was removed.

## Instruction counts

Each value is the median of three independent Gungraun/Callgrind runs. Counts
are **per scenario invocation**, not per task. The common
`ae_basic_operations/` prefix is omitted. Negative changes mean fewer
instructions. [The CSV](instruction-counts.csv) retains all six raw counts.

| Scenario | Before | After | Change |
| --- | ---: | ---: | ---: |
| `basic/noop/cold` | 7,271 | 7,250 | -0.29% |
| `basic/noop/warm` | 767 | 737 | -3.91% |
| `basic/spawn_and_complete_one/warm` | 1,601 | 1,485 | -7.25% |
| `basic/yield_one/warm` | 2,812 | 2,677 | -4.80% |
| `decomposed/cycle_pending/first_poll` | 1,194 | 1,077 | -9.80% |
| `decomposed/cycle_pending/inactive` | 778 | 758 | -2.57% |
| `decomposed/cycle_ready/tasks_1` | 1,321 | 1,205 | -8.78% |
| `decomposed/cycle_ready/tasks_32` | 7,772 | 7,302 | -6.05% |
| `decomposed/cycle_woken/all_32` | 9,696 | 8,791 | -9.33% |
| `decomposed/cycle_woken/one_of_1` | 1,201 | 1,195 | -0.50% |
| `decomposed/cycle_woken/one_of_32` | 1,224 | 1,208 | -1.31% |
| `decomposed/cycle_woken/overflow` | 92,326 | 80,039 | -13.31% |
| `decomposed/poll_completed/ready` | 134 | 134 | 0.00% |
| `decomposed/task_add/cold` | 5,272 | 5,285 | +0.25% |
| `decomposed/task_add/warm` | 315 | 315 | 0.00% |
| `decomposed/wake_by_ref/delivered` | 162 | 162 | 0.00% |
| `decomposed/wake_by_ref/duplicate` | 162 | 162 | 0.00% |
| `decomposed/wake_by_ref/overflow` | 145 | 145 | 0.00% |
| `decomposed/yield_cycle/completion` | 1,366 | 1,348 | -1.32% |
| `decomposed/yield_cycle/self_wake` | 1,263 | 1,136 | -10.06% |
| `slow/spawn_and_complete_10k/burst_10000` | 5,277,393 | 4,817,301 | -8.72% |
| `slow/spawn_and_complete_one_times_many/sequential_1000` | 1,513,231 | 1,397,181 | -7.67% |
| `slow/yield_10k/burst_10000` | 8,599,600 | 7,918,665 | -7.92% |

The explicit cold registration case increases by 13 instructions; there is no
claim of improving every path. Clock-dependent branches vary by tens of
instructions between runs, so small differences, especially the single-wake
cases, should not be treated as independently established gains.

## Allocations and wall-clock observations

Both revisions allocate zero times in all 21 non-cold scenarios. The cold idle
cycle allocates 10 times / 984 bytes and cold registration allocates 10 times /
23,056 bytes in both revisions. These cold cases measure the first operation
on prepared executor state, not executor construction itself.

Criterion also uses the same operation bodies. These complementary observations
used 30 samples, a 0.3-second warm-up and a 1-second measurement window:

| Scenario | Before | After |
| --- | ---: | ---: |
| Warm idle cycle | 189.88 ns | 167.16 ns |
| One spawn/complete | 307.72 ns | 254.56 ns |
| One yield round trip | 688.95 ns | 571.44 ns |
| 1,000 sequential spawn/completes | 317.21 us | 248.78 us |
| 10,000-task ready burst | 1,194.99 us | 888.34 us |
| 10,000-task yield burst | 1,913.14 us | 1,416.27 us |

The host is shared and these timing runs are not a controlled throughput study.
They are supporting observations, not guaranteed application speedups.
Callgrind likewise does not establish contention, kernel scheduling, or
cross-thread throughput improvements.

## Reproduction

Environment: Ubuntu 24.04 under WSL2, Linux
`6.6.87.2-microsoft-standard-WSL2`, x86-64, Intel Xeon Platinum 8370C.
Rust `1.96.1` (`31fca3adb`), LLVM `22.1.2`, metabench `0.1.1`,
Gungraun and its matching runner `0.19.4`, Valgrind `3.22.0`.

Use the workspace's `bench` profile: fat LTO, one codegen unit, and
`-C target-cpu=x86-64-v3` from `.cargo/config.toml`. The report metadata calls
this profile `release`; the invocation is `cargo bench`.

On each revision, run the following three times with distinct output names:

```sh
cargo bench -p arty_executor --features test-util \
  --bench ae_basic_operations --locked -- --gungraun --output instructions-1
```

The matching `gungraun-runner` must be on `PATH`. Output paths above are relative
to the crate directory. Native profiles are under the workspace's
`target/metabench/ae_basic_operations` directory.

For one-shot allocation observations:

```sh
cargo bench -p arty_executor --features test-util \
  --bench ae_basic_operations --locked -- --allocations --test --output allocations
```

For the timing observations:

```sh
cargo bench -p arty_executor --features test-util \
  --bench ae_basic_operations --locked -- --criterion \
  --criterion-arg 'ae_basic_operations/(basic|slow)' \
  --criterion-arg --warm-up-time --criterion-arg 0.3 \
  --criterion-arg --measurement-time --criterion-arg 1 \
  --criterion-arg --sample-size --criterion-arg 30 --output timing
```

Setup primes metrics, pool reuse, and queue capacity outside decomposed
measurements. Reusable composite states complete two warm-up operations.
Teardown drops retained wakers and join handles before executor shutdown.
Overflow scenarios saturate the 1,024-entry queue with duplicate notifications,
wake another task through the fallback, and leave other tasks inactive.

## Rejected candidates and stopping point

| Candidate | Decision |
| --- | --- |
| Move pointer bit reversal from each write to hash finalization | Same instruction counts under the optimized profile; reverted. |
| Drain wakes with `pop_front()` | Improved small cases but increased overflow instructions by about 5.5%; reverted. |
| Unguarded iterator/`for_each` wake loops | Introduced fixed idle-cycle overhead; replaced with guarded, direct slice loops. |
| Normal and forced inlining of the wake helper | No instruction reduction in delivered, duplicate, or fallback wake cases; reverted. |
| Coalesce notifications or skip atomic exchanges | Would alter notifications, metrics, or synchronization; not adopted. |
| Replace the inactive set or change its hashes | Broader representation/order tradeoffs without a demonstrated need; not adopted. |

The campaign stopped after profiling the remaining work and exhausting these
concrete, low-risk candidates. The remaining costs include required metrics and
clock observations, task ownership, synchronization, and collection operations.
This is confidence in the measured local improvements, **not a proof that no
further optimization exists**.
