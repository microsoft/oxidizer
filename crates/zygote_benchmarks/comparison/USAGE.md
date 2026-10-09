<!-- Copyright (c) Microsoft Corporation. Licensed under the MIT License. -->

# Paired direct-startup comparison

On Linux, run `python3 crates/zygote_benchmarks/comparison/measure.py --runs 100`
from the repository root. Requires Cargo and GNU `size`; the script uses
temporary, isolated release target directories and deletes them afterward.
Both pairs use identical source and `zygote_rt` dependencies; the wrapped
package adds the linker integration and `zygote_rt::link!()`. Each package
reports a clean build, a no-op incremental build, a real source-change
incremental rebuild, a final-link rebuild, section and executable sizes, and
direct process-startup observations without starting a controller. Isolated
targets live under `target/zygote-build-comparison` and are deleted afterward.
Clean build time includes dependency compilation.
Compare results on the same host and toolchain. No results are stored or gated.

The Linux launch benchmark also writes per-fixture native and accelerated
fault, RSS, PSS, private-dirty, descriptor, and thread observations to
`target/zygote-benchmarks/memory-observations.json`. Override the destination
with `ZYGOTE_MEMORY_REPORT_PATH`. To fail on regressions, set
`ZYGOTE_MEMORY_BASELINE_PATH` to a previous report and
`ZYGOTE_MEMORY_MAX_REGRESSION_PERCENT` to the permitted percentage increase.
These snapshots are collected outside Criterion timing loops.
