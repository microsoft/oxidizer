# Runtime benchmark report

**Historical baseline:** these measurements belong to the pinned revision below.
The later direct task-registration optimization, timeout workload, telemetry
handle-storage change, and lifecycle fixes are not measured by this report.
No replacement numbers are claimed; rerun the documented command on the desired
revision for a current comparison.
The historical `system` cases below measure blocking tasks. Current benchmark
identifiers use `blocking` instead; the recorded historical identifiers are unchanged.

On this Windows host, Tokio was faster for ordinary, local, and nested task
scheduling. Arty was faster for the blocking-pool cases. Timer results were
dominated by host timer granularity. These are measurements of the initial
port, not a performance guarantee or a claim that either runtime is universally faster.

## Snapshot and reproduction

- Source revision: `ddfcde60ed48ac63073ab1f7541aa0a82aabc120`.
- Arty 0.3.1, Tokio 1.53.1, metabench 0.1.1; versions resolved by that revision's lockfile.
- Measurement timestamp: `2026-09-29T17:07:47.8474566Z`.
- Windows 11 Enterprise, build 26200; Intel Xeon Platinum 8370C at 2.80 GHz;
  8 physical cores and 16 logical processors visible to the host.
- Rust 1.95.0 (`59807616e`), LLVM 22.1.2, `x86_64-pc-windows-msvc`.
- Cargo bench profile: fat LTO, one codegen unit, `-C target-cpu=x86-64-v3`.
- System allocator; Criterion timing and process-wide allocation tracking.
- Criterion: 50 samples, 0.5-second warmup, 2-second requested measurement
  window per case; Criterion may extend this for slower workloads.

```powershell
cargo +1.95 bench -p arty --features rt --bench arty_scheduling --bench arty_telemetry --locked -- --criterion --allocations --no-baseline --fail-fast
```

Both targets use `#[metabench::benchmark]` and `metabench::main!`.
Metabench writes `report.json`, `report.md`, and raw engine artifacts under
`target\metabench\arty_scheduling` and `target\metabench\arty_telemetry`.
[benchmarks.json](https://github.com/microsoft/oxidizer/blob/590799f4be8eba435cd0e1ddcd9ba31d58a6c4b5/crates/arty/docs/benchmarks.json) retains the measured values and confidence
bounds used for this report without machine-specific artifact paths.
Metabench's `UNCOMPARED` status refers to the absence of a historical baseline;
the Arty/Tokio ratios below are calculated from the paired cases in this run.

## Measurement boundary

Each case uses 1 or 4 asynchronous workers and batches of 1 or 100 tasks.
Runtime construction, reusable vectors, and the external waking thread are
outside steady-state timing. Preparation runs one batch per worker before
the measured invocation, covering every round-robin Arty worker. Tokio keeps
its own task-placement policy; equivalent preparation does not promise that
each Tokio worker sees the same tasks.

External spawn, yield, wake, timer, and blocking cases end after all task results
are observed. Both runtimes use the same external `futures::executor::block_on`
join driver. Nested/local wall-clock measurements start inside the parent
async entry; process-wide allocation measurements also include that entry
and its per-invocation setup. Result observation is not a full reclamation
barrier, so allocations around asynchronous cleanup can vary between invocations.

Arty pins its workers and preserves task affinity; Tokio's multi-thread
scheduler is not pinned and may move work. Arty local tasks run on a worker;
Tokio `LocalSet` runs on the benchmark controller. Nested Arty spawning is
worker-affine, unlike Tokio's general spawning. These API-level comparisons
include those architectural differences and are not isolated executor-cost comparisons.

Both blocking-work configurations cap their shared blocking pool at four threads,
but retain different pool growth policies. The blocking workload is intentionally
a minimal completed task, measuring submission and completion rather than I/O.
Yield uses the same benchmark-local self-waking future; there is no runtime yield API.
Remote wake uses a reusable external thread, not thread creation per task.
Timers request one millisecond, but both runtimes observed roughly 12-13 ms
on this host. No timer-resolution override or isolated-host guarantee was applied.

## Arty versus Tokio

Times are nanoseconds per completed batch. Brackets are Criterion's 95%
confidence interval for the median, not task-latency percentiles.
The ratio is Arty divided by Tokio: below 1 favors Arty.
Allocation counts and bytes are process-wide totals for one batch invocation,
not per-task rates or peak retained memory.

| Workload | Workers | Tasks | Arty ns [95% CI] | Tokio ns [95% CI] | Time ratio | Arty/Tokio allocations | Arty/Tokio bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| spawn | 1 | 1 | 2087.05 [2013.40, 2143.58] | 1903.09 [1845.64, 1964.61] | 1.10 | 5/3 | 920/272 |
| spawn | 1 | 100 | 84291.22 [83477.80, 86394.48] | 52235.13 [51506.91, 52719.88] | 1.61 | 303/102 | 79856/12944 |
| spawn | 4 | 1 | 34551.04 [31852.00, 35704.00] | 19058.14 [15226.10, 21336.19] | 1.81 | 3/1 | 776/128 |
| spawn | 4 | 100 | 258982.55 [200260.50, 321129.02] | 59802.55 [59463.22, 60399.06] | 4.33 | 304/102 | 80608/12944 |
| nested | 1 | 1 | 1086.56 [1083.38, 1098.63] | 284.28 [282.89, 285.51] | 3.82 | 9/5 | 2128/536 |
| nested | 1 | 100 | 64310.21 [63736.95, 64634.02] | 31450.14 [31206.29, 31663.64] | 2.04 | 307/102 | 82648/13856 |
| nested | 4 | 1 | 1122.18 [1099.26, 1135.85] | 572.37 [559.45, 589.14] | 1.96 | 7/3 | 1984/392 |
| nested | 4 | 100 | 64778.47 [63974.34, 65392.81] | 44401.88 [43935.63, 45011.07] | 1.46 | 307/102 | 82648/13856 |
| local | 1 | 1 | 668.34 [663.42, 674.76] | 260.83 [260.00, 262.33] | 2.56 | 7/6 | 1408/1344 |
| local | 1 | 100 | 25262.44 [24985.62, 25707.04] | 17816.12 [17753.22, 17897.17] | 1.42 | 104/106 | 6808/15832 |
| local | 4 | 1 | 669.01 [664.26, 673.39] | 261.41 [260.34, 263.91] | 2.56 | 5/6 | 1264/1344 |
| local | 4 | 100 | 25724.59 [25235.31, 26018.01] | 18007.42 [17945.08, 18131.16] | 1.43 | 104/107 | 6808/15864 |
| yield | 1 | 1 | 2666.09 [2553.46, 2777.33] | 2014.54 [1990.12, 2085.24] | 1.32 | 3/3 | 1056/272 |
| yield | 1 | 100 | 105726.69 [104661.07, 107460.89] | 68270.53 [67534.22, 69013.43] | 1.55 | 303/101 | 107856/12896 |
| yield | 4 | 1 | 34204.47 [33446.29, 36163.34] | 26503.34 [24701.94, 27833.90] | 1.29 | 3/2 | 1056/160 |
| yield | 4 | 100 | 125727.94 [110688.98, 145513.00] | 60434.14 [59973.97, 61850.69] | 2.08 | 304/102 | 108608/12944 |
| wake | 1 | 1 | 52752.60 [52219.91, 53372.22] | 55596.67 [55062.94, 56087.01] | 0.95 | 4/2 | 1152/280 |
| wake | 1 | 100 | 144387.38 [139826.92, 147954.18] | 122089.71 [121411.72, 122852.27] | 1.18 | 406/203 | 120456/31000 |
| wake | 4 | 1 | 58053.88 [56633.82, 61467.50] | 64501.11 [63786.42, 65220.14] | 0.90 | 4/2 | 1152/280 |
| wake | 4 | 100 | 134831.37 [127827.37, 140786.67] | 96467.92 [95451.26, 112076.16] | 1.40 | 408/204 | 122208/32000 |
| timer | 1 | 1 | 13230062.50 [11973025.00, 13868637.50] | 13144012.50 [12711300.00, 13539862.50] | 1.01 | 4/1 | 1600/256 |
| timer | 1 | 100 | 12407550.00 [11893337.50, 13244025.00] | 12855812.50 [12433112.50, 13223912.50] | 0.97 | 322/100 | 125800/25600 |
| timer | 4 | 1 | 12856850.00 [11830950.00, 13439562.50] | 13017087.50 [12546062.50, 13501025.00] | 0.99 | 4/1 | 1600/256 |
| timer | 4 | 100 | 13016283.33 [11723600.00, 13926100.00] | 12931012.50 [11648700.00, 13149425.00] | 1.01 | 324/100 | 127296/25600 |
| system | 1 | 1 | 1486.70 [1422.84, 1534.86] | 5922.05 [1779.28, 10233.04] | 0.25 | 2/1 | 48/256 |
| system | 1 | 100 | 69348.54 [67622.12, 70811.05] | 172660.56 [168651.06, 177978.33] | 0.40 | 209/100 | 7227/25600 |
| system | 4 | 1 | 1477.08 [1408.38, 1527.10] | 8457.25 [4906.22, 9759.99] | 0.17 | 2/1 | 48/256 |
| system | 4 | 100 | 66773.42 [65831.47, 68244.12] | 162151.63 [154893.27, 166481.82] | 0.41 | 204/100 | 7808/25600 |

## Telemetry overhead

These are Arty-only comparisons. The active processor synchronously reads
and redacts every field/enrichment, then discards it; it does not buffer or
export events. This is not equivalent to measuring a production exporter.
Lifecycle cases include construction and complete shutdown. Spawn cases
exclude runtime construction and observe task results.

| Case | Median ns [95% CI] | Allocations | Allocated bytes |
| --- | ---: | ---: | ---: |
| lifecycle/active/one | 1138379.37 [1036590.48, 1601161.11] | 1386 | 217921 |
| lifecycle/noop | 1210032.80 [1059100.00, 1718628.57] | 1362 | 218020 |
| spawn/active/hundred | 93994.95 [92038.67, 95548.04] | 404 | 83856 |
| spawn/active/one | 5939.96 [5494.60, 6665.54] | 5 | 816 |
| spawn/noop/hundred | 84930.29 [83823.00, 86329.17] | 304 | 81456 |
| spawn/noop/one | 2251.42 [2164.36, 2366.79] | 4 | 792 |

The apparent lifecycle advantage with an active sink is not evidence that
telemetry improves startup: these cases are sensitive to thread scheduling
and allocation timing. Compare confidence bounds and repeat on the target host.

## Limits

No performance acceptance threshold was specified. This report records both
wins and regressions, without treating metabench's default historical-regression
threshold as a requirement. Reproduce on the intended deployment hardware
before choosing worker counts or making optimization claims.

The earlier diagnostic run warmed only one batch. It showed first-use allocations
on subsequent round-robin workers and was superseded by the preparation fix in
the measured revision. Neither run measures async I/O, metadata, fan-out,
work stealing equivalence, or an isolated single-threaded instruction cost.
Linux perf, Gungraun, and VTune were not run.
