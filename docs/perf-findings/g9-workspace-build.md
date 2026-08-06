# g9-workspace-build findings

## Summary

Scope: everything ABOVE individual crate source — Cargo profiles and `.cargo/config.toml`,
`[workspace.dependencies]` and `Cargo.lock`, feature-flag architecture, benchmark/profiling
infrastructure (`justfile`, `justfiles/*.just`, `.github/workflows/`), and workspace-structure
effects on cross-crate inlining.

Environment constraint: this container has no egress to `index.crates.io` / `static.crates.io`,
no cargo registry cache and no prebuilt `target/`. `cargo metadata --offline` exits 101 with
`error: no matching package named 'tokio' found ... location searched: crates.io index`.
Therefore **no build, test, benchmark, `cargo tree --duplicates` or `just` recipe could be run**.
Every finding below is derived from direct file reads, `Cargo.lock` parsing and shell censuses
over the checkout. Findings are labelled `empirically verified` when they rest on exact file
content / lockfile parsing, and `inferred from code reading` when they rest on reasoning about
what that content implies at runtime. **No performance number in this document was measured.**

Headline conclusion: the workspace's measurement apparatus is systematically biased, and the
bias points in the direction that will cause correct optimisations to be rejected. Benchmarks
are built with `lto = "fat"` + `codegen-units = 1` + `--all-features` + `target-cpu=x86-64-v3`,
none of which any consumer gets; the documented verification command for `#[inline]` decisions
(`just package=<crate> bench-cg`) **does not exist**; and no benchmark is ever executed in CI.
Meanwhile the three highest-fan-in internal crates (`ohno` 9 dependents, `thread_aware` 8,
`tick` 7) carry **zero** `#[inline]` annotations while the release profile leaves `lto = false`.

Counts of findings: 26 total — 5 High, 11 Medium, 10 Low.

Cross-cutting warning for sibling workers and the report author: **every Criterion and Callgrind
number that exists in this repository was produced under whole-program fat LTO, a single codegen
unit, all features enabled, `-C target-cpu=x86-64-v3`, and — in 19 bench files — a non-default
global allocator.** Any per-crate finding that cites a benchmark result inherits all five biases.

---

## Cargo profiles

### F1. `[profile.bench]` diverges from `[profile.release]`, making benchmarks blind to missing `#[inline]`

- **Location:** `Cargo.toml:340-341` (`[profile.release]`), `Cargo.toml:343-346` (`[profile.bench]`)
- **Issue:** `[profile.release]` sets only `debug = "line-tables-only"`. It does not set `lto`
  or `codegen-units`, so Cargo's defaults apply: `lto = false`, `codegen-units = 16`.
  `[profile.bench]` — introduced by the comment `# Best perf possible for benchmarks` at
  `Cargo.toml:343` — sets `lto = "fat"` (`Cargo.toml:345`) and `codegen-units = 1`
  (`Cargo.toml:346`). Every Criterion and Callgrind measurement in this repository is therefore
  produced by a whole-program, single-codegen-unit build that no consumer of these crates ever
  receives. Note the precision that matters here: profile settings in a workspace `Cargo.toml`
  apply only to builds *of this workspace*. A downstream consumer compiles these crates under
  *their* `release` profile, which by default is `lto = false` / `codegen-units = 16`. So the
  divergence is not "benchmarks are a bit optimistic"; it is a categorical difference in whether
  cross-crate inlining happens at all.
- **Impact:** High — the damaging consequence is not optimistic absolute numbers. It is that
  under fat LTO, LLVM can inline across crate boundaries **regardless of whether `#[inline]` is
  present**, because the whole program is in one module. A missing `#[inline]` on a small public
  function in `ohno` or `tick` is invisible to the benchmark and catastrophic in a consumer's
  no-LTO release build. This is exactly the defect `docs/performance.md:18-30` exists to prevent:
  rule 1 (`docs/performance.md:18-23`) instructs authors to add `#[inline]` to small public
  functions precisely because they cannot otherwise be inlined across a crate boundary.
  `docs/performance.md:29` then mandates verifying the annotation with
  `just package=<crate> bench-cg`. Under `[profile.bench]`'s fat LTO, that verification will
  reliably report "the annotation makes no difference", and a diligent contributor following the
  documented loop will therefore **revert correct `#[inline]` additions**. The measurement
  apparatus is biased against the workspace's own stated rule.
- **Remediation:** Either (a) make `[profile.bench]` inherit release semantics for the two
  settings that govern cross-crate inlining — drop `lto = "fat"` and `codegen-units = 1` so
  benchmarks measure what consumers get; or (b) if a fat-LTO bench profile is wanted for
  comparing algorithmic changes, keep it but add a second, release-faithful bench configuration
  and make `docs/performance.md:18-30`'s `#[inline]` verification loop use *that* one, and state
  explicitly in `docs/benchmarks.md` that fat-LTO numbers cannot be used to evaluate `#[inline]`.
  Option (b) is the more surgical of the two and matches the house preference for surgical over
  architectural change. Whichever is chosen, `docs/performance.md` must say so, because today the
  doc's verification instruction is silently unsound.
- **Evidence:** empirically verified (read `Cargo.toml:340-346` directly; confirmed by grep that
  no other `[profile.release]` or `[profile.bench]` table exists anywhere in the workspace, and
  that no crate manifest contains a `[profile]` table). The consequence for `#[inline]` visibility
  is inferred from code reading plus documented Cargo/LLVM semantics.

### F2. No `[profile.dev]` and no `[profile.*.package.*]` overrides anywhere

- **Location:** `Cargo.toml:340-366` (the complete set of profile tables)
- **Issue:** The workspace defines `release` (340), `bench` (344), `test` (351), `mutants` (355)
  and `fuzz` (362). There is **no `[profile.dev]`**, so development builds run at `opt-level = 0`
  with `debug-assertions = true` and `overflow-checks = true`. There are also **no
  `[profile.*.package.*]` per-package overrides** of any kind in the workspace.
- **Impact:** Low — this is a defensible default, and for a library workspace an unoptimised dev
  profile is normal. The observation is recorded because the absence of per-package overrides
  means there is no mechanism today to, for example, build `syn`/`prettyplease`/`prost-build` at
  `opt-level = 2` in dev builds to speed up the proc-macro-heavy compile (see F24). Adding
  `[profile.dev.package."*"] opt-level = 1` or targeted overrides is the standard ecosystem
  remedy for slow proc-macro-heavy dev builds and is currently unused.
- **Remediation:** Consider `[profile.dev.package.<proc-macro-dep>] opt-level = 2` for the heavy
  code-generation dependencies if developer build times are a concern. Purely a build-time,
  not runtime, matter — deprioritise relative to F1.
- **Evidence:** empirically verified (full read of `Cargo.toml:340-366`; grep for `[profile.` and
  for `.package.` across all `Cargo.toml` files in the workspace found no other occurrences).

### F3. `[profile.test]` sets `debug = "full"`; `[profile.mutants]` and `[profile.fuzz]` are coherent

- **Location:** `Cargo.toml:351-352` (`[profile.test]`, `debug = "full"`), `Cargo.toml:355-360`
  (`[profile.mutants]`, inherits `test`, `debug = "none"`), `Cargo.toml:362-366` (`[profile.fuzz]`,
  inherits `dev`, `opt-level = 3`, `incremental = false`, `codegen-units = 1`)
- **Issue:** These are all well-chosen. `debug = "full"` on `test` is annotated as being required
  for `cargo-llvm-cov` line attribution; `mutants` correctly turns debug info back off (mutation
  testing recompiles constantly and does not need symbols); `fuzz` correctly raises `opt-level`
  to 3 and pins a single codegen unit with incremental off, which is the standard configuration
  for fuzzing throughput.
- **Impact:** Low — recorded as a positive. No action.
- **Remediation:** None.
- **Evidence:** empirically verified (direct read of `Cargo.toml:351-366`).

### F4. `-C target-cpu=x86-64-v3` is applied to workspace builds but never reaches consumers, and covers no ARM target

- **Location:** `.cargo/config.toml:1-7` (the entire file)
- **Issue:** The file is seven lines. It sets
  `rustflags = ["-C", "target-cpu=x86-64-v3"]` for exactly two targets,
  `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`. There is no `[build]` section and no
  `aarch64-*` entry. Two consequences:
  1. `x86-64-v3` enables AVX2, BMI1/2, FMA, LZCNT, MOVBE. Published crates are compiled by the
     consumer without this flag, so any benchmark measuring code that autovectorises will report
     throughput that the consumer's baseline-`x86-64` build cannot reach. This compounds with F1:
     benchmark numbers differ from consumer reality on *both* the LTO axis and the ISA axis.
  2. CI matrices include `ubuntu-24.04-arm` and `windows-11-arm` runners, which get no
     corresponding rustflags entry. That is correct in the sense that no wrong flag is applied,
     but it means the x86 and ARM CI legs are compiled with materially different optimisation
     opportunities and any cross-architecture comparison is invalid.
  Note also that `.cargo/config.toml` rustflags are *not* additive with `RUSTFLAGS` from the
  environment: setting `RUSTFLAGS` in a shell silently discards `target-cpu=x86-64-v3`, so a
  contributor who exports `RUSTFLAGS` for any reason gets a quietly different build.
- **Impact:** Medium — it does not make the workspace slower; it makes the workspace's own
  numbers unrepresentative, and it is a third independent reason (with F1 and F6) that a
  benchmark delta observed here may not exist for a user.
- **Remediation:** Document in `docs/benchmarks.md` that all local and CI numbers are
  `x86-64-v3`, so readers can discount ISA-sensitive results. If consumer-representative numbers
  are wanted, add a documented way to run benchmarks without the flag. Do not remove the flag
  from ordinary builds — it legitimately speeds up the test suite.
- **Evidence:** empirically verified (full read of `.cargo/config.toml`; grep for `RUSTFLAGS`
  across `justfiles/` and `.github/workflows/`). The ISA-sensitivity consequence is inferred from
  code reading.

### F5. Duplicate-version policy is non-blocking from both directions

- **Location:** `Cargo.toml:329` (`clippy.multiple_crate_versions = "allow"`), `deny.toml:56`
  (`[bans] multiple-versions = "warn"`)
- **Issue:** The workspace lint table explicitly *allows* clippy's `multiple_crate_versions`, and
  `cargo-deny`'s `[bans] multiple-versions` is set to `warn` rather than `deny`. Neither
  mechanism can fail a build. The result is visible in the lockfile: 36 crate names appear at
  more than one version (see F12).
- **Impact:** Medium — duplicate versions cost binary size, compile time, and — where the
  duplicated crate is a data structure appearing in a public type — can force conversions at
  boundaries. The specific instances that matter are enumerated in F12/F13/F14. The policy
  finding is that nothing prevents the count from growing.
- **Remediation:** Consider promoting `deny.toml`'s `multiple-versions` to `deny` with an
  explicit `skip` list for the duplicates that are genuinely unavoidable (transitive `windows-sys`
  generations, `syn` 2-vs-3 during the ecosystem transition). That converts an unbounded drift
  into a reviewed, enumerated set. This is a policy change, not a behavioural one.
- **Evidence:** empirically verified (direct reads of `Cargo.toml:329` and `deny.toml:56`).

### F6. Benchmarks are compiled with `--all-features`, contaminating the workspace's lowest-overhead primitives

- **Location:** `justfiles/anvil/checks/bench.just:13` (recipe `anvil-bench`),
  `justfiles/anvil/checks/bench.just:16` (`cargo bench <scope> --all-features --no-run`);
  also `justfiles/basic.just:45,56,59` (`build`, `check`, `clippy` all use
  `--all-features --all-targets`) and `justfiles/basic.just:214-222` (`test-more`, which uses
  `--all-features --locked --tests --benches`)
- **Issue:** The only way benchmarks are ever compiled in this repository is with
  `--all-features`. That turns on every optional, default-off, performance-relevant feature in
  every crate simultaneously. Concretely verified instances:
  - **`multitude/stats` and `plurality/stats`** — runtime counters. Gating verified at
    `crates/multitude/src/lib.rs:487`, `crates/multitude/src/lib.rs:528`,
    `crates/multitude/src/internal/chunk_mutator.rs:94,542,564,587`,
    `crates/multitude/src/internal/chunk_provider.rs:23,71,79,88`;
    `crates/plurality/src/lib.rs:125,137`, `crates/plurality/src/pool.rs:28,100,217,906`,
    `crates/plurality/src/builder.rs:124`. These are exactly the crates whose Callgrind
    instruction counts are supposed to protect the workspace's cheapest primitives, and the
    counts include increments no consumer executes.
  - **`seatbelt/metrics` (OpenTelemetry) and `seatbelt/logs`** — 69 and 51 `cfg` sites
    respectively. The sharpest case:
    `crates/seatbelt/src/breaker/engine/engine_telemetry.rs:38` carries
    `#[cfg(not(any(feature = "metrics", feature = "logs", test)))]` — a deliberately zero-cost
    no-telemetry path. Under `--all-features` **that path is never the one benchmarked.** The
    crate's cheapest configuration has no measurement at all.
  - **`cachet/logs`** — 19 `cfg` sites. Several cachet bench targets go further and *require*
    it: `crates/cachet/Cargo.toml:92,106,110,118,122,126,166,170` set
    `required-features = ["logs", ...]`. The main cache benchmark cannot run without tracing
    enabled.
  - **`tick/test-util`** — the most serious of the set. `crates/tick/src/state.rs:13-17` makes
    `ClockState` a **two-variant** enum (`System` plus `ClockControl`) when `test-util` is on,
    versus a single-variant enum otherwise. A single-variant enum has no discriminant and its
    match compiles away; a two-variant enum adds a discriminant load and a real branch to every
    clock access. `crates/tick/src/clock.rs:17` documents "zero-cost overhead in production".
    `crates/tick/benches/clock_bench.rs` (target declared at `crates/tick/Cargo.toml:100-102`)
    does not declare `required-features`, but under `--all-features` it is compiled *with*
    `test-util` — so the one benchmark of the workspace's clock abstraction measures the
    non-production shape of the type whose entire selling point is that the production shape is
    free.
  - **`bytesbuf/test-util`** — 8 of 10 bench targets set `required-features = ["test-util"]`
    (`crates/bytesbuf/Cargo.toml:72,77,87,92,97,106,110,114`). This one appears benign:
    bytesbuf's `test-util` adds `pub mod testing` at `crates/bytesbuf/src/mem/mod.rs:61-62` and
    does not change the shape of any hot type. Recorded for completeness.
  Workspace-wide there are 157 `feature = "test-util"` `cfg` sites across 32 source files.
- **Impact:** High — every instruction count and wall-clock number for `multitude`, `plurality`,
  `seatbelt`, `cachet` and `tick` includes work that a default-feature consumer does not perform.
  For `tick` specifically the benchmarked type is structurally different from the shipped one.
  These are the crates the workspace most cares about being fast.
- **Remediation:** Benchmark the default feature set, not `--all-features`. The most surgical
  form: change the bench recipe's default scope to `--features <the crate's default set>` (or
  simply drop `--all-features`) and add explicit `required-features` to the small number of bench
  targets that genuinely need an optional feature — cachet's already declare theirs, so they
  would continue to be skipped-or-selected explicitly rather than universally enabled. Separately,
  add a benchmark for `seatbelt`'s no-telemetry path (`engine_telemetry.rs:38`) and for `tick`
  without `test-util`, since those are the configurations consumers ship.
- **Evidence:** empirically verified (all cited `cfg` sites and `required-features` lines read
  directly; the `--all-features` flag read at `justfiles/anvil/checks/bench.just:16`). The runtime
  cost consequences are inferred from code reading.
- **Philosophy note:** none — this is aligned with `docs/performance.md`'s concern that
  measurements reflect what users actually run.

---

## Dependencies and features

### F7. `just bench-cg` and `just bench` do not exist; nine documentation references point at nothing

- **Location:** recipe inventory across `justfile`, `justfiles/*.just`, `justfiles/anvil/**`;
  references at `docs/callgrind-benchmarks.md:11,153,300,322,325,328,335,430`,
  `docs/callgrind-benchmarks.md:302` (`just bench`), `docs/performance.md:29`
- **Issue:** Enumerating every recipe defined in `justfile`, every `justfiles/*.just` and every
  file under `justfiles/anvil/` yields exactly one benchmark-related recipe: `anvil-bench`
  (`justfiles/anvil/checks/bench.just:13`). There is no `bench` recipe and no `bench-cg` recipe.
  `grep -rn "bench-cg"` across the repository returns **only documentation hits** — eight in
  `docs/callgrind-benchmarks.md` and one in `docs/performance.md`. A reader following
  `docs/performance.md:29` ("verify with `just package=<crate> bench-cg`") receives a
  `just` "unknown recipe" error.
- **Impact:** High — this is the load-bearing instruction in the workspace's own performance
  methodology. `docs/performance.md:18-30` tells contributors to add `#[inline]` and verify;
  `docs/callgrind-benchmarks.md` builds an entire Callgrind/Gungraun chapter on top of a command
  that errors out. The practical result is that nobody runs the verification at all, which in
  turn means F1's LTO bias has never been noticed. The two findings sustain each other.
- **Remediation:** Add the missing recipes (a `bench` recipe running `cargo bench` for the scope,
  and a `bench-cg` recipe running the `*_cg` targets under `gungraun-runner`), or amend the docs
  to give the literal `cargo bench` invocations. Adding the recipes is preferable, because
  `justfiles/setup.just:53-58` already installs the tooling those recipes would need.
- **Evidence:** empirically verified (recipe inventory by reading every `.just` file;
  `grep -rn "bench-cg" .` returning only the nine documentation lines cited).

### F8. Nothing installs a `#[global_allocator]` in library code, but 19 bench files do — and inconsistently

- **Location:** `crates/multitude/benches/criterion_arena_vs_allocator.rs:41-42` (mimalloc),
  `crates/plurality/benches/graph_churn.rs:27` (mimalloc), plus 16 files installing
  `alloc_tracker::Allocator<System>` (bytesbuf ×4, `crates/cachet/benches/dynamic.rs`,
  fetch ×2, http_extensions ×2, `crates/layered/benches/dynamic.rs`,
  `crates/multitude/benches/multitude_record_batch.rs`, seatbelt ×5), and
  `crates/internity/benches/internity_mem.rs` (a bespoke `Tracking` allocator)
- **Issue:** No library crate in the workspace sets a `#[global_allocator]` — consumers get the
  system allocator, which is correct. But 19 benchmark binaries replace it:
  - **mimalloc in 2 files only.** `multitude` has 15 bench files; exactly one uses mimalloc.
    `plurality` has 4 bench targets; exactly one uses mimalloc. Both uses are documented in the
    file, but it means those two benchmarks' numbers are not comparable with their 13 and 3
    sibling targets in the same crate. `docs/benchmarks.md:48-49` asks for benchmarks that
    isolate elementary operations; silently varying the allocator between targets in the same
    crate cuts against that.
  - **`alloc_tracker::Allocator<System>` in 16 files.** This wraps every allocation with a
    counter. It is a *wall-clock* measurement perturbation on every one of those binaries.
  - **A Criterion/Callgrind pair with mismatched allocators.**
    `crates/multitude/benches/multitude_record_batch.rs` installs `alloc_tracker`; its Callgrind
    counterpart `multitude_record_batch_cg.rs` does not. `docs/callgrind-benchmarks.md:277-281`
    requires the paired files to share their setup, precisely so the two measurements describe
    the same program.
  `mimalloc` is a dev-dependency of `multitude` and `plurality` only, so it does not reach
  published artifacts.
- **Impact:** Medium — it does not affect shipped code at all. It affects the trustworthiness of
  within-crate benchmark comparisons and of the one mismatched Criterion/Callgrind pair.
- **Remediation:** Adopt one allocator convention per crate and state it in `docs/benchmarks.md`.
  At minimum, fix the `multitude_record_batch` pair so both halves use the same allocator.
- **Evidence:** empirically verified (grep for `#[global_allocator]` across `crates/*/benches/`,
  then reading each hit).

### F9. `cachet` ships `futures-executor` in its runtime dependency graph for test-only use

- **Location:** `crates/cachet/Cargo.toml:59`
  (`futures = { workspace = true, features = ["async-await", "executor"] }`, in `[dependencies]`)
- **Issue:** The `executor` feature is requested in the *runtime* dependency table, so
  `futures-executor` is part of the published crate's dependency graph and is compiled into
  every consumer build of `cachet`. Auditing the actual uses:
  - The only non-test use of `futures` in `cachet/src` is `futures::join`, at
    `crates/cachet/src/fallback.rs:14`. `join` comes from `async-await`, not `executor`.
  - Every `futures::executor::block_on` call site is inside a `#[cfg(test)] mod` — verified at
    `crates/cachet/src/refresh.rs:325` (the enclosing `#[cfg(test)]` was read to confirm), and
    the same pattern at `crates/cachet/src/cache.rs:881` and
    `crates/cachet/src/telemetry/cache.rs:439` and following.
- **Impact:** Medium — compile time and dependency-graph surface for every consumer of `cachet`,
  for zero runtime benefit. It is not a hot-path cost, but it is a gratuitous transitive
  dependency in a crate that is otherwise carefully minimal.
- **Remediation:** Drop `"executor"` from the `[dependencies]` entry and add
  `futures = { workspace = true, features = ["executor"] }` to `[dev-dependencies]` instead.
  This is a manifest-only change with no behavioural effect. (Reported, not applied — this task
  is analysis-only.)
- **Evidence:** empirically verified (read `crates/cachet/Cargo.toml:59`; grepped every
  `futures::` use in `crates/cachet/src` and read the enclosing `cfg` attribute of each
  `block_on` site).
### F10. 36 duplicated crate names in `Cargo.lock`, including four `hashbrown` generations

- **Location:** `Cargo.lock` (541 `[[package]]` entries)
- **Issue:** Parsing the lockfile directly (the only option offline) gives 541 packages, of which
  36 names appear at more than one version:
  `allocator-api2` [0.2.21, 0.4.0] · `base64` [0.22.1, 0.23.0] · `bitflags` [1.3.2, 2.13.0] ·
  `deadpool` [0.12.3, 0.13.0] · `deadpool-runtime` · `embedded-io` · `foldhash` [0.1.5, 0.2.0] ·
  `getrandom` [0.2.17, 0.3.4, 0.4.3] · **`hashbrown` [0.14.5, 0.15.5, 0.16.1, 0.17.1]** ·
  `itertools` [0.13, 0.14, 0.15] · `logos` / `logos-codegen` / `logos-derive` ·
  `matchit` [0.8.4, 0.9.2] · **`ohno` [0.3.5, 0.3.9]** · `ohno_macros` [0.3.3, 0.3.5] ·
  `phf_shared` · `prettyplease` [0.2.37, 0.3.0] · `proc-macro-crate` [2.0.2, 3.5.0] · `r-efi` ·
  `rand` [0.9.4, 0.10.1] · `rand_core` · **`syn` [2.0.118, 3.0.3]** · `toml_datetime` ·
  `toml_edit` · `windows-sys` [0.52.0, 0.60.2, 0.61.2] · `windows-targets` + eight `windows_*`
  target crates · `winnow` [0.5.40, 1.0.3].
  Attributions established by building a reverse-dependency map from the lockfile:
  - `hashbrown` 0.14.5 ← `dashmap` 6.2.1 (a **runtime** dependency, reached via `uniflight`) and
    `lasso`; 0.15.5 ← `chumsky` / `petgraph` / `symbol_table` (benchmark dependencies);
    0.16.1 ← `string-interner`; 0.17.1 ← `indexmap`, `internity`, `multitude`. Four generations of
    the workspace's most performance-relevant map implementation are linked into one build.
  - `matchit` 0.8.4 ← `axum` 0.8.9 versus 0.9.2 ← `routerama` and `rest_over_grpc_tests` — **two
    independent router trie implementations are linked into the same binary**, one of them
    (`routerama`) being a crate this workspace wrote specifically to be a fast router.
  - `syn` 2.0.118 is used by 46 proc-macro crates; `syn` 3.0.3 by 15, including **all eight
    workspace proc-macro crates**. Both are compiled on every clean build, as are
    `prettyplease` 0.2 and 0.3. This is a compile-time, not runtime, cost.
- **Impact:** Medium — binary size and compile time mostly; the `matchit` and `hashbrown`
  duplications are the two with plausible runtime relevance (cache pressure from multiple
  monomorphised map/trie implementations). Nothing here is a hot-path defect.
- **Remediation:** Track the small set that can actually be unified — `matchit` (align `axum` or
  accept it), `hashbrown` (the 0.14.5 arrival via `dashmap` is the one on the runtime path and is
  worth chasing), `base64`, `bitflags` — and encode the remainder in a `deny.toml` skip list per
  F5. The `syn`/`prettyplease` split will resolve itself as the ecosystem migrates.
- **Evidence:** empirically verified (`Cargo.lock` parsed directly; reverse-dependency map built
  from the `dependencies` lists of every `[[package]]` entry).

### F11. `ohno` is duplicated in the graph via the published `cpulist` crate

- **Location:** `Cargo.lock` — `ohno` 0.3.5 and `ohno` 0.3.9; `ohno_macros` 0.3.3 and 0.3.5
- **Issue:** Fourteen workspace crates depend on the workspace's own `ohno` at 0.3.9. A second
  copy, `ohno` 0.3.5, enters the graph through the *published* `cpulist` 1.1.4 crate (reached via
  `many_cpus_impl`). So the workspace links two versions of its own error type, with two versions
  of its derive macro. Error types from the two copies are distinct types and cannot interoperate.
- **Impact:** Low — `ohno` is a small crate and errors are, by house philosophy, off the hot path.
  The cost is binary size and the conceptual hazard of two incompatible `ohno::Error` types.
- **Remediation:** If `cpulist` is maintained by the same organisation, bump its `ohno`
  requirement. Otherwise accept and record it in the `deny.toml` skip list.
- **Evidence:** empirically verified (lockfile reverse-dependency map).

### F12. Five different hashers are linked with no workspace-level policy

- **Location:** `Cargo.lock`; `[workspace.dependencies]` at `Cargo.toml:52-240`
- **Issue:** The graph contains `ahash` (via `uniflight`), `rapidhash`, `xxhash-rust` and
  `rustc-hash` — **all three of the latter inside `data_privacy` alone** — plus `rustc-hash` in
  `internity` and `foldhash` in `cachet_memory` (and `foldhash` at two versions, 0.1.5 and 0.2.0).
  Nothing in `[workspace.dependencies]` or in any documentation states a preferred hasher, so each
  crate author chose independently. For a workspace whose stated concern is per-instruction
  overhead in map-heavy primitives, the hasher is one of the highest-leverage single choices, and
  it is being made ad hoc.
- **Impact:** Medium — three hashers in one crate (`data_privacy`) is the clearest smell; it
  suggests at least two of them are vestigial. Workspace-wide it is binary size plus a missed
  opportunity to make one considered decision instead of five unconsidered ones.
- **Remediation:** Pick a default hasher, record the decision and its rationale (the house rule at
  `docs/performance.md:86-106` requires justifying deviations from ecosystem defaults — this is
  the mirror case, where there is no default to deviate from), and have crates opt out explicitly
  with a comment. Start by auditing `data_privacy`'s three.
- **Evidence:** empirically verified (lockfile parsing plus reading the `[dependencies]` tables of
  the named crates).

### F13. `moka` reaches consumers through `cachet`'s **default** `memory` feature

- **Location:** `crates/cachet/Cargo.toml` (`default = ["memory"]`), `crates/cachet_memory`'s
  dependency on `moka` 0.12.15; transitive set from `Cargo.lock`
- **Issue:** `cachet`'s default feature set includes `memory`, which brings `cachet_memory`,
  which brings `moka` 0.12.15, which brings `async-lock`, `crossbeam-channel`, `crossbeam-epoch`,
  `crossbeam-utils`, `event-listener`, `futures-util`, `parking_lot`, `portable-atomic`,
  `smallvec`, `tagptr` and **`uuid`**. A consumer who writes `cachet = "..."` and only wants the
  cache traits gets all of it. `uuid` in particular is a surprising member of a cache's default
  graph.
- **Impact:** Medium — dependency-graph weight and compile time on the default path.
  `moka` itself is a reasonable choice for a concurrent cache; the finding is about it being
  *default-on* rather than about the choice.
- **Remediation:** Consider making `memory` non-default so the base `cachet` crate is just the
  abstraction, with `cachet = { features = ["memory"] }` as the documented common case. This is a
  breaking-ish API-surface change and should be weighed against convenience; recording it as an
  option, not a recommendation.
- **Evidence:** empirically verified (`crates/cachet/Cargo.toml` default features read directly;
  the transitive set enumerated from `Cargo.lock`).

### F14. `[workspace.dependencies]` correctly enforces `default-features = false`, and CI checks it

- **Location:** `Cargo.toml:44-50` (the header comment mandating it), `Cargo.toml:52-240` (the
  table), `.github/workflows/main.yml:186`,
  `justfiles/anvil/checks/ensure-no-default-features.just`
- **Issue:** None — this is a positive finding worth recording because it is the single most
  effective dependency-weight control a workspace can have, and it is both stated and *enforced*.
  The comment block at `Cargo.toml:44-50` mandates `default-features = false` on every workspace
  dependency; spot-checking the table confirms it is honoured; and there is a dedicated CI check
  (`ensure-no-default-features`) wired into `main.yml:186` that would catch a regression.
- **Impact:** Low (positive) — no action.
- **Remediation:** None. Preserve it.
- **Evidence:** empirically verified (read `Cargo.toml:44-50` and scanned the full table; read the
  check recipe and its `main.yml` wiring).

---

## Feature-flag architecture

### F15. Features are additive and default-off; defaults are conservative

- **Location:** the `[features]` table of each of the 53 crate manifests
- **Issue:** None structurally. Census of default feature sets:
  `bytesbuf` `["std"]` · `cachet` `["memory"]` · `data_privacy` `["serde"]` ·
  `http_path_template` `["std"]` · `internity` `["std"]` · `multitude` `["std"]` ·
  `plurality` `["std"]` · `rest_over_grpc` `["serving"]` · `routerama` `["query"]` ·
  `routerama_build` `["codegen"]` · `templated_uri` `["uuid"]` ·
  `thread_aware` `["std", "derive"]`; and `seatbelt`, `fetch`, `layered`, `tick`, `anyspawn`,
  `bytesbuf_io`, `http_extensions` all default to `[]`. Crucially, the performance-relevant
  observability features — `stats`, `logs`, `metrics`, `telemetry` — are default-*off*
  everywhere. That is exactly right, and it is what makes F6 (benchmarking with `--all-features`)
  a measurement problem rather than a shipping problem.
- **Impact:** Low (positive).
- **Remediation:** None. The one caveat is `cachet`'s `memory` default (F13) and
  `templated_uri`'s `uuid` default, both of which pull a real dependency on the default path.
- **Evidence:** empirically verified (read every crate's `[features]` table).

### F16. `cachet` declares a `telemetry` feature that no code references

- **Location:** `crates/cachet/Cargo.toml` (`telemetry = []` in `[features]`)
- **Issue:** `grep -rn 'feature *= *"telemetry"' crates/cachet/src` returns nothing. The feature
  is declared and gates no code. It is inert, but it is part of the crate's public API surface:
  a consumer can enable it, will observe no effect, and semver-wise it can never be removed
  without a breaking change.
- **Impact:** Low — no runtime cost. It is a correctness-of-API-surface issue and a small trap
  for anyone reasoning about which features affect performance.
- **Remediation:** Either wire it up (there is a `telemetry` module — `crates/cachet/src/telemetry/`
  — whose gating currently rides on `logs` instead) or remove the declaration.
- **Evidence:** empirically verified (grep over `crates/cachet/src` and read of the `[features]`
  table).

### F17. No `cfg(debug_assertions)` check leaks into release builds

- **Location:** `crates/bytesbuf/src/buf.rs:486,488,494` · `crates/bytesbuf/src/view.rs:90` ·
  `crates/multitude/src/internal/chunk_mutator.rs:488` ·
  `crates/ohno/src/error_label.rs:345,352,359`
- **Issue:** None. There are exactly eight `cfg(debug_assertions)` sites in the workspace and all
  eight are correctly gated — the checks they guard are compiled out of release builds. Alongside
  them the workspace uses 46 `debug_assert!`, 18 `debug_assert_eq!` and 2 `debug_assert_ne!`,
  which are by construction release-free. This is a clean bill of health on one of the classic
  ways a workspace accidentally ships debug overhead, and it aligns with the house rule about
  preserving defensive runtime checks (`docs/performance.md:40-71`) while keeping them off the
  release hot path.
- **Impact:** Low (positive).
- **Remediation:** None.
- **Evidence:** empirically verified (exhaustive grep for `debug_assertions` and `debug_assert`
  across `crates/`, then reading each of the eight `cfg` sites).

### F18. Loom marker features are inert under `--all-features`

- **Location:** the `loom` feature declarations in `crates/internity/Cargo.toml`,
  `crates/multitude/Cargo.toml`, `crates/plurality/Cargo.toml`
- **Issue:** None. These are empty marker features used in combination with `--cfg loom`; enabling
  the feature alone (as `--all-features` does) does not switch any type to a Loom shim, so
  benchmarks compiled with `--all-features` are not measuring Loom's instrumented atomics. This
  was checked specifically because it would have been a severe instance of F6 had it been
  otherwise.
- **Impact:** Low (positive) — recorded to close the question.
- **Remediation:** None.
- **Evidence:** empirically verified (read the feature declarations and their `cfg` usage).

---

## Benchmark and profiling infrastructure

### F19. No benchmark is ever executed anywhere in CI, and benches are not even compiled in PR CI

- **Location:** `justfiles/anvil/checks/bench.just:13,16` · `.github/workflows/main.yml` (627
  lines; `testing` job at 103-145, `cargo nextest run --workspace --all-features` at
  `main.yml:128`) · `justfiles/anvil/groups/scheduled-exhaustive.just:14` ·
  `.github/workflows/anvil-scheduled-impl.yml:105-116` · `docs/callgrind-benchmarks.md:436-437`
- **Issue:** Two distinct gaps.
  1. **Never executed.** The only bench recipe, `anvil-bench`, runs
     `cargo bench <scope> --all-features --no-run` (`bench.just:16`). `--no-run` compiles the
     benchmark harnesses and stops. It is referenced from exactly one group,
     `anvil-scheduled-exhaustive` (`scheduled-exhaustive.just:14`), which is wired into
     `.github/workflows/anvil-scheduled-impl.yml:105-116` and runs on x86_64 Linux and Windows
     only. So the strongest statement CI ever makes about performance is "the benchmarks still
     compile" — and only on a scheduled, non-blocking run. **There is zero performance regression
     detection of any kind.**
  2. **Not compiled on PRs.** `main.yml:128` is `cargo nextest run --workspace --all-features`
     with no `--benches`. Benchmark targets are therefore not built during pull-request CI at
     all, so a refactor can break every benchmark in the workspace and merge green. The failure
     surfaces days later on the scheduled exhaustive run, if anyone looks. `justfiles/basic.just:214-222`
     defines `test-more` which *does* pass `--tests --benches`, but grep shows it is not
     referenced from any workflow.
  The contrast with the rest of the repository is stark: this workspace gates on Miri (three
  flavours), Loom, Bolero, mutation testing, coverage, a `cargo-hack` feature powerset,
  semver-checks, `udeps`/`machete` and external-types. Performance is the only quality dimension
  with no automation whatsoever.
  **Important nuance, in fairness to the authors:** `docs/callgrind-benchmarks.md:436-437`
  states explicitly that the Gungraun trip wire is deliberately *not* a CI gate, because
  "performance trade-offs require human evaluation". That is a legitimate and defensible
  position — instruction-count gates are notoriously noisy and generate false failures. But the
  human loop it delegates to is `just bench-cg`, which per F7 **does not exist**. The deliberate
  choice not to automate has, in combination with F7, produced no loop at all rather than a human
  one.
- **Impact:** High — a workspace of performance-oriented primitives with no regression detection
  and no PR-time compile check on its benchmarks. Any of the per-crate findings the sibling
  workers produce could be reintroduced tomorrow without anything noticing.
- **Remediation:** Two cheap, independent steps, in order of value per unit of effort:
  (a) add `--benches` to the PR-time `nextest`/`check` invocation so benchmark targets are at
  least *compiled* on every PR — this costs one flag and eliminates silent bench rot;
  (b) add the missing `bench-cg` recipe (F7) so the documented human loop actually runs, and
  publish Gungraun's output as a non-blocking PR comment or artifact. A non-blocking report
  respects `docs/callgrind-benchmarks.md:436-437`'s reasoning while still surfacing regressions.
  A hard gate is not recommended.
- **Evidence:** empirically verified (read `bench.just` in full; read `main.yml` and
  `anvil-scheduled-impl.yml`; grepped every workflow for `bench`; grepped for references to
  `test-more`).

### F20. Benchmark naming and pairing conventions are violated in ways that break the documented discovery mechanism

- **Location:** `docs/naming.md:17-32` (crate-prefix rule; rationale for collisions at 28-29),
  `docs/naming.md:41-43` (`_bench` suffix ban), `docs/naming.md:76-95` (Callgrind pairing rule,
  mandatory pairing at 81-88), `docs/callgrind-benchmarks.md:153,338` (the
  `crates/*/benches/*_cg.rs` discovery glob), `docs/callgrind-benchmarks.md:267-290` (paired-setup
  requirement)
- **Issue:** Four classes of deviation, in descending severity:
  1. **A real bench-target name collision.** A target named `dynamic` exists in **both** `cachet`
     and `layered` (`crates/cachet/benches/dynamic.rs`, `crates/layered/benches/dynamic.rs`).
     `docs/naming.md:28-29` explains that the crate-prefix rule exists specifically to prevent
     collisions in `target/.../deps/`. This is that collision, in the wild.
  2. **Gungraun benchmarks that the documented discovery glob cannot find.**
     `docs/callgrind-benchmarks.md:153,338` describe locating Callgrind benches via
     `crates/*/benches/*_cg.rs`. Three do not match: `crates/routerama/benches/gungraun_routers.rs`,
     `crates/plurality/benches/gungraun/`, `crates/thread_aware/benches/gungraun_third_party/`.
     They are named with a `gungraun` *prefix* instead of a `_cg` *suffix*, so any tooling or
     human following the doc silently skips them.
  3. **Unpaired Callgrind benchmarks.** `crates/rest_over_grpc_tests/benches/rog_router_cg.rs`
     and `crates/rest_over_grpc_tests/benches/rog_transcode_cg.rs` have no corresponding Criterion
     file. `docs/naming.md:81-88` makes the pairing mandatory. (Sibling worker g5 independently
     flagged this.)
  4. **The banned `_bench` decorator.** `crates/tick/benches/clock_bench.rs` uses the suffix
     explicitly prohibited by `docs/naming.md:41-43`, and also lacks the crate prefix.
  Beyond these, many bench files simply lack the crate prefix required by `docs/naming.md:17-21`:
  `anyspawn/spawner`, `cachet/{operations,dynamic,refresh}`, `cachet_memory/overhead`,
  `fetch/{pipelines,http_crate}`, `layered/{dynamic,intercept,tower}`, `uniflight/performance`,
  `routerama_build/generator_scaling`, all of `seatbelt/*` and `templated_uri/*`,
  `plurality/{criterion,gungraun,pool_comparison}`, `multitude/criterion_*`.
  (`bytesbuf`'s `buf`, `view` and `global_pool` are explicitly grandfathered at
  `docs/naming.md:32` and are not deviations.)
- **Impact:** Medium — the collision (1) and the invisible-to-discovery Gungraun files (2) have
  concrete consequences: a colliding target name can produce confusing artifacts, and benchmarks
  nobody can find are benchmarks nobody runs. Items (3) and (4) are hygiene.
- **Remediation:** Rename to satisfy `docs/naming.md` — at minimum resolve the `dynamic`
  collision and give the three Gungraun directories/files the `_cg` suffix so the documented glob
  finds them. Add the two missing Criterion counterparts in `rest_over_grpc_tests`, or document
  why the pair is not required there.
- **Evidence:** empirically verified (full recursive listing of every `crates/*/benches/`
  directory; read of `docs/naming.md` and `docs/callgrind-benchmarks.md`).

### F21. Benchmark coverage census — 19 of 53 crates have benchmarks; 11 have Callgrind coverage

- **Location:** `crates/*/benches/` (full recursive listing), `crates/*/Cargo.toml`
  `[[bench]]` tables
- **Issue:** The census below counts bench *files* recursively, including directory-style bench
  targets (a `benches/foo/main.rs` target counts once as `foo`). "CG" marks crates with at least
  one Callgrind/Gungraun benchmark.

| Crate | Bench targets | Callgrind? | Notes |
|---|---:|:---:|---|
| `anyspawn` | 1 | — | `spawner` (unprefixed) |
| `bytesbuf` | 7 | CG | `buf`/`view`/`global_pool` grandfathered by `naming.md:32`; 8 of 10 targets need `test-util` |
| `cachet` | 3 | — | `operations`, `dynamic` (name collision, F20), `refresh`; several targets `required-features = ["logs"]` |
| `cachet_memory` | 1 | — | `overhead` (unprefixed) |
| `fetch` | 2 | — | `pipelines`, `http_crate` (both unprefixed) |
| `http_extensions` | 4 | CG | |
| `http_path_template` | 2 | CG | |
| `internity` | 3 + `counts/linux.rs` | CG | `internity_mem.rs` installs a bespoke `Tracking` allocator |
| `layered` | 3 | — | `dynamic` (collision), `intercept`, `tower` |
| `multitude` | 15 | CG | one target uses mimalloc, one uses `alloc_tracker`, rest default |
| `plurality` | 4 targets | CG | `criterion/`, `graph_churn.rs` (mimalloc), `gungraun/` (no `_cg` suffix), `pool_comparison/` |
| `rest_over_grpc_tests` | 2 | CG | both are `*_cg.rs` with **no Criterion pair** (F20) |
| `routerama` | 10 + `common/` | CG | `gungraun_routers.rs` lacks the `_cg` suffix |
| `routerama_build` | 1 | — | `generator_scaling` |
| `seatbelt` | 10 | CG | 5 install `alloc_tracker`; all unprefixed; no-telemetry path never benchmarked (F6) |
| `templated_uri` | 6 | CG | all unprefixed |
| `thread_aware` | 2 | CG | `criterion_third_party.rs` + `gungraun_third_party/` (no `_cg` suffix) |
| `tick` | 1 | — | `clock_bench.rs` — banned `_bench` suffix; compiled with `test-util` under `--all-features` (F6) |
| `uniflight` | 1 | — | `performance` (unprefixed) |

  **Crates with no benchmarks at all (34):** `anyspawn_azure`, `automation`, `benchmarking`,
  `bytesbuf_io`, `cachet_service`, `cachet_tier`, `data_privacy`, `data_privacy_core`,
  `data_privacy_macros`, `data_privacy_macros_impl`, `fetch_azure`, `fetch_hyper`,
  `fetch_options`, `fetch_tls`, `fetch_winhttp`, `fundle`, `fundle_macros`, `fundle_macros_impl`,
  `internity_macros`, `internity_macros_impl`, `multitude_macros`, `multitude_macros_impl`,
  `ohno`, `ohno_macros`, `recoverable`, `rest_over_grpc`, `rest_over_grpc_examples`,
  `routerama_macros`, `seatbelt_http`, `templated_uri_macros`, `templated_uri_macros_impl`,
  `testing_aids`, `thread_aware_macros`, `thread_aware_macros_impl`.

  Many of those are legitimately not benchmarkable (the 14 proc-macro and `_macros_impl` crates,
  `testing_aids`, `benchmarking`, `automation`, the example crates). Excluding those, roughly 19
  of ~38 benchmarkable crates have any benchmark. The notable *unbenchmarked* runtime crates are
  **`ohno`** (the workspace error type, 9 internal dependents), **`recoverable`**,
  **`cachet_tier`** (3 dependents), **`bytesbuf_io`**, **`fetch_options`**, **`fetch_tls`**,
  **`fetch_hyper`** and **`rest_over_grpc`** (13,134 LOC, 120 public functions, zero benchmarks).
- **Impact:** Medium — coverage is respectable for the data-structure crates and absent for the
  I/O and error-handling crates. `rest_over_grpc` at 13k LOC with no benchmark and no `#[inline]`
  (F23) is the single largest unmeasured surface.
- **Remediation:** Prioritise a first benchmark for `ohno` (error construction and `Display` are
  on some hot-ish paths and it has the highest fan-in in the workspace) and for `rest_over_grpc`'s
  transcoding path. Do not chase blanket coverage — `docs/performance.md`'s surgical philosophy
  argues for measuring what is actually hot.
- **Evidence:** empirically verified (recursive directory listing of `crates/*/benches/` and reads
  of the `[[bench]]` tables that declare `required-features` / directory-style targets).
### F22. Tooling installed by `just setup` supports a recipe that does not exist; version pins are, however, consistent

- **Location:** `justfiles/setup.just:53-58` → `scripts/install-callgrind-tools.ps1`;
  `constants.env` (`GUNGRAUN_RUNNER_VERSION=0.19.2`); `Cargo.toml:112` (`gungraun = "0.19.2"`)
- **Issue:** `just setup` installs Valgrind and `gungraun-runner` at a pinned version, in
  preparation for running Callgrind benchmarks — via the `bench-cg` recipe that does not exist
  (F7). Positive counterpart: the runner version in `constants.env` and the library version at
  `Cargo.toml:112` are both 0.19.2, so there is **no** version drift between the harness crate
  and the binary that runs it. That is a genuinely easy thing to get wrong and it is right here.
  Separately, `DEVELOPMENT.md`'s documented validation loop (`just build`, `just test`,
  `just test-scripts`) never mentions benchmarks at all, so a contributor following the
  development guide never encounters the benchmark suite.
- **Impact:** Low — wasted setup time, and a documentation loop with a hole in it. Subsumed by F7
  and F19 for remediation purposes.
- **Remediation:** Fix F7; then mention the benchmark loop in `DEVELOPMENT.md`.
- **Evidence:** empirically verified (read `justfiles/setup.just:53-58`, `constants.env`,
  `Cargo.toml:112`, and `DEVELOPMENT.md`).

### F23. `#[inline]` density census — the crates with the highest fan-in have none

- **Location:** whole-workspace grep of `pub ... fn` versus `#[inline`
- **Issue:** Format is `crate PUB_FN / #[inline] / #[inline(always)] / src LOC`:

| Crate | pub fn | `#[inline]` | `#[inline(always)]` | src LOC |
|---|---:|---:|---:|---:|
| `anyspawn` | 12 | 0 | 0 | 1005 |
| `anyspawn_azure` | 3 | 0 | 0 | 196 |
| `automation` | 3 | 0 | 0 | 239 |
| `benchmarking` | 3 | 0 | 0 | 306 |
| `bytesbuf` | 73 | 20 | 0 | 11550 |
| `bytesbuf_io` | 41 | 0 | 0 | 2679 |
| `cachet` | 48 | 1 | 0 | 6199 |
| `cachet_memory` | 18 | 0 | 0 | 1493 |
| `cachet_service` | 8 | 0 | 0 | 648 |
| `cachet_tier` | 25 | 0 | 0 | 1054 |
| `data_privacy` | 18 | 1 | 0 | 1263 |
| `data_privacy_core` | 4 | 0 | 0 | 541 |
| `data_privacy_macros` | 4 | 0 | 0 | 47 |
| `data_privacy_macros_impl` | 6 | 0 | 0 | 590 |
| `fetch` | 62 | 0 | 0 | 6580 |
| `fetch_azure` | 1 | 0 | 0 | 192 |
| `fetch_hyper` | 17 | 0 | 0 | 3143 |
| `fetch_options` | 25 | 0 | 0 | 1470 |
| `fetch_tls` | 21 | 0 | 0 | 1880 |
| `fetch_winhttp` | 0 | 0 | 0 | 36 |
| `fundle` | 0 | 0 | 0 | 173 |
| `fundle_macros` | 3 | 0 | 0 | 245 |
| `fundle_macros_impl` | 11 | 0 | 0 | 1066 |
| **`http_extensions`** | **114** | **0** | 0 | 10028 |
| `http_path_template` | 19 | 0 | 0 | 1208 |
| `internity` | 30 | 49 | 0 | 3318 |
| `internity_macros` | 2 | 0 | 0 | 95 |
| `internity_macros_impl` | 2 | 0 | 0 | 3437 |
| `layered` | 7 | 2 | 0 | 2088 |
| **`multitude`** | **433** | **716** | **52** | 29295 |
| `multitude_macros` | 1 | 0 | 0 | 21 |
| `multitude_macros_impl` | 1 | 0 | 0 | 2550 |
| **`ohno`** | **26** | **0** | 0 | 2231 |
| `ohno_macros` | 4 | 2 | 0 | 3506 |
| `plurality` | 73 | 122 | 0 | 3045 |
| `recoverable` | 8 | 0 | 0 | 920 |
| **`rest_over_grpc`** | **120** | **0** | 0 | 13134 |
| `rest_over_grpc_examples` | 0 | 0 | 0 | 325 |
| `rest_over_grpc_tests` | 0 | 0 | 0 | 312 |
| `routerama` | 60 | 48 | 4 | 4633 |
| `routerama_build` | 22 | 9 | 1 | 5901 |
| `routerama_macros` | 3 | 0 | 0 | 187 |
| **`seatbelt`** | **128** | **2** | 0 | 15799 |
| `seatbelt_http` | 5 | 0 | 0 | 1535 |
| `templated_uri` | 49 | 1 | 0 | 5340 |
| `templated_uri_macros` | 3 | 0 | 0 | 35 |
| `templated_uri_macros_impl` | 3 | 0 | 0 | 2139 |
| `testing_aids` | 22 | 0 | 0 | 1298 |
| **`thread_aware`** | **33** | **0** | 0 | 4972 |
| `thread_aware_macros` | 1 | 0 | 0 | 27 |
| `thread_aware_macros_impl` | 3 | 0 | 0 | 477 |
| **`tick`** | **35** | **0** | 0 | 6179 |
| `uniflight` | 6 | 4 | 0 | 637 |

  Two observations, one of which **corrects round 1**:
  - The distribution is bimodal in the extreme. `multitude` (716), `plurality` (122),
    `internity` (49), `routerama` (48) and `bytesbuf` (20) account for essentially all
    `#[inline]` in the workspace. Twenty-eight crates have exactly zero.
  - **Correction:** round 1 asserted that the well-annotated crates are exactly the
    Callgrind-covered ones. That is only partly true. `thread_aware` (33 pub fns / **0**
    `#[inline]`), `http_path_template` (19 / **0**), `templated_uri` (49 / **1**) and
    `http_extensions` (114 / **0**) all *have* Callgrind coverage and near-zero annotations.
    The correlation is with a handful of specific crates, not with Callgrind coverage as such.
  - `multitude` carries **52 `#[inline(always)]`**. `docs/performance.md:32-34` (rule 3) says
    `#[inline(always)]` must not be used without specific justification. Whether each of the 52
    carries that justification was **not audited** here — it is per-crate source scope and belongs
    to the sibling worker covering `multitude`. Flagged so it is not lost.
- **Impact:** Medium on its own; High in combination with F1 and F24 — see F24.
- **Remediation:** See F24. The census itself is data for the report author.
- **Evidence:** empirically verified (grep census over `crates/*/src/**`; counts are of
  `#[inline` attribute occurrences and of lines matching a `pub ... fn` pattern, so they are
  indicative rather than exact — the bimodality is far larger than any counting error).

---

## Workspace structure and cross-crate inlining

### F24. The three highest-fan-in crates have zero `#[inline]` while the release profile leaves LTO off

- **Location:** internal `[dependencies]` fan-in map (built from all 53 crate manifests);
  `Cargo.toml:340-341` (`[profile.release]` with no `lto`); `docs/performance.md:18-23` (rule 1)
- **Issue:** Internal fan-in, counting runtime `[dependencies]` only (dev-dependencies excluded),
  with each crate's `#[inline]` count from F23 in parentheses:

| Crate | Internal dependents | `#[inline]` |
|---|---:|---:|
| `ohno` | 9 | **0** |
| `thread_aware` | 8 | **0** |
| `layered` | 8 | 2 |
| `tick` | 7 | **0** |
| `bytesbuf` | 7 | 20 |
| `anyspawn` | 4 | **0** |
| `templated_uri` | 4 | 1 |
| `cachet_tier` | 3 | **0** |
| `recoverable` | 3 | **0** |
| `http_extensions` | 3 | **0** |
| `seatbelt` | 3 | 2 |
| `http_path_template` | 3 | **0** |
| `routerama_build` | 3 | 9 |
| `plurality` | 2 | 122 |
| `data_privacy`, `fetch_options`, `fetch_tls`, `rest_over_grpc` | 2 each | — |

  (All remaining crates have fan-in 1 or 0.)

  Put the three facts together:
  1. `[profile.release]` does not set `lto`, so consumers build with `lto = false`
     (`Cargo.toml:340-341`, F1).
  2. Without LTO, a non-generic, non-`#[inline]` public function **cannot** be inlined across a
     crate boundary — the callee's MIR is not available to the caller's codegen unit.
  3. The three crates with the most internal dependents — `ohno` (9), `thread_aware` (8),
     `tick` (7) — have **zero** `#[inline]` annotations between them.
  So every call from any of ~24 dependent crates into `ohno`, `thread_aware` or `tick` is a real,
  non-inlinable function call in a consumer's release build. For `tick` this is directly at odds
  with `crates/tick/src/clock.rs:17`'s claim of "zero-cost overhead in production": a clock read
  that must cross a crate boundary as an opaque call is not zero-cost, whatever the enum shape.
  This is precisely the case `docs/performance.md:18-23` rule 1 was written for, and it is
  unaddressed in exactly the crates where it matters most.

  This is also the answer to the "is the fine-grained 53-crate split a performance concern"
  question: yes, but conditionally. The split is only a problem *because* `release` leaves LTO
  off. A 53-crate workspace with `lto = "thin"` on release would have almost none of this
  exposure. The split itself is good design — it is the profile that fails to accommodate it.
- **Impact:** High — this is the workspace-level finding with the most plausible real-world cost,
  and it is invisible to every existing benchmark because of F1 (fat LTO inlines these calls
  anyway during benchmarking). F1 and F24 are the same defect seen from two ends.
- **Remediation:** Two independent, both surgical:
  (a) Add `#[inline]` to the small public functions of `ohno`, `tick`, `thread_aware`,
      `recoverable`, `cachet_tier` and `http_extensions` — accessors, constructors, small
      wrappers. This is exactly what `docs/performance.md:18-23` already asks for; it is not a new
      policy. Verify with a *release-faithful* benchmark configuration, not the current fat-LTO
      one (F1), or the verification will report no benefit.
  (b) Consider `lto = "thin"` on `[profile.release]` so that the workspace's own release builds
      match what a well-configured consumer would do, and document the recommendation for
      consumers. `thin` LTO is cheap relative to `fat` and would recover most cross-crate inlining.
  Recommendation (a) first: it helps every consumer regardless of their profile, which (b) cannot.
- **Evidence:** empirically verified (fan-in map built by parsing the `[dependencies]` table of
  every crate manifest; `#[inline]` counts from the F23 census; `Cargo.toml:340-341` read
  directly). The inlining consequence is inferred from code reading plus documented rustc
  semantics.

### F25. 26% of the workspace is proc-macro machinery, compiled against two `syn` generations

- **Location:** the 8 proc-macro crates (`data_privacy_macros`, `fundle_macros`,
  `internity_macros`, `multitude_macros`, `ohno_macros`, `routerama_macros`,
  `templated_uri_macros`, `thread_aware_macros`) plus 6 `_macros_impl` crates;
  `Cargo.lock` (`syn` 2.0.118 and 3.0.3, `prettyplease` 0.2.37 and 0.3.0)
- **Issue:** 14 of 53 crates (26%) exist solely to generate code. Several are large:
  `ohno_macros` 3506 LOC, `internity_macros_impl` 3437, `multitude_macros_impl` 2550,
  `templated_uri_macros_impl` 2139. All eight workspace proc-macros use `syn` 3, while 46 other
  proc-macro crates in the graph use `syn` 2 — so both `syn` generations and both `prettyplease`
  generations compile on every clean build, and proc-macro crates are compiled for the *host* and
  cannot be cross-compiled away. Additionally there are exactly two build scripts in the whole
  workspace (`crates/rest_over_grpc_examples/build.rs`,
  `crates/rest_over_grpc_tests/build.rs`), both invoking `protox` / `prost-build` /
  `tonic-prost-build`, which is a heavy code-generation step — though both are in test/example
  crates, so consumers do not pay for them.
- **Impact:** Low for runtime (proc-macros cost nothing at run time, and the split into
  `_macros` / `_macros_impl` is the correct pattern for testability); Medium for developer
  compile time, which the `[profile.dev]` gap in F2 does nothing to mitigate.
- **Remediation:** Nothing at runtime. If build times are a pain point, the standard remedy is
  per-package `opt-level` overrides for `syn`/`prettyplease`/`proc-macro2` in `[profile.dev]`
  (F2). The `syn` 2-vs-3 split will resolve as the ecosystem migrates and is not actionable here.
- **Evidence:** empirically verified (crate inventory and LOC counts; `Cargo.lock` parsing; grep
  for `build.rs` across the workspace).

### F26. Published crates include `benches/**`

- **Location:** `Cargo.toml:32-42` (`[workspace.package].include`)
- **Issue:** The packaging allowlist includes `/benches/**`, so benchmark sources ship in every
  published `.crate`. This is a size and (per `docs/packaging-guidelines.md`'s LFS rule)
  reproducibility surface. It was checked specifically for the LFS hazard the packaging guidelines
  warn about; no LFS-tracked binary was found under any `benches/` directory, so the packaging
  rule is not violated.
- **Impact:** Low — larger published artifacts; also means a consumer can run the benchmarks,
  which some maintainers consider a feature. No correctness or performance issue.
- **Remediation:** None required. Noted so the packaging surface is documented.
- **Evidence:** empirically verified (read `Cargo.toml:32-42`; checked `.gitattributes` and
  `benches/` contents for LFS-tracked files).

---

## Considered and ruled out

The following were investigated at workspace level and found to be **non-issues**. They are
recorded so the report author knows they were checked rather than missed.

1. **`http_path_template`'s default `std` feature capturing backtraces.** The default feature set
   is `["std"]`, and `std` enables backtrace capture in `ParseError`. This looked like a
   default-on cost. It is not: `MaybeBacktrace` at `crates/http_path_template/src/error.rs:66-80`
   only allocates/boxes when `Backtrace::capture()` reports the capture is actually enabled
   (i.e. when `RUST_BACKTRACE` is set). The no-backtrace path is a cheap unit-like variant. This
   is well-designed and needs no change. *Empirically verified* (read `error.rs:66-80`).

2. **`cfg(debug_assertions)` leaking into release.** Exhaustively checked; all eight sites are
   correctly gated. See F17 — recorded there as a positive finding. *Empirically verified.*

3. **Loom features being accidentally active under `--all-features`.** Checked because it would
   have been a severe amplification of F6. They are inert marker features requiring `--cfg loom`
   in addition. See F18. *Empirically verified.*

4. **`[workspace.dependencies]` failing to disable default features.** Checked the entire table
   against the mandate at `Cargo.toml:44-50`; it is honoured, and CI enforces it
   (`main.yml:186`). See F14 — positive. *Empirically verified.*

5. **A workspace-level `#[global_allocator]` being imposed on consumers.** No library crate sets
   one; only benchmark binaries do. Consumers get the system allocator, which is the correct
   default for a library. See F8, which is scoped to benchmark comparability only.
   *Empirically verified.*

6. **`GUNGRAUN_RUNNER_VERSION` drifting from the `gungraun` library version.** `constants.env`
   says 0.19.2 and `Cargo.toml:112` says 0.19.2. No drift. Recorded in F22 as a positive.
   *Empirically verified.*

7. **Toolchain pinning inconsistency.** `constants.env` gives `RUST_MSRV=1.93`,
   `RUST_LATEST=1.96.1`, `RUST_NIGHTLY=nightly-2026-05-30`, and the root manifest declares
   edition 2024 / `rust-version` 1.93. These are mutually consistent and CI uses them coherently.
   No finding. *Empirically verified.*

8. **`overflow-checks` / `debug-assertions` accidentally enabled in release.** Neither is set in
   `[profile.release]` (`Cargo.toml:340-341`), so both take their release defaults of `false`.
   No leak. *Empirically verified.*

9. **Per-crate `[profile]` tables overriding workspace settings.** Cargo ignores non-workspace-root
   `[profile]` tables with a warning, so a crate-level one would be a latent trap. Grep found none.
   *Empirically verified.*

10. **`bytesbuf`'s `test-util` feature changing hot-path types under `--all-features`.** Checked
    because `tick`'s does (F6). `bytesbuf`'s merely adds `pub mod testing`
    (`crates/bytesbuf/src/mem/mod.rs:61-62`) and does not alter the shape of any hot type. Benign.
    *Empirically verified.*
