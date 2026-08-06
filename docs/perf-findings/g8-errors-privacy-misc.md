# g8-errors-privacy-misc findings

Performance analysis of thirteen crates: `ohno`, `ohno_macros`, `data_privacy`,
`data_privacy_core`, `data_privacy_macros`, `data_privacy_macros_impl`, `fundle`,
`fundle_macros`, `fundle_macros_impl`, `recoverable`, `testing_aids`,
`benchmarking`, `automation`.

Analysis only — no source file in the workspace was modified.

## How to read this document

Every finding carries an **Evidence** line reading either `empirically verified`
(with the method) or `inferred from code reading`. Findings that pull against
`docs/performance.md` carry an explicit **Philosophy note**.

Impact grades are relative *within this group*, and are assigned on the
frequency-times-cost axis the house philosophy uses: a cost paid at request rate
outranks a larger cost paid once at construction or teardown.

---

## Environment constraint and how it was worked around

There is no egress to `index.crates.io` / `static.crates.io` from this container
(403 `CONNECT tunnel failed`), no populated `~/.cargo/registry`, and no prebuilt
`target/`. `cargo build`, `cargo clippy`, `cargo bench`, `cargo build --offline`
and every `just` recipe fail at dependency resolution:

```
$ cargo build -p ohno --offline
error: no matching package named `tokio` found
location searched: crates.io index
```

Confirming this was time-boxed as instructed. Real empirical numbers were
obtained instead with three throwaway standalone `rustc` programs written to
`/tmp/perf8` (never added to the repository, deleted afterwards):

1. `layout.rs` — layout-identical replicas of the `ohno` and `data_privacy`
   types, printing `size_of` / `align_of`.
2. `alloc.rs` — a counting `GlobalAlloc` measuring per-operation allocation
   counts, byte totals and deallocation counts, run under `RUST_BACKTRACE=0`,
   `=1` and `=full`.
3. `alloc2.rs` — an isolation harness that decomposes the `ohno` error
   construction path into its individual allocations, to attribute the
   `Box` → `Arc` conversion cost precisely.

Replicas are layout- and allocation-identical to the real types (same field
order, same `repr`, same owning containers); they are not the real types, so
each finding says so. Everything else in this document is
`inferred from code reading`.

---

## Empirical results

These are the only measured numbers available to the whole team. Reproduced
verbatim from the probe output.

### Type layouts (x86-64, `rustc -O`)

```
--- ohno ---
BacktraceR (Captured(Arc)|Disabled|Unsupported)     size= 16 align= 8
Location { &'static str, u32 }                      size= 24 align= 8
EnrichmentEntry { Cow<'static,str>, Location }      size= 48 align= 8
Source (None|Transparent(Arc<dyn>)|Error(Arc<dyn>)) size= 24 align= 8
Inner { Source, Backtrace, Vec<EnrichmentEntry> }   size= 64 align= 8
OhnoCore { Box<Inner> }                             size=  8 align= 8
Option<OhnoCore>                                    size=  8 align= 8
Result<(), OhnoCore>                                size=  8 align= 8   <-- NO Result inflation
Result<u64, OhnoCore>                               size= 16 align= 8
Result<[u8;64], OhnoCore>                           size= 72 align= 8
AppError                                            size=  8 align= 8
Result<(), AppError>                                size=  8 align= 8
ConfigError { String, OhnoCore } (typical derive)   size= 32 align= 8
ErrorLabel(Cow<'static,str>)                        size= 24 align= 8
std::backtrace::Backtrace                           size= 48 align= 8

--- data_privacy ---
DataClass { Cow, Cow }                              size= 48 align= 8
Cow<'static,str>                                    size= 24 align= 8
Sensitive<u8>                                       size= 56 align= 8   <-- 7x the payload
Sensitive<String>                                   size= 72 align= 8
Sensitive<()>                                       size= 48 align= 8
SimpleRedactorMode                                  size= 32 align= 8
RedactionPolicy (Redact(Box<dyn>)|Suppressed)       size= 16 align= 8
```

### Allocation counts (counting `GlobalAlloc`)

```
### RUST_BACKTRACE=0
OhnoCore::from(io::Error)                allocs=6   bytes=148    deallocs=1
OhnoCore::from(&'static str)             allocs=4   bytes=132    deallocs=1
OhnoCore::new() (no source)              allocs=1   bytes=64     deallocs=0
OhnoCore::clone() (0 enrichments)        allocs=1   bytes=64     deallocs=0
OhnoCore::clone() (3 enrichments)        allocs=2   bytes=208    deallocs=0
first .enrich() (Vec first push)         allocs=1   bytes=192    deallocs=0
derived Display w/ #[display] -> fmt     allocs=1   bytes=34     deallocs=1
DataClass::clone (both Borrowed)         allocs=0   bytes=0      deallocs=0
DataClass::clone (both Owned/deser'd)    allocs=2   bytes=15     deallocs=2
Sensitive fmt, value <=128 bytes         allocs=0   bytes=0      deallocs=0
Sensitive fmt, value >128 bytes          allocs=2   bytes=400    deallocs=2

### RUST_BACKTRACE=1  (identical results for RUST_BACKTRACE=full)
OhnoCore::from(io::Error)                allocs=10  bytes=1780   deallocs=1   (12.0x bytes)
OhnoCore::from(&'static str)             allocs=8   bytes=1764   deallocs=1   (13.4x bytes)
OhnoCore::new() (no source)              allocs=5   bytes=1696   deallocs=0   (26.5x bytes)
(all non-construction rows unchanged)
```

### `Box` → `Arc` conversion isolation (`RUST_BACKTRACE=0`)

```
io::Error::other("boom") alone                    allocs=3  bytes=52  deallocs=0
  + Box::new(io::Error) [into Box<dyn>]           allocs=1  bytes=8   deallocs=0
  + Box<dyn> -> Arc<dyn> (the .into())            allocs=1  bytes=24  deallocs=1
&'static str -> Box<dyn StdError>                 allocs=2  bytes=28  deallocs=0
  + Box<dyn> -> Arc<dyn> (the .into())            allocs=1  bytes=40  deallocs=1
Arc::new(io::Error) directly (hypothetical fix)   allocs=4  bytes=76  deallocs=0
```

---

## Cross-cutting: benchmark coverage across the whole group

**None of the thirteen crates has a `benches/` directory.** Verified by direct
listing of every `crates/<name>/` in scope; there is no `benches` entry and no
`[[bench]]` section in any of the thirteen `Cargo.toml` files.

This matters more than the raw statement suggests, because of context from the
workspace-level analysis:

* Root `Cargo.toml` sets `[profile.bench] lto = "fat"` and `codegen-units = 1`,
  while `[profile.release]` sets only `debug = "line-tables-only"` — neither LTO
  nor `codegen-units = 1`. Benchmark numbers in this repository therefore come
  from a build configuration no consumer receives, and are **structurally blind
  to missing `#[inline]`**: fat LTO inlines across crate boundaries regardless
  of the attribute, so a benchmark cannot detect the very cross-crate inlining
  gaps identified below.
* Benchmarks build with `--all-features`.
* No benchmark is executed in CI at all.

Consequence for this group: even if the benchmarks recommended below were
written, they would not by themselves validate any `#[inline]` finding. The
`#[inline]` findings in this document are therefore justified by API-shape
reasoning, which `docs/performance.md` explicitly sanctions for small
non-generic exported functions.

Per-crate recommendations are in each crate's **Benchmark coverage** section.
Summarised, the operations that genuinely warrant coverage are:

| Crate | Operation | Why |
|---|---|---|
| `ohno` | `OhnoCore::from(io::Error)` / `::new` | 4–6 allocations on every error; the group's most-executed error path |
| `ohno` | `.enrich()` (first and subsequent) | first push allocates 192 bytes; subsequent are free |
| `ohno` | derived `Display::fmt` and `ErrorExt::message()` | one and two allocations respectively, per format |
| `ohno` | `OhnoCore::clone()` at 0 / 3 / 10 enrichments | quantifies F2 |
| `data_privacy` | `RedactionEngine::redact` on a hit and a miss | the request-rate hot path |
| `data_privacy` | `Sensitive<T>` / `#[classified]` redacted `Display` | ≤128 and >128 byte payloads straddle the stack-buffer cliff |
| `data_privacy_core` | `DataClass` hash + equality | the lookup key cost in isolation |
| `recoverable` | `RecoveryInfo::from(io::ErrorKind)` | cheap, but it is the crate's only real operation |

Operations that do **not** warrant benchmark coverage, and why:

* All five proc-macro crates (`ohno_macros`, `data_privacy_macros`,
  `data_privacy_macros_impl`, `fundle_macros`, `fundle_macros_impl`). Their cost
  is compile time, which Criterion cannot measure meaningfully; the correct
  instrument is `cargo build --timings` / `-Zself-profile`. What *should* be
  benchmarked is the code they generate — and that is covered by benchmarking
  `ohno`'s derived `Display` and `data_privacy`'s `#[classified]` types, which
  is where those macros' output actually runs.
* `testing_aids` — dev-dependency-only, never in a release path (verified
  below). Benchmarking it would measure test infrastructure.
* `benchmarking` — it *is* the harness. Benchmarking the harness with the
  harness is circular. Its correctness is better served by the existing
  drop-ordering regression tests plus the fix proposed in F39.
* `automation` — build tooling, `publish = false`, invoked by humans and CI at
  human timescales.
* `fundle` — the runtime crate is doc-and-re-export only (147 lines, no logic);
  what would be benchmarked is generated code, and the only generated code with
  a runtime cost is the `#[deps]` / `#[newtype]` clone in F33/F34, which is a
  construction-path cost the philosophy deprioritises.

---

## Crate: ohno

### Summary

`ohno` is the workspace's error type. Its central design decision is excellent
and should be protected: `OhnoCore` is a single `Box<Inner>`, so it is **8 bytes**
and `Result<(), OhnoCore>` is also **8 bytes** — there is zero `Result` inflation
on the success path. Every function in the workspace that returns
`Result<T, SomeOhnoError>` pays nothing on success beyond what `T` already costs.
This is the single most important performance property an error type can have,
this crate has it, and it is already protected by a size regression test at
`crates/ohno/tests/size_test.rs`.

The costs are all on the error path, and the philosophy correctly deprioritises
those relative to hot paths. They are still worth stating because error paths in
this workspace are not always cold: `ohno` errors are constructed by `fetch`,
`cachet` and `seatbelt` in response to conditions (cache miss escalation, retry,
timeout, upstream 5xx) that occur at a meaningful fraction of request rate under
load, which is exactly when the system can least afford six allocations per
event.

Nine findings follow. The headline one is F1: error construction performs a
`Box` allocation and then immediately reallocates it as an `Arc`, throwing the
`Box` away — one wholly avoidable allocation plus one deallocation plus a memcpy
on every single error the workspace constructs.

### Findings

#### F1. Every error construction allocates a `Box`, then immediately re-allocates it as an `Arc` and frees the `Box`

- **Location:** `crates/ohno/src/core.rs:185`, `crates/ohno/src/core.rs:187`; the
  receiving field is `Source` at `crates/ohno/src/source.rs`, and the same
  double conversion is reachable via `OhnoCore::new_from` at
  `crates/ohno/src/core.rs:65`.
- **Issue:** The `From<E> for OhnoCore` impl writes `error.into().into()`. The
  first `.into()` produces a `Box<dyn StdError + Send + Sync>` (the standard
  library's blanket conversion); the second converts that `Box` into an
  `Arc<dyn StdError + Send + Sync>`. That second conversion cannot reuse the
  `Box`'s allocation, because an `Arc` needs a header (strong and weak counts)
  in front of the value. `Arc<T>: From<Box<T>>` therefore allocates a fresh
  block sized `header + value`, memcpys the value across, and frees the `Box`.
  The intermediate `Box` exists only to be destroyed.
- **Impact:** Medium — one avoidable allocation, one deallocation and one memcpy
  on *every* `ohno` error constructed anywhere in the workspace, which is the
  most-executed error path in the repository. It is not High because it is
  strictly on the error path and the absolute cost is one malloc/free pair.
- **Remediation:** Have the conversion produce the `Arc` directly rather than
  laundering through a `Box`. Concretely, add an `Arc`-producing constructor on
  `Source` and call it with the concrete `E` (`Arc::new(error) as Arc<dyn ...>`),
  so the unsizing coercion happens on an already-`Arc`-allocated value. This is a
  contained change to one conversion site — surgical, not architectural, and it
  removes an allocation from an error path without altering any behaviour or
  removing any check.
- **Evidence:** empirically verified — the `alloc2.rs` isolation probe
  decomposed the path. `io::Error::other("boom")` alone: 3 allocs / 52 bytes.
  Boxing it as `Box<dyn StdError>`: +1 alloc / 8 bytes. The `Box<dyn>` → `Arc<dyn>`
  conversion: **+1 alloc / 24 bytes and +1 dealloc**. For a `&'static str`
  source the conversion costs +1 alloc / 40 bytes / +1 dealloc. The
  `Arc::new(...)` direct route totals 4 allocs / 76 bytes with **zero**
  deallocations, versus 5 allocs and 1 dealloc for the current route. The 1
  `dealloc` visible in the full-path measurements (`OhnoCore::from(io::Error)`:
  `allocs=6 bytes=148 deallocs=1`) is precisely this discarded `Box`.

#### F2. `OhnoCore: Clone` deep-copies the entire error including the enrichment `Vec`

- **Location:** `crates/ohno/src/core.rs:42-45` (the `#[derive(Clone)]` on
  `OhnoCore`), over `Inner` at `crates/ohno/src/core.rs:14-19`.
- **Issue:** `OhnoCore` is `Box<Inner>`, so cloning it clones `Inner`, which
  clones the `Source` (cheap — `Arc` refcount bump), the `Backtrace` (cheap —
  also `Arc`), and the `Vec<EnrichmentEntry>` (**not** cheap — a fresh heap
  allocation plus a per-element clone, and each `EnrichmentEntry` holds a
  `Cow<'static, str>` that itself allocates if it is `Owned`, which it is
  whenever the message came from a `format!`).
- **Impact:** Medium. Cloning an error is not rare: it happens whenever an error
  is stored and also returned, fanned out to multiple waiters (a
  cache-stampede-collapse or retry coordinator does exactly this), or converted
  between layers. The cost scales with enrichment depth, and enrichment depth
  grows precisely as an error propagates up through more layers — so the most
  expensive clones are of the most deeply propagated errors.
- **Remediation:** The clean fix is `Box<Inner>` → `Arc<Inner>`, making `Clone`
  a refcount bump and keeping `OhnoCore` at 8 bytes. But mutation
  (`enrich`) would then need `Arc::make_mut`, changing the cost model of
  enrichment (copy-on-write on a shared error) and requiring `Inner: Clone` to
  stay. That is a rewrite of the workspace's central error type.
  **Recommendation: do not do this now.** Instead, benchmark it first (see
  Benchmark coverage) so the trade is made on numbers rather than on this
  document's reasoning.
- **Evidence:** empirically verified — measured `OhnoCore::clone()` with 0
  enrichments at `allocs=1 bytes=64 deallocs=0` (the `Inner` box only), and with
  3 enrichments at `allocs=2 bytes=208 deallocs=0` (the `Inner` box plus a
  144-byte `Vec` buffer for 3 × 48-byte entries, rounded to the 4-element
  capacity the measurement shows). Layout replica of `Inner` and
  `EnrichmentEntry`; not the real types.
- **Philosophy note:** CONFLICTING. `docs/performance.md` prefers surgical
  interventions over architectural rewrites, and this is unambiguously an
  architectural rewrite of a type that appears in the signature of a large
  fraction of the workspace's public API. It is reported because the cost is
  real, but the philosophy's answer here is "measure, then probably don't".

#### F3. Derived `Display` allocates a `String` on every format

- **Location:** `crates/ohno_macros/src/derive_error/display.rs:108-114`
  (`generate_display_expression`), consumed by
  `crates/ohno_macros/src/derive_error/mod.rs:112-139`
  (`generate_display_impl`). Listed under `ohno` as well as `ohno_macros`
  because the cost lands in every `ohno`-derived error type in the workspace.
- **Issue:** For an error variant carrying `#[display("...")]`, the macro emits
  `std::borrow::Cow::from(format!(...))` and then writes that `Cow` to the
  formatter. `format!` allocates a `String`, the `String` is written out, and it
  is dropped immediately. The `Display` implementation is handed a
  `&mut Formatter` — a sink — and constructs an intermediate owned buffer
  anyway.
- **Impact:** Medium. `Display` on an error is called by every logging
  statement, every `to_string()`, every `{}` interpolation, and by
  `ErrorExt::message()`. On a service that logs its errors — which is all of
  them — this is one allocation per logged error, per log site.
- **Remediation:** Emit `write!(f, "...")` directly against the formatter
  instead of materialising a `Cow`. The `Cow` return type exists so that the
  no-`#[display]` case can return a borrowed `&'static str`; that case can be
  served by `f.write_str(...)`. Both branches then write to the sink and neither
  allocates. This is a change to the macro's codegen only — no public API
  change, no behaviour change in the rendered text.
- **Evidence:** empirically verified — a hand-written replica of the emitted
  code measured `allocs=1 bytes=34 deallocs=1` per `Display::fmt` call.
  Code reading confirms the emitted shape at `display.rs:108-114`.

#### F4. `ErrorExt::message()` allocates twice per call

- **Location:** `crates/ohno_macros/src/derive_error/mod.rs:236-248`
  (`generate_error_ext_impl`), which routes through `format_message` at
  `crates/ohno/src/core.rs:118-153`.
- **Issue:** `message()` builds a `MessageFormatter` and calls `.to_string()` on
  it. `to_string()` allocates the result `String`. The `Display` impl that
  `to_string()` drives is the F3 code, which allocates its own intermediate
  `String` first. Net: two allocations to obtain one message.
- **Impact:** Medium — `message()` is the idiomatic way to get an error's text
  for structured logging, so it runs at log rate. It compounds directly with F3:
  fixing F3 halves this finding, which is a good reason to prioritise F3.
- **Remediation:** Fixing F3 removes the inner allocation. The outer one is
  inherent to a `-> String` signature; if the caller only wants to write the
  message somewhere, an additional `message_to(&mut impl fmt::Write)` or simply
  documenting `write!(sink, "{err}")` as the zero-allocation path would let
  callers opt out. Do not remove `message()` — it is the ergonomic API and
  ergonomics are worth an allocation on an error path.
- **Evidence:** inferred from code reading, with the inner allocation
  empirically verified as part of F3.

#### F5. No build-time opt-out for backtrace capture; enabling backtraces costs 12–26x the allocated bytes per error

- **Location:** `crates/ohno/src/backtrace.rs:37-40`; feature list in
  `crates/ohno/Cargo.toml` (the only features are `app-err` and `test-util`).
- **Issue:** Error construction always calls `StdBacktrace::capture()`. That
  function is cheap when `RUST_BACKTRACE` is unset — it returns `Disabled`
  without unwinding — so the default path is fine. But the decision is purely a
  runtime environment-variable check; there is no way for a consumer to compile
  backtrace capture *out*. An operator who sets `RUST_BACKTRACE=1` for
  diagnostics — a completely normal thing to do, and often done globally in a
  container image — silently multiplies the cost of every error in the process.
- **Impact:** Medium. Not High, because the default (env unset) is already the
  fast path and the multiplier only applies when an operator opts in. But the
  opt-in is coarse (process-wide, all-or-nothing) and its cost is invisible at
  the point of decision, which is the classic shape of a production performance
  surprise.
- **Remediation:** Add a `backtrace` Cargo feature, default-on to preserve
  current behaviour, that compiles `Backtrace::capture()` down to the
  `Unsupported`/`Disabled` variant when disabled. This preserves the defensive
  default (philosophy: preserve defensive runtime checks) while giving a
  latency-sensitive consumer a way to guarantee the cost is gone. Additionally,
  document the measured multiplier in the crate docs so the operator setting
  `RUST_BACKTRACE=1` knows what it costs.
- **Evidence:** empirically verified — the counting allocator was run under
  three settings of `RUST_BACKTRACE`. With `=0`: `OhnoCore::from(io::Error)` is
  `allocs=6 bytes=148`; `OhnoCore::new()` is `allocs=1 bytes=64`. With `=1`
  (and identically with `=full`): `from(io::Error)` becomes
  `allocs=10 bytes=1780` (**12.0x** the bytes) and `new()` becomes
  `allocs=5 bytes=1696` (**26.5x** the bytes). `from(&'static str)` goes from
  132 to 1764 bytes (**13.4x**). All non-construction operations were unchanged,
  confirming the cost is entirely in capture and not in formatting or
  enrichment. `std::backtrace::Backtrace` measured at 48 bytes, but `ohno`'s
  `Backtrace` wrapper is 16 bytes because it holds an `Arc` — good design, and
  it means the *layout* cost of backtraces is already minimised.

#### F6. `ErrorLabel::from_error_chain` allocates a `String` even for a single-label chain

- **Location:** `crates/ohno/src/error_label.rs:152-161`
  (`from_error_chain`), calling `from_parts` at
  `crates/ohno/src/error_label.rs:118-130`.
- **Issue:** When the error has a source, `from_error_chain` unconditionally
  routes through `from_parts`, which builds an owned `String` by joining the
  chain's labels. For a chain that resolves to exactly one label — the common
  case for a leaf error wrapping a single `io::Error` — the result is
  byte-identical to a `&'static str` that the code already has in hand, and the
  `Cow` could have stayed `Borrowed`.
- **Impact:** Low. Error labels are typically computed at telemetry-emission
  time rather than at error-construction time, so the frequency is
  log-rate-divided-by-something, and the allocation is small. Reported for
  completeness and because the fix is a two-line early return.
- **Remediation:** Short-circuit in `from_error_chain`: if the chain yields a
  single label, return `ErrorLabel(Cow::Borrowed(that_label))` without entering
  `from_parts`. `ErrorLabel` is already a `Cow<'static, str>` (measured 24
  bytes), so the borrowed representation costs nothing extra.
- **Evidence:** inferred from code reading.

#### F7. `app_err!` with a literal message allocates through `format!` for no reason

- **Location:** `crates/ohno/src/app/macros.rs:45-47`.
- **Issue:** `app_err!("some message")` expands to
  `AppError::new(format!($msg))`. With no interpolation arguments, `format!` on
  a literal still constructs a `String` — the compiler will not fold it away
  into a `&'static str`, because the expression's type is `String`.
- **Impact:** Low — error path, one small allocation, and the macro exists for
  ergonomics which is worth something. But the no-argument case is likely the
  majority of uses and the fix is free.
- **Remediation:** Give the macro two arms: a single-literal arm that passes the
  `&'static str` straight through (`AppError::new($msg)` where `new` takes
  `impl Into<Cow<'static, str>>`), and the existing `format!` arm for the
  arguments case. This is a standard pattern in error macros across the
  ecosystem (`anyhow!` does precisely this), so it is a move *towards* the
  ecosystem default rather than away from it.
- **Evidence:** inferred from code reading.

#### F8. Eager `to_string()` in the non-lazy `IntoAppErr` enrichment methods

- **Location:** `crates/ohno/src/app/into_app_err.rs:41`, `:62`, `:75`, `:84`,
  `:95`, `:113`.
- **Issue:** The eager variants call `msg.to_string()` to build the enrichment
  message. This is correct for the eager API contract, but it means the string
  is materialised even on the `Ok` path if the call site is written as
  `result.with_context(expensive_message())` — the argument is evaluated before
  the method runs.
- **Impact:** Low. The `_with` closure-taking variants in the same file are
  correctly lazy, so the fast path already exists and is idiomatic; this is a
  documentation and API-guidance matter more than a code defect.
- **Remediation:** No code change. Document in the trait's rustdoc that the
  eager variants evaluate their argument unconditionally and that the `_with`
  variants should be preferred whenever the message costs anything to build.
  This is the ecosystem-standard split (`anyhow`'s `context` vs `with_context`)
  and matching it is correct.
- **Evidence:** inferred from code reading.

#### F9. Zero `#[inline]` annotations across 25 public functions

- **Location:** crate-wide; `crates/ohno/src/` contains no `#[inline]` outside
  `test_util.rs` (which has 3, in code that is behind the `test-util` feature).
  The affected accessors include `OhnoCore::backtrace`, `OhnoCore::source`,
  `OhnoCore::enrichments`, `ErrorLabel::as_str`, and the `AppError` accessors.
- **Issue:** These are small, non-generic, exported functions. Without
  `#[inline]`, a downstream crate compiled without LTO cannot inline them,
  because their MIR is not available across the crate boundary. Several are
  trivial field projections whose call overhead exceeds their body.
- **Impact:** Low — individually each is a handful of instructions, and most are
  on error paths. Reported because the *pattern* is systematic (zero out of 25)
  rather than considered, and because `docs/performance.md` rule 1 names exactly
  this case.
- **Remediation:** Be judicious, per the philosophy: annotate only the trivial
  non-generic accessors that a caller would reasonably use in a loop, and leave
  the constructors and formatting functions alone (they are large enough that
  inlining them costs code size for no gain). A defensible narrow set is
  `OhnoCore::backtrace`, `OhnoCore::source`, `OhnoCore::enrichments`,
  `ErrorLabel::as_str`.
- **Evidence:** inferred from code reading (grep census of `#[inline]` across
  `crates/ohno/src/`). Note that this finding **cannot** be validated by a
  benchmark in this repository, because `[profile.bench]` sets `lto = "fat"`,
  which inlines across crate boundaries regardless of the attribute. Any
  benchmark added for it would show no difference and would be misleading.
- **Philosophy note:** Partially conflicting. `docs/performance.md` is
  deliberately restrained about `#[inline]`, and rule 1's applicability is in
  tension with the document's own "measure first" stance — a tension that cannot
  be resolved here because the repository's benchmark profile is blind to the
  effect. The recommendation above is deliberately narrower than "annotate
  everything rule 1 covers" for that reason.

### Benchmark coverage

`crates/ohno/benches/` does not exist. This is the highest-value benchmark gap
in the group: `ohno` has 25 public functions, zero benchmark coverage, and sits
in the return type of a large fraction of the workspace's public API.

Warranted, in priority order:

1. **`OhnoCore::from(io::Error)` and `OhnoCore::new()`** — elementary
   construction. This is the operation F1 and F5 are about, and it is the one
   number that would let the team decide whether error construction cost matters
   at all. Pair with a Callgrind `_cg.rs` file: construction is an
   allocation-heavy but branch-light operation, exactly the shape where
   instruction counts are stable and informative.
2. **`.enrich()` — first call and second call, benchmarked separately.** The
   first allocates the `Vec` (measured 192 bytes); the second is a push into
   existing capacity. Benchmarking them together would average a
   first-insert cost the philosophy explicitly deprioritises into a number that
   looks worse than the steady state.
3. **Derived `Display::fmt` and `ErrorExt::message()`** — validates F3 and F4,
   and would catch a regression if the macro's codegen changes. Use
   `std::hint::black_box` on the formatter sink so the write is not elided.
4. **`OhnoCore::clone()` at 0, 3 and 10 enrichments** — the evidence F2's
   go/no-go decision needs. Three separate Criterion inputs, not one averaged
   case.

Not warranted: the `ErrorLabel` chain walk (frequency too low to justify the
maintenance), and anything under the `test-util` feature.

### Considered and ruled out

* **`Result<T, OhnoCore>` inflation on the success path.** Ruled out
  empirically: `OhnoCore` is 8 bytes, `Option<OhnoCore>` is 8 bytes (niche
  optimisation through the `Box`), and `Result<(), OhnoCore>` is 8 bytes. This
  is the correct design and there is nothing to improve. It is already protected
  by `crates/ohno/tests/size_test.rs`.
* **Large error enum variants inflating the whole enum.** Does not apply. The
  workspace's derived error types embed `OhnoCore` (8 bytes) rather than
  inlining source data; a representative derived type measured 32 bytes
  (`String` + `OhnoCore`). There is no oversized variant to box.
* **`Source` being an enum with two `Arc` variants (24 bytes).** Considered
  shrinking it; ruled out — it is already niche-packed to the size of a fat
  pointer plus a discriminant word, it lives behind the `Box<Inner>` so it never
  appears in a `Result`, and distinguishing `Transparent` from `Error` is
  semantically necessary.
* **`enrichment: Vec::new()` in the constructors** (`crates/ohno/src/core.rs:67`,
  `:77`). Considered flagging the eventual `Vec` allocation; ruled out as a
  *positive* — `Vec::new()` does not allocate, so an error that is never
  enriched never pays for the `Vec`. This is exactly right.
* **`format!` inside `#[enrich_err]`'s generated code.** Considered; ruled out —
  it is inside the `map_err` closure, so it is lazily evaluated on the error
  path only. Correct as written.
* **`Backtrace::as_backtrace` allocating for the disabled case.** Ruled out —
  `crates/ohno/src/backtrace.rs:65` returns a reference to a
  `static DISABLED_BACKTRACE`. Zero allocation. Correct as written.
* **`is_string_error`'s `TypeId` comparisons** (`crates/ohno/src/core.rs:192-201`).
  Three 128-bit comparisons per construction. Ruled out: this is a
  correctness-preserving discrimination on the construction path, the
  comparisons compile to a handful of instructions, and removing it would
  require either `unsafe` or an API change. The philosophy says preserve
  defensive checks and deprioritise construction-path costs; both point the same
  way here. **Recommendation: do nothing.**
* **`Box<dyn Error>` dynamic dispatch on the source chain.** Considered as
  avoidable dynamic dispatch; ruled out — type-erasing the source is the entire
  point of an interoperable error type, it is the universal ecosystem pattern,
  and the vtable call happens only when the chain is walked (formatting or
  labelling), not on construction or propagation.

---

## Crate: ohno_macros

### Summary

For a proc-macro crate, "performance" has two distinct meanings: the compile
time it imposes on every downstream crate, and the runtime efficiency of the
code it generates. `ohno_macros` has one finding of each kind, plus a structural
observation about `#[enrich_err]` on `async fn`.

The generated-code finding (the `format!` in `Display`) is recorded above as F3
and F4 because that is where its cost lands; it is cross-referenced here rather
than duplicated.

### Findings

#### F10. `syn` is enabled with `extra-traits` in `[dependencies]`, inflating downstream compile time

- **Location:** `crates/ohno_macros/Cargo.toml`, the `syn` entry in
  `[dependencies]`, which enables the `extra-traits` feature. Compare the
  workspace declaration at root `Cargo.toml:207`:
  `syn = { version = "3.0.2", default-features = false }`.
- **Issue:** `extra-traits` makes `syn` derive `Debug`, `Eq`, `PartialEq` and
  `Hash` for its entire AST — several hundred types. That is a large amount of
  generated code to compile, and because Cargo unifies features across the
  dependency graph, enabling it here turns it on for *every* crate in the build
  that uses `syn`, including third-party ones. Any downstream consumer of `ohno`
  pays this in build time.
- **Impact:** Medium on compile time, zero at runtime. Compile time is a real
  cost that the team pays on every CI run and every developer edit-build cycle,
  and this one is paid by consumers who never asked for it.
- **Remediation:** Move `extra-traits` to `[dev-dependencies]` (a separate `syn`
  entry with the feature, used only by the macro crate's own tests, which is
  where `Debug`-printing a `syn` AST is actually useful) and drop it from
  `[dependencies]`. There is a clean in-repo proof this is achievable:
  `crates/data_privacy_macros_impl/Cargo.toml` does **not** enable
  `extra-traits` and works fine. `crates/fundle_macros_impl/Cargo.toml` has the
  same problem as `ohno_macros`.
- **Evidence:** inferred from code reading (manifest comparison across the three
  proc-macro implementation crates in this group). Not empirically verified —
  measuring it requires a build, which the environment cannot do.

#### F11. Generated `Display` materialises a `String` instead of writing to the formatter

- **Location:** `crates/ohno_macros/src/derive_error/display.rs:108-114`.
- **Issue / Impact / Remediation / Evidence:** see **F3** above. Recorded here
  so that a reader auditing `ohno_macros` alone does not miss it: the defect is
  in this crate's codegen, the cost is paid by `ohno`'s users.

#### F12. `#[enrich_err]` on an `async fn` nests an async closure inside the function's future

- **Location:** `crates/ohno_macros/src/enrich_err/mod.rs:47-60`, in particular
  the emitted shape at `:52-60`.
- **Issue:** The macro rewrites the function body to
  `(#asyncness || #body)() #await_suffix .map_err(...)`. When `#asyncness` is
  `async`, this constructs an **async closure**, immediately calls it, and
  awaits the resulting future — all inside the outer `async fn`'s own future.
  The result is an extra state-machine layer: the outer future contains the
  inner future as a field, so the outer future's size is at least the inner
  future's size plus the outer's own state, and each poll traverses one
  additional `Future::poll` frame.
- **Impact:** Low to Medium, and genuinely uncertain without a build. Future
  size matters because futures are frequently boxed (`Box<dyn Future>` in
  trait objects and in `tokio::spawn`), and a larger future means a larger
  allocation and more bytes memcpy'd when the future is moved. The extra poll
  frame is usually inlined away by LLVM at `-O2`; the size growth is less
  reliably eliminated. If `#[enrich_err]` is applied to functions on a
  request-rate async path, this compounds.
- **Remediation:** Emit the body inline and apply the enrichment to the result
  expression, rather than wrapping the body in a closure. Concretely, for the
  async case emit `let __result = async { #body }.await;` — or better, since the
  body is already inside an `async fn`, emit the statements directly and
  transform the return path. The closure wrapper exists to give the `?` operator
  a place to return to; a labelled block (`'a: { ... }`) achieves the same in
  edition 2024 without introducing a nested future.
- **Evidence:** inferred from code reading. **Not verified** — quantifying the
  future-size delta requires compiling both shapes, which the environment
  cannot do. This should be re-measured by anyone with a working build before
  acting on it. `std::mem::size_of_val` on the two futures is the check.

#### F13. `#[derive(Error)]` emits eight impl blocks per error type

- **Location:** `crates/ohno_macros/src/derive_error/mod.rs` — the top-level
  expansion assembles `Display`, `Debug`, `std::error::Error`, `ErrorExt`, the
  `From` impls, the constructors, and the `Enrichable` plumbing.
- **Issue:** Each derived error type expands to a substantial amount of code.
  With the number of error types in this workspace, that is a meaningful
  fraction of the workspace's total compiled LOC, and it is compiled afresh in
  every crate that derives.
- **Impact:** Low. This is what a derive macro is for, and the alternative
  (hand-written impls) is worse on every axis except compile time. Recorded
  because compile time is explicitly in scope for this analysis and because it
  interacts with F10 — the two together are what make `ohno`-derived error
  types expensive to build.
- **Remediation:** No action recommended on the codegen volume itself. The
  actionable part is F10. If build time becomes a measured problem, the
  instrument is `cargo build --timings`, and the lever would be moving the
  rarely-used generated impls (the constructors, say) behind an opt-in
  attribute — but that trades ergonomics for build time and should not be done
  speculatively.
- **Evidence:** inferred from code reading.

### Benchmark coverage

`crates/ohno_macros/benches/` does not exist, and **should not**. A proc-macro
crate's cost is compile time; Criterion measures wall-clock time of a runtime
closure and cannot express "how long does `rustc` take to expand this". The
correct instruments are `cargo build --timings` and `-Zself-profile`, neither of
which belongs in a `benches/` directory.

What *does* warrant benchmarking is the code this crate generates, and that is
covered by the `ohno` recommendations above (derived `Display::fmt`,
`ErrorExt::message()`). Adding those benchmarks under `crates/ohno/benches/`
gives this crate's codegen the coverage it needs without a `benches/` directory
here.

### Considered and ruled out

* **`proc-macro2` / `quote` dependency weight.** Ruled out — they are the
  universal ecosystem baseline for proc macros and there is no lighter
  alternative that is not a deviation requiring justification. The philosophy
  explicitly asks for justification when deviating from ecosystem defaults;
  using them *is* the default.
* **Generated `From` impls forcing allocation.** Checked
  `crates/ohno_macros/src/derive_error/from_impls.rs`; the generated `From`
  bodies delegate to `OhnoCore`'s conversion and add nothing of their own. The
  cost is F1, in `ohno`, not here.
* **Recursion or quadratic behaviour in the macro itself.** Read the expansion
  paths; everything is a single linear pass over the variants and fields. No
  nested iteration over the AST that would make expansion superlinear in the
  number of variants.
* **Generated `Debug` printing the full backtrace.** Considered as a hidden
  cost; ruled out — `Debug` on an error is a diagnostic operation by
  definition, and printing the backtrace when one exists is the desired
  behaviour. Making it cheaper would make it less useful.

---

## Crate: data_privacy

### Summary

This is the group's most performance-sensitive **product** code. Redaction runs
on the telemetry path, which means it runs at request rate — every log line and
every span attribute carrying a classified value passes through it. Unlike
`ohno`'s error paths, there is no "this only happens when something goes wrong"
discount to apply.

The design is largely good: `redacted_debug` / `redacted_display` use
`core::fmt::from_fn` and are genuinely allocation-free for payloads that fit the
stack buffer (measured: 0 allocations for values ≤128 bytes), and
`RedactionEngine::new` calls `shrink()` on its map. But there are five real
costs on the hot path, and the top one — F14 — is paid before any redaction work
is done at all.

### Findings

#### F14. Every redaction hashes two strings before doing any redaction work

- **Location:** `crates/data_privacy/src/redaction_engine.rs:113-121`
  (`Redactor::redact` for `RedactionEngine`) delegating to
  `crates/data_privacy/src/redaction_engine_inner.rs:31-37` (`resolve`); the key
  type is `DataClass` at `crates/data_privacy_core/src/data_class.rs:17-21`.
- **Issue:** The policy lookup is an `FxHashMap<DataClass, RedactionPolicy>::get`.
  `DataClass` is a **48-byte struct of two `Cow<'static, str>`** with a derived
  `Hash`, so hashing the key means hashing two string *contents* — walking both
  strings byte by byte — on every single redaction. On a hit, the derived
  `PartialEq` then performs two string comparisons to confirm the bucket match.
  All of this happens before the redactor is even selected, let alone invoked.
- **Impact:** Medium, and the highest-impact product-code finding in this group.
  The cost is proportional to the length of the taxonomy and class names, which
  are developer-chosen and tend to be descriptive (`"contoso.privacy"`,
  `"EndUserPseudonymousIdentifiers"` — the crate's own examples run to 30+
  characters), so this is on the order of 60–80 bytes hashed per redaction. At
  request rate with several classified fields per request, that is a real
  fraction of the redaction path's total cost, spent on identity resolution
  rather than on redacting.
- **Remediation:** Intern the class identity so the map key is a machine word
  rather than two strings. Two viable shapes, in increasing order of intrusiveness:
  (a) precompute and cache the hash inside `DataClass` at construction — since
  `DataClass::new` is `const` and the `#[taxonomy]` macro constructs classes in
  `const` context, the hash can be computed at compile time, turning `Hash` into
  a single `u64` write and leaving `PartialEq` as the only string work;
  (b) replace the map key with a `u64` id derived from the pointer identity of
  the `&'static DataClass` that `#[taxonomy]` already promotes to a static (see
  `crates/data_privacy_macros_impl/src/taxonomy.rs:95`, which emits
  `const { &DataClass::new(...) }`). Option (a) is the surgical one and is the
  recommendation; option (b) is faster but changes the public key type.
- **Evidence:** inferred from code reading, with the 48-byte `DataClass` layout
  **empirically verified** (layout replica: `DataClass { Cow, Cow }` = 48 bytes,
  align 8; `Cow<'static, str>` = 24 bytes). The hashing cost itself was not
  measured — measuring it requires `rustc-hash`, which is not available offline.

#### F15. `redacts()` and `redact()` each perform an independent lookup, so a guarded call hashes twice

- **Location:** `crates/data_privacy/src/redaction_engine.rs:109-111`
  (`redacts`) and `:113-121` (`redact`).
- **Issue:** Both methods call `resolve` independently. The natural defensive
  call site — `if engine.redacts(&class) { engine.redact(&class, ...) }` —
  therefore pays the F14 cost twice for one redaction.
- **Impact:** Medium, conditional on call sites actually using the guard. It
  doubles the F14 cost where it applies, which is why it is graded alongside
  F14 rather than below it.
- **Remediation:** Offer a combined entry point that resolves once and returns
  the resolved policy (or an `Option<&dyn Redactor>`), so a caller that wants to
  branch on "will this be redacted" and then redact does one lookup. Keep
  `redacts()` for the cases that genuinely only want the boolean. This is an
  additive API change — no existing caller breaks.
- **Evidence:** inferred from code reading.

#### F16. Redaction formats the payload in full and then discards it, under the default fallback policy

- **Location:** `crates/data_privacy/src/sensitive.rs:81-137` (the `Display` and
  `Debug` impls for `Sensitive<T>`), the identical generated code at
  `crates/data_privacy_macros_impl/src/classified.rs:104-156`, the default
  fallback at `crates/data_privacy/src/redaction_engine_inner.rs:76`
  (`SimpleRedactorMode::Erase`), and the erase implementation at
  `crates/data_privacy/src/redactors/simple_redactor.rs:78-81`.
- **Issue:** Neither `Sensitive`'s formatting impls nor the generated
  `#[classified]` `RedactedDisplay` / `RedactedDebug` ever consult
  `redactor.redacts()`. They unconditionally format the payload into a buffer
  and hand the resulting string to the redactor. The **default fallback
  redactor** is `SimpleRedactorMode::Erase`, whose implementation writes
  **nothing** at all. So in the default configuration, the entire `Display` or
  `Debug` formatting of every unclassified-by-policy value is performed and then
  thrown away.
- **Impact:** Medium. This is the default configuration, so it is what a
  consumer gets before they tune anything, and formatting is the expensive part
  of the operation — for a payload over 128 bytes it also triggers a heap
  allocation (measured: 2 allocs / 400 bytes) purely to build a string nobody
  reads.
- **Remediation:** Query the policy before formatting. If the resolved policy is
  an erasing or suppressing one, write the fixed output directly and skip
  payload formatting entirely. This is a fast-path addition, not a
  behaviour change: the rendered output is byte-identical. It composes well with
  the F15 remediation — a single `resolve` that returns the policy lets the
  formatting impl check "does this policy need the payload?" for free.
- **Evidence:** the wasted-formatting mechanism is inferred from code reading;
  the allocation it wastes is **empirically verified** — replica measurement of
  `Sensitive` formatting gave `allocs=0 bytes=0` for a payload ≤128 bytes and
  `allocs=2 bytes=400 deallocs=2` for a payload >128 bytes.

#### F17. 128 stack bytes are zero-initialised on every redacted format call, then immediately overwritten

- **Location:** `crates/data_privacy/src/sensitive.rs:84` and `:117`
  (`let mut local_buf = [0u8; STACK_BUFFER_SIZE];`, with
  `STACK_BUFFER_SIZE = 128` at `crates/data_privacy/src/sensitive.rs:9`); the
  same pattern is generated at
  `crates/data_privacy_macros_impl/src/classified.rs:113` and `:140`.
- **Issue:** The buffer is created zeroed and then written through a cursor that
  overwrites exactly the bytes that are subsequently read. The zeroing is dead
  work — 128 bytes of `memset` per formatted value, times two code paths
  (`Display` and `Debug`), times every classified field on every log line.
- **Impact:** Low to Medium. 128 bytes of `memset` is on the order of a handful
  of vector stores and LLVM may in some cases prove the zeroing dead and remove
  it — but it cannot always, because the buffer is passed to a cursor whose
  writes it cannot fully track through the `fmt::Write` abstraction. At request
  rate this is worth measuring.
- **Remediation:** Use `MaybeUninit<[u8; 128]>` with the cursor tracking the
  initialised prefix, and slice only the initialised region. Note that this
  would **not** be the crate's first `unsafe`: `from_utf8_unchecked` already
  appears at `crates/data_privacy/src/sensitive.rs:96` and `:129`, and in the
  generated code at `crates/data_privacy_macros_impl/src/classified.rs:123` and
  `:150`. The safety argument for the `MaybeUninit` version is the same shape as
  the one already being made for `from_utf8_unchecked` and is if anything easier
  to state. Alternatively, measure first: if LLVM already elides the zeroing at
  `-O2`, do nothing.
- **Evidence:** inferred from code reading. The zeroing's *cost* was not
  isolated by the allocation probe (it is a stack operation, invisible to a
  `GlobalAlloc` counter) — this needs a Callgrind run or a microbenchmark, which
  the environment cannot provide.
- **Philosophy note:** Mildly conflicting — the philosophy prefers idiomatic
  Rust and surgical changes, and reaching for `MaybeUninit` is a step away from
  idiomatic. Mitigated by the crate's existing `unsafe` in the same functions.
  **Measure before acting** is the right first move here.

#### F18. `Sensitive<T>` embeds a 48-byte `DataClass` by value

- **Location:** `crates/data_privacy/src/sensitive.rs:16-19`.
- **Issue:** The wrapper stores a `DataClass` inline. Since `DataClass` is two
  `Cow`s, `Sensitive<T>` is 48 bytes larger than `T` regardless of what `T` is.
- **Impact:** Medium. `Sensitive<u8>` measured **56 bytes** — seven times the
  payload. Every `Sensitive` value moved, returned, stored in a collection or
  captured in a future carries 48 bytes of class identity that is almost always
  a pointer to a compile-time constant. In a struct with several sensitive
  fields this multiplies.
- **Remediation:** Store `&'static DataClass` (8 bytes) instead of `DataClass`.
  The `#[taxonomy]` macro **already** promotes classes to statics — see
  `crates/data_privacy_macros_impl/src/taxonomy.rs:95`, which emits
  `const { &DataClass::new(...) }` for the `AsRef<DataClass>` impls — so the
  `&'static` is already available at every construction site the macro
  generates. This would shrink `Sensitive<u8>` from 56 to 16 bytes. The cost is
  that dynamically-constructed classes (deserialised ones) would need interning
  or a `Cow<'static, DataClass>`-shaped compromise. Worth doing; it is a
  contained change to one struct plus its constructor.
- **Evidence:** empirically verified — layout replica measured
  `Sensitive<u8>` = 56 bytes, `Sensitive<String>` = 72 bytes,
  `Sensitive<()>` = 48 bytes, `DataClass` = 48 bytes. Replica, not the real
  type, but field-for-field identical.

#### F19. Redactors route single-string writes through the full formatting machinery

- **Location:** `crates/data_privacy/src/redactors/simple_redactor.rs:88`,
  `:99`, `:123`; `crates/data_privacy/src/redactors/xxh3_redactor.rs:60`;
  `crates/data_privacy/src/redactors/rapidhash_redactor.rs:39`.
- **Issue:** These sites use `write!(output, "{}", s)` where `s` is already a
  `&str`. `write!` with a format string constructs an `Arguments`, invokes
  `fmt::write`, which walks the format-string pieces and dispatches through the
  `Display` vtable for `str`. `output.write_str(s)` does the same job with a
  single direct call.
- **Impact:** Low individually — LLVM optimises the trivial `{}` case reasonably
  well — but this is on the redaction hot path and the fix is a mechanical
  one-token change per site with zero risk and zero behaviour difference.
- **Remediation:** Replace `write!(output, "{}", s)` with
  `output.write_str(s)?` at each of the five sites. Idiomatic, surgical,
  ecosystem-standard.
- **Evidence:** inferred from code reading.

#### F20. Off-by-one boundary sends exactly-`ASTERISKS.len()` values down the slow path

- **Location:** `crates/data_privacy/src/redactors/simple_redactor.rs:98` and
  `:111`.
- **Issue:** The fast path is guarded by `len < ASTERISKS.len()`, where
  `ASTERISKS` is a 120-character constant. A value of exactly 120 bytes fails
  the test and falls into the per-character loop, even though the constant slice
  can serve it exactly.
- **Impact:** Low — it affects one input length out of every 120. Reported
  because it is a one-character fix (`<` → `<=`) and because the analogous
  boundary is handled correctly elsewhere in the crate: the stack-buffer path in
  `sensitive.rs` uses `<=` against `STACK_BUFFER_SIZE`, and there is even a test
  comment at `crates/data_privacy/src/sensitive.rs:190` noting that a
  128-byte output specifically tests the `<=` boundary. The inconsistency
  between the two suggests the `<` here is an oversight rather than a decision.
- **Remediation:** Change `<` to `<=` at both sites, and add the boundary-length
  case to the redactor tests (test authoring is not this worker's remit; flagged
  for the test coder).
- **Evidence:** inferred from code reading.

#### F21. Only one `#[inline]` in the crate, and it is not on the hot path

- **Location:** the sole `#[inline]` is at
  `crates/data_privacy/src/redactors/mod.rs:13` on `u64_to_hex_array`. Nothing
  on `RedactionEngine::redact`, `RedactionEngine::redacts`,
  `Sensitive::data_class`, or the `Redactor` trait's small methods.
- **Issue:** `redact` and `redacts` are non-generic exported functions on a
  request-rate path, called from other crates. Without `#[inline]` and without
  LTO in `[profile.release]`, a consumer cannot inline them.
- **Impact:** Low. The bodies are not trivial (they do a map lookup and a
  dispatch), so inlining buys less here than it would for a field accessor; the
  main gain would be enabling the caller's optimiser to see through to the
  `resolve` call and combine it with F15's guard. Reported for completeness.
- **Remediation:** Narrow set only: the trivial accessors
  (`Sensitive::data_class`, `Sensitive::payload` and the equivalents on
  generated `#[classified]` types). Leave `redact`/`redacts` alone — they are
  large enough that `#[inline]` would be a code-size bet without measurement,
  and the philosophy says be judicious.
- **Evidence:** inferred from code reading (grep census).
- **Philosophy note:** Same tension as F9 — cannot be validated by a benchmark
  in this repository because `[profile.bench]` uses fat LTO.

### Benchmark coverage

`crates/data_privacy/benches/` does not exist. This is the group's second
highest-value gap after `ohno`, and arguably the more urgent of the two on a
frequency basis, since redaction runs on the success path of every request while
error construction runs only on failures.

Warranted:

1. **`RedactionEngine::redact` on a policy hit and on a fallback miss**, with
   short (`"pii"`/`"name"`) and long (30+ character) class names as separate
   inputs. The two name lengths are what make F14 visible; averaging them would
   hide it. Pair with a Callgrind `_cg.rs` file — hashing is branch-light and
   instruction counts will be stable.
2. **Redacted `Display` of a `Sensitive<String>` at payload lengths 16, 128 and
   256 bytes.** 128 straddles the stack-buffer cliff; 256 forces the heap path.
   Three inputs, not an average. This benchmark validates F16 and F17
   simultaneously.
3. **Each concrete redactor in isolation** (`SimpleRedactor` in each mode,
   `Xxh3Redactor`, `RapidhashRedactor`) on a fixed input, so that a policy
   choice can be made on numbers. This is the crate's most decision-relevant
   benchmark for a consumer.

Not warranted: `RedactionEngine::new` and the builder — construction path, run
once at startup, explicitly deprioritised by `docs/performance.md`.

### Considered and ruled out

* **`redacted_debug` / `redacted_display` allocating.** Ruled out empirically —
  `crates/data_privacy/src/redaction_engine.rs:85-100` uses `core::fmt::from_fn`,
  and the probe measured **0 allocations** for payloads ≤128 bytes. This is
  excellent design and should be preserved; the `from_fn` approach is exactly
  the right ecosystem pattern for a lazily-rendered value.
* **`RedactionEngine::new` not shrinking its map.** Ruled out — it calls
  `inner.shrink()` at `crates/data_privacy/src/redaction_engine.rs:74`. Already
  correct.
* **`RedactionPolicy` being oversized.** Measured 16 bytes
  (`Redact(Box<dyn>) | Suppressed`) — a thin pointer plus discriminant, niche
  candidates already exhausted. Nothing to gain.
* **`Box<dyn Redactor>` dynamic dispatch.** Considered as avoidable dynamic
  dispatch. Ruled out — the redactor is chosen at runtime from configuration by
  design; that is the feature. Monomorphising it would require the policy to be
  known at compile time, which defeats the purpose of a configurable engine.
* **`SimpleRedactorMode` at 32 bytes.** Considered; ruled out — it lives inside
  a `Box`ed redactor, constructed once at configuration time, never on the hot
  path.
* **Per-access indirection in the wrappers.** Checked: `Sensitive`'s accessors
  are direct field projections with no indirection; the classification is stored
  inline (which is F18's problem, but it means access is a direct read, not a
  pointer chase). No per-access indirection to remove.

---

## Crate: data_privacy_core

### Summary

A small crate holding `DataClass`, `Redacted*` traits and `Classified`. Its
performance significance is entirely out of proportion to its size, because
`DataClass` is the key type on `data_privacy`'s hot path — F14 and F18 are both
consequences of decisions made here.

### Findings

#### F22. `DataClass` is a 48-byte two-`Cow` struct used as a hash-map key on the hot path

- **Location:** `crates/data_privacy_core/src/data_class.rs:17-21` (the struct
  and its derived `Hash` / `PartialEq` / `Eq`).
- **Issue:** The type models a class identity as two owned-or-borrowed strings.
  That is the right *semantic* model and a poor *lookup key*: hashing it walks
  both strings, comparing it compares both strings, and copying it moves 48
  bytes. Everything F14 and F18 complain about traces to this one decision.
- **Impact:** Medium — it is the root cause of the group's top product-code
  finding, and it is graded here rather than doubled because F14 and F18 already
  carry the downstream cost.
- **Remediation:** Add a cached hash computed in the `const fn` constructor, so
  `Hash` becomes a single `u64` write while `PartialEq` keeps its exact
  semantics. This preserves the semantic model, preserves the public API,
  preserves every defensive property, and is confined to one file — the most
  surgical available fix for F14. A more aggressive interning scheme is possible
  but is an architectural change and is not recommended without measurement.
- **Evidence:** empirically verified for the layout (48 bytes, align 8, via
  replica); the hash cost is inferred from code reading.

#### F23. `DataClass::clone` allocates twice when the class was deserialised

- **Location:** `crates/data_privacy_core/src/data_class.rs:17-21` (derived
  `Clone`).
- **Issue:** Cloning a `DataClass` whose `Cow`s are `Borrowed` — the normal case
  for a `#[taxonomy]`-generated class — is free. Cloning one whose `Cow`s are
  `Owned` — which is what deserialisation produces — allocates two `String`s.
- **Impact:** Low, and conditional: it only bites configuration-driven or
  deserialised classes, which are a startup-path concern rather than a
  request-path one. Reported because the two cases have wildly different costs
  and nothing in the API signals which one a caller has.
- **Remediation:** No code change. Document the distinction on `DataClass` so a
  consumer building classes from configuration knows to construct them once and
  reuse, rather than cloning per use. F18's `&'static DataClass` remediation
  would make this moot for the macro-generated path.
- **Evidence:** empirically verified — replica measurement:
  `DataClass::clone` with both `Cow`s `Borrowed` = `allocs=0 bytes=0`; with both
  `Owned` = `allocs=2 bytes=15 deallocs=2`.

#### F24. No `#[inline]` on the accessors that the redaction path calls

- **Location:** `crates/data_privacy_core/src/data_class.rs:44` (`taxonomy()`)
  and `:50` (`name()`); also `Classified::data_class` in
  `crates/data_privacy_core/src/classified.rs`. The crate contains **zero**
  `#[inline]` annotations.
- **Issue:** These are trivial non-generic exported functions — each returns a
  `&str` from a field — called from another crate on the redaction path. Without
  `#[inline]` and without release-profile LTO, the call cannot be inlined
  downstream, and the call overhead exceeds the body.
- **Impact:** Low, but this is the textbook case for `docs/performance.md`'s
  rule 1: small, non-generic, exported, on a hot path.
- **Remediation:** Annotate `taxonomy()`, `name()` and `Classified::data_class`.
  Three annotations, zero risk, no API change.
- **Evidence:** inferred from code reading (grep census).
- **Philosophy note:** Same benchmark-blindness caveat as F9 and F21 — the
  repository's `[profile.bench]` fat LTO means no benchmark here can demonstrate
  the effect. Justified on API shape, which the philosophy permits for exactly
  this category of function.

### Benchmark coverage

`crates/data_privacy_core/benches/` does not exist. One benchmark is warranted:
**`DataClass` hashing and equality in isolation**, at short and long name
lengths. This is the measurement that would turn F14 and F22 from "reasoned" to
"quantified", and it is the cheapest possible benchmark to write — no engine, no
redactor, just the key type. A Callgrind pairing is appropriate: hashing is
deterministic and branch-light.

Nothing else in this crate warrants coverage. The traits are dispatch surfaces
with no bodies worth measuring, and `Classified` is a marker.

### Considered and ruled out

* **`Redacted` / `RedactedDisplay` / `RedactedDebug` trait dispatch.** Ruled out
  — these are statically dispatched at every call site examined; there is no
  vtable on the path.
* **`RedactedToString` allocating.** It returns a `String`; allocating is
  inherent to the signature. Callers who want zero allocation already have
  `redacted_display` (the `from_fn` path), which is documented. Nothing to fix.
* **`DataClass::new` being non-`const`.** Checked — it *is* `const fn`, which is
  what allows `#[taxonomy]` to promote classes to statics. Correct as written
  and load-bearing for F18's remediation.

---

## Crate: data_privacy_macros

### Summary

A 47-line re-export shim: it declares the proc-macro entry points and forwards
every one of them to `data_privacy_macros_impl`. It contains no logic.

### Findings

**No performance issues found.**

The shim adds one crate to the dependency graph, which costs a small fixed
amount of build time, but the split is the standard and correct structure for a
proc-macro crate: it lets the implementation crate be a normal library that can
be unit-tested, which a `proc-macro = true` crate cannot easily be. This is an
ecosystem convention followed correctly and there is no justification needed for
it.

### Benchmark coverage

`crates/data_privacy_macros/benches/` does not exist and should not. There is no
runtime code to benchmark — every function in the crate is a proc-macro entry
point that runs in `rustc`.

### Considered and ruled out

* **The extra crate in the graph.** Considered as a build-time cost; ruled out —
  it is a handful of forwarding functions, compiles in negligible time, and
  removing the split would cost testability. The trade is correct as made.
* **`syn`/`quote` feature selection.** The shim does not pull `syn` at all;
  the parsing dependency lives in the impl crate where it belongs. Correct.
* **Re-export indirection at runtime.** None exists — proc-macro forwarding
  happens at expansion time, not at runtime.

---

## Crate: data_privacy_macros_impl

### Summary

Generates the `#[classified]`, `#[taxonomy]` and derive machinery. Its generated
code is on `data_privacy`'s hot path, so this crate's codegen quality is a
runtime concern for every consumer.

There is one clear positive worth recording: this crate does **not** enable
`syn/extra-traits`, unlike `ohno_macros` and `fundle_macros_impl`. That makes it
the in-repo proof that F10's remediation is achievable.

### Findings

#### F25. Generated `#[classified]` formatting duplicates the 128-byte zeroed-buffer pattern

- **Location:** `crates/data_privacy_macros_impl/src/classified.rs:104-156`, in
  particular the buffer declarations at `:113` and `:140` and the
  `from_utf8_unchecked` calls at `:123` and `:150`.
- **Issue:** The generated `RedactedDisplay` / `RedactedDebug` impls are a
  textual copy of `Sensitive`'s hand-written impls, including the
  `[0u8; STACK_BUFFER_SIZE]` zeroing (F17) and the unconditional payload
  formatting under an erasing policy (F16). Because it is generated, the pattern
  is instantiated once per `#[classified]` type in the workspace rather than
  once in total.
- **Impact:** Medium — it multiplies F16 and F17 across every classified type,
  and it means fixing those two findings in `sensitive.rs` alone would leave the
  generated path unfixed. It also inflates compiled code size linearly in the
  number of classified types.
- **Remediation:** Extract the shared formatting body into a public helper
  function in `data_privacy` (or `data_privacy_core`) and have the macro emit a
  call to it rather than a copy of it. This deduplicates the codegen, shrinks
  compiled size, and — importantly — means the F16/F17 fixes need to be made in
  exactly one place. This is the highest-leverage change in this crate: it does
  not itself make anything faster, but it makes two other findings fixable once
  instead of twice.
- **Evidence:** inferred from code reading (side-by-side comparison of
  `crates/data_privacy/src/sensitive.rs:81-137` with
  `crates/data_privacy_macros_impl/src/classified.rs:104-156`).

#### F26. `#[taxonomy]`-generated `data_class()` returns a 48-byte `DataClass` by value while a zero-cost `&'static` is right there

- **Location:** `crates/data_privacy_macros_impl/src/taxonomy.rs:95` (the
  `AsRef<DataClass>` arms, which correctly emit
  `const { &DataClass::new(...) }`), `:109-111` (the `classify_*` helpers) and
  `:123` (`data_class()`).
- **Issue:** The macro emits *both* a zero-cost `&'static DataClass` path (via
  `AsRef`, backed by a promoted `const` static) and a by-value `data_class()`
  that constructs a fresh 48-byte `DataClass` per call. The by-value one is the
  discoverable, documented API, so it is the one call sites use — including
  `classify_*`, which constructs a fresh `DataClass` for every `Sensitive` it
  wraps.
- **Impact:** Low to Medium. In many cases LLVM will constant-fold the
  `const fn` construction of literal `Cow::Borrowed`s into a static, making the
  by-value path free. But that is a hope, not a guarantee, and it evaporates as
  soon as the value crosses a non-inlined function boundary — which is exactly
  what happens when it is passed to `Sensitive::new` in another crate with no
  `#[inline]` (F21).
- **Remediation:** Steer callers to the `&'static` path: have `classify_*` use
  the `AsRef` arm internally rather than calling `data_class()`, and document
  `AsRef<DataClass>` as the preferred accessor. If F18's `&'static DataClass`
  change is made, `data_class()` can return the reference directly and the
  question disappears.
- **Evidence:** inferred from code reading. The `const { &DataClass::new(...) }`
  promotion at `:95` was verified by reading the emitted token stream in the
  macro source; the 48-byte by-value cost is empirically verified from the
  layout probe.

#### F27. Generated code is emitted per type rather than delegating to shared helpers

- **Location:** `crates/data_privacy_macros_impl/src/classified.rs:104-156` and
  `crates/data_privacy_macros_impl/src/taxonomy.rs:95-125`.
- **Issue:** Both macros emit substantial inline bodies per annotated item. This
  is a compile-time and code-size cost that scales with the number of classified
  types and taxonomies in a consumer's codebase.
- **Impact:** Low. Recorded because compile-time cost is in scope and because it
  is the same underlying issue as F25 seen from the build-time rather than the
  runtime angle — the same remediation fixes both.
- **Remediation:** As F25 — emit calls to shared helpers.
- **Evidence:** inferred from code reading.

### Benchmark coverage

`crates/data_privacy_macros_impl/benches/` does not exist and should not — see
the `ohno_macros` reasoning; proc-macro cost is compile time and Criterion is
the wrong instrument.

The generated code, however, badly needs coverage, and the right home for it is
`crates/data_privacy/benches/`: a benchmark of a `#[classified]` type's redacted
`Display` alongside the hand-written `Sensitive<T>` equivalent would confirm
that the two paths perform identically (they should — the code is a copy) and
would catch a divergence introduced by fixing F16/F17 in only one of them.

### Considered and ruled out

* **`syn` feature bloat.** Ruled out and recorded as a **positive**: this crate
  does not enable `extra-traits`, unlike `ohno_macros` and `fundle_macros_impl`.
  It is the existence proof that F10 is fixable.
* **`AsRef<DataClass>` arms costing anything.** Ruled out — `const { &... }` is
  a promoted static; the arm compiles to loading a constant address. Zero cost,
  correct design, should be the documented path (see F26).
* **Quadratic expansion in the number of variants.** Read the generation loops;
  they are single linear passes. No superlinear behaviour.
* **`#[taxonomy]` generating a runtime registry or lazy static.** Checked — it
  does not; everything is `const`. Good.

---

## Crate: fundle

### Summary

The runtime crate is 147 lines and is essentially documentation plus
re-exports — there is no logic in it. Its performance story is entirely about
what `fundle_macros_impl` generates, and about one claim its own documentation
makes that the generated code does not honour.

### Findings

#### F28. The crate documents itself as providing "zero-cost abstractions" while two of its three macros generate deep clones

- **Location:** `crates/fundle/src/lib.rs:9` (the claim); the generated clones
  are at `crates/fundle_macros_impl/src/deps.rs:69` and
  `crates/fundle_macros_impl/src/newtype.rs:64`; the claim is repeated in the
  macro documentation at `crates/fundle_macros/src/lib.rs:174-175` and `:236`.
- **Issue:** `#[bundle]` genuinely is zero-cost (see the positives below). But
  `#[fundle::deps]` and `#[newtype]` both emit clones of every dependency, so
  the crate-level claim is true of one macro out of three and false of the other
  two. A consumer reading the crate docs will reasonably assume that reaching
  for `#[deps]` is free, and it is not.
- **Impact:** Low as a runtime cost — the clones happen when a dependency
  bundle is destructured, which is a wiring/construction operation, and
  `docs/performance.md` explicitly deprioritises construction-path costs. Graded
  Low rather than Medium for exactly that reason. It is reported because an
  incorrect zero-cost claim in documentation causes consumers to make wrong
  decisions, which is a performance defect propagated by prose rather than code.
- **Remediation:** Either narrow the documentation claim to `#[bundle]` (cheap,
  honest, and the recommended action), or make the generated code match the
  claim (see F33/F34). Doing the documentation fix does not preclude the code
  fix later.
- **Evidence:** inferred from code reading.

### Benchmark coverage

`crates/fundle/benches/` does not exist, and there is nothing here to benchmark
— the crate has no runtime logic of its own. The generated code's only
measurable cost is the F33/F34 clone, which is a construction-path cost that the
philosophy deprioritises; writing a benchmark for it would produce a number
nobody should act on. **No benchmark recommended.**

### Considered and ruled out

* **Re-export indirection.** Costs nothing at runtime; re-exports are resolved
  at name-resolution time.
* **`fundle`'s own dependencies.** It has essentially none beyond the macro
  re-export. Nothing to trim.

---

## Crate: fundle_macros

### Summary

The proc-macro entry-point shim for `fundle_macros_impl`, structurally identical
to `data_privacy_macros`. It carries the crate's user-facing documentation.

### Findings

**No performance issues found** in the shim itself.

The one thing attributable to this crate is documentation accuracy: the
zero-cost claim discussed in F28 is restated here at
`crates/fundle_macros/src/lib.rs:174-175` and `:236`, on the specific macros
that do not honour it. Fixing F28 means fixing the wording here too.

### Benchmark coverage

`crates/fundle_macros/benches/` does not exist and should not — proc-macro entry
points have no runtime code.

### Considered and ruled out

* **The shim/impl split.** Standard, correct, testability-motivated. Same
  reasoning as `data_privacy_macros`.
* **Feature flags.** The shim declares none that affect generated code.

---

## Crate: fundle_macros_impl

### Summary

Where fundle's real codegen lives. `#[bundle]` is a genuine success — the
generated `AsRef` impls return references and the whole abstraction compiles
away. `#[deps]` and `#[newtype]` are not: both clone.

### Findings

#### F29. `#[fundle::deps]` deep-clones every field it extracts

- **Location:** `crates/fundle_macros_impl/src/deps.rs:69`.
- **Issue:** For each field the macro emits
  `<T as AsRef<Ty>>::as_ref(&value).to_owned()`. The `as_ref` half is free — it
  is a reference projection into the bundle. The `.to_owned()` then deep-copies
  the referent. For a dependency that is an `Arc<dyn Service>` this is a
  refcount bump and harmless; for one that is a `String`, a `Vec`, or a
  configuration struct it is a full heap copy, per field, per extraction.
- **Impact:** Low to Medium depending on what consumers put in their bundles.
  Graded Low overall because dependency extraction is a wiring operation that
  happens at construction, and the philosophy explicitly deprioritises
  construction-path costs. It rises to Medium for any consumer who extracts deps
  per request rather than per process — a pattern the current API does not
  discourage.
- **Remediation:** Emit borrows where the target type permits it, cloning only
  when the destructured binding genuinely needs ownership. Since the macro knows
  the field types syntactically it cannot always decide this, so the honest fix
  is an opt-in attribute (`#[deps(by_ref)]`) plus documentation of when each is
  appropriate. Do **not** silently change the ownership semantics of an existing
  macro — that would break callers.
- **Evidence:** inferred from code reading.

#### F30. `#[newtype]` clones the wrapped value on construction

- **Location:** `crates/fundle_macros_impl/src/newtype.rs:64`.
- **Issue:** Emits `Self(x.as_ref().clone())`. The newtype takes a reference and
  then owns a copy, so wrapping a value in a newtype costs a full clone of it
  even when the caller had ownership to give.
- **Impact:** Low — construction path, same reasoning as F29.
- **Remediation:** Provide an owning constructor alongside the cloning one
  (`impl From<T> for TheNewtype` taking `T` by value), so a caller with
  ownership can hand it over without a copy. Additive, breaks nobody.
- **Evidence:** inferred from code reading.

#### F31. `syn` with `extra-traits` in `[dependencies]`

- **Location:** `crates/fundle_macros_impl/Cargo.toml`.
- **Issue / Impact / Remediation:** identical to **F10** in `ohno_macros` —
  `extra-traits` derives `Debug`/`Eq`/`PartialEq`/`Hash` across `syn`'s entire
  AST and, through Cargo's feature unification, imposes that on every crate in
  the build. `data_privacy_macros_impl` demonstrates it is unnecessary.
- **Evidence:** inferred from code reading (manifest comparison).

#### F32. Generated bundle plumbing is O(N) impls each carrying N type parameters

- **Location:** `crates/fundle_macros_impl/src/bundle.rs:540-620`
  (`generate_select_macro` and `generate_builder_export_impls`).
- **Issue:** For an N-field bundle, the macro emits on the order of N impl
  blocks, each generic over N type parameters. The total type-checking work is
  therefore superlinear in bundle size.
- **Impact:** Low, and purely compile-time — there is no runtime cost, since the
  impls are monomorphised away. It becomes noticeable only for large bundles.
  Recorded because compile time is in scope and because a consumer with a
  30-field bundle would feel it.
- **Remediation:** No action recommended without a measurement. If a consumer
  reports slow builds on a large bundle, the lever is to generate a single impl
  parameterised over a tuple rather than N impls. That is a substantial codegen
  rewrite and should not be done speculatively.
- **Evidence:** inferred from code reading.

### Benchmark coverage

`crates/fundle_macros_impl/benches/` does not exist and should not — proc-macro
cost is compile time. The generated code's costs (F29, F30) are
construction-path and deprioritised. **No benchmark recommended for this crate.**

If bundle sizes become a build-time problem, the instrument is
`cargo build --timings` on a synthetic large-bundle crate, not Criterion.

### Considered and ruled out

* **`#[bundle]` itself.** Ruled out and recorded as a strong **positive**: the
  generated `AsRef` impls at `crates/fundle_macros_impl/src/bundle.rs:133`,
  `:382`, `:462`, `:492` and `:572` return references with no copying, and the
  bundle struct is a plain aggregate. The abstraction genuinely compiles away.
  This is the part of fundle that earns the "zero-cost" description.
* **The builder being larger than the bundle it builds.** The builder holds
  `Option<T>` per field, so it is larger than the finished struct. Ruled out —
  it exists only during construction and is consumed by `build()`. This is a
  construction-path/teardown cost, explicitly deprioritised by
  `docs/performance.md`.
* **`self.#field_name.as_ref().unwrap()` at `bundle.rs:382`.** Considered as an
  avoidable check; ruled out — it is a defensive runtime check that turns a
  missing-field programming error into a clean panic. The philosophy says
  preserve defensive checks. **Recommendation: do nothing.**
* **Dynamic dispatch in the generated accessors.** None found — all generated
  `AsRef` impls are concrete and statically dispatched.

---

## Crate: recoverable

### Summary

The cleanest crate in the group. `RecoveryInfo` is a small POD-like struct, all
eight public functions are `const fn`, `Display` writes `&'static str` directly
to the formatter with no allocation, and the `From<io::ErrorKind>` conversion is
a match that compiles to a jump table.

### Findings

#### F33. All eight public `const fn` accessors lack `#[inline]`

- **Location:** `crates/recoverable/src/lib.rs:186`, `:220`, `:248`, `:279`,
  `:317`, `:341`, `:386`, `:485`. The crate contains **zero** `#[inline]`
  annotations.
- **Issue:** These are the archetypal rule-1 case: tiny, non-generic, exported,
  `const fn` accessors on a type whose entire purpose is to be inspected by
  other crates' retry loops. `RecoveryKind::as_str` returns a `&'static str`
  from a match; the delay accessors return a field. Without `#[inline]`, a
  downstream crate compiled without LTO makes a real call for each.
- **Impact:** Low in absolute terms — a handful of instructions each — but the
  call sites are retry-decision loops, which are exactly where a caller is
  making a per-attempt decision. Reported because this crate is the clearest
  instance of the pattern in the group: eight functions, all trivial, all
  exported, none annotated.
- **Remediation:** Annotate all eight. They are `const fn` already, so the
  bodies are guaranteed small and there is no code-size risk. This is the
  narrowest, lowest-risk `#[inline]` recommendation in this document.
- **Evidence:** inferred from code reading (grep census of
  `crates/recoverable/src/`).
- **Philosophy note:** Same benchmark-blindness caveat as F9, F21 and F24 — the
  repository's fat-LTO bench profile cannot demonstrate the effect. Justified on
  API shape, which `docs/performance.md` rule 1 sanctions for precisely this
  category.

Beyond F33, **no performance issues were found** in this crate.

### Benchmark coverage

`crates/recoverable/benches/` does not exist. One benchmark is warranted, and
only just: **`RecoveryInfo::from(io::ErrorKind)`** across a representative
spread of kinds. The value is not that anyone expects it to be slow — it is a
jump table — but that it is the crate's only real operation, it is on the retry
decision path, and a regression (someone replacing the match with a lookup that
allocates, say) would otherwise be invisible. Cheap to write, cheap to run.

A Callgrind pairing is appropriate and would be more informative than the
Criterion file: the operation is far too fast for stable wall-clock measurement
and instruction counts are exactly the right resolution.

Nothing else warrants coverage. The accessors are too trivial to benchmark
meaningfully, and `Display` writing a `&'static str` has no interesting
behaviour.

### Considered and ruled out

* **`RecoveryInfo` layout.** `{ kind: RecoveryKind, delay: Option<Duration> }`
  at `crates/recoverable/src/lib.rs:107-110` — compact, `Copy`-friendly, no
  boxed variant, no oversized enum arm. Nothing to gain.
* **`From<ErrorKind>` at `crates/recoverable/src/io.rs:58-84`.** A dense match
  over a `#[non_exhaustive]` C-like enum; LLVM lowers this to a jump table or a
  small lookup. Correct as written.
* **`Display` allocating.** Ruled out — `RecoveryKind::as_str` returns
  `&'static str` and `Display` uses `f.write_str`. Zero allocation. This is
  exactly the pattern F3 recommends for `ohno`'s generated `Display`, and it is
  worth noting that the workspace already has the right pattern in-house.
* **Dynamic dispatch.** None in the crate.

---

## Crate: testing_aids

### Summary

The question this crate had to answer is "does any of it reach a release build",
and the answer is a clean **no**. This was verified exhaustively rather than
assumed, and the result is recorded here as a positive finding because it is the
kind of property that quietly regresses.

### Findings

#### F34. Heavy real `[dependencies]` inflate test build time (but not release builds)

- **Location:** `crates/testing_aids/Cargo.toml` — the `[dependencies]` section
  carries `opentelemetry_sdk`, `tracing-subscriber` and `futures`.
- **Issue:** These are substantial dependency trees. Because `testing_aids` is
  only ever a dev-dependency, they are pulled into the *test* build graph of
  eight crates. They do not reach any release artifact.
- **Impact:** Low, and compile-time only. Recorded because CI test build time is
  paid on every push, and because it is worth stating explicitly that the cost
  is bounded to the test graph so nobody mistakes it for a release concern.
- **Remediation:** No action recommended. The dependencies are what the crate
  needs to do its job (installing tracing subscribers and capturing OTel
  output), and the alternative — reimplementing subscriber capture by hand —
  would be a deviation from the ecosystem default with no benefit. If test build
  time becomes a measured problem, feature-gating the OTel half so that crates
  which only need `init_tracing!` do not pull `opentelemetry_sdk` would be the
  first lever.
- **Evidence:** inferred from code reading (manifest inspection).

#### F35. POSITIVE — zero release-path leakage, verified across all eight consumers

- **Location:** the eight consuming manifests, each listing `testing_aids` under
  `[dev-dependencies]`: `crates/seatbelt/Cargo.toml:95`,
  `crates/fetch/Cargo.toml:135`, `crates/ohno/Cargo.toml:39`,
  `crates/bytesbuf_io/Cargo.toml:42`, `crates/recoverable/Cargo.toml:29`,
  `crates/bytesbuf/Cargo.toml:58`, `crates/cachet/Cargo.toml:83`,
  `crates/fetch_hyper/Cargo.toml:109`. Also declared at root `Cargo.toml:211`.
- **Finding:** Not one consumer lists it under `[dependencies]`. The workspace
  uses `resolver = "2"` (root `Cargo.toml:5`), which does **not** unify features
  from dev-dependencies into normal builds — so even the feature-unification
  route by which a dev-dependency can accidentally influence a release build is
  closed. The crate is additionally `publish = false`, so it cannot leak to
  external consumers even by mistake, and the whole crate is
  `#![cfg_attr(coverage_nightly, coverage(off))]`
  (`crates/testing_aids/src/lib.rs:8-9`), correctly excluding test scaffolding
  from coverage metrics.
- **Impact:** N/A — this is the desired state. Recorded so that a future change
  that moves `testing_aids` into a `[dependencies]` section is recognised as the
  regression it would be.
- **Evidence:** empirically verified by exhaustive grep of every `Cargo.toml` in
  the workspace for `testing_aids`, and manual inspection of each of the eight
  hits to confirm the section heading it appears under.

Beyond F34, **no performance issues were found**.

### Benchmark coverage

`crates/testing_aids/benches/` does not exist and **should not**. Benchmarking
test infrastructure measures the test harness rather than the product, produces
numbers nobody can act on, and would add a maintenance burden to code whose only
consumers are `#[test]` functions. **No benchmark recommended.**

The one thing worth protecting is F35, and the right instrument for that is not
a benchmark but a CI check (or a comment in the manifest) asserting that
`testing_aids` never appears outside `[dev-dependencies]`.

### Considered and ruled out

* **`init_tracing!` installing a global subscriber cheaply enough.** Per
  `docs/tracing-tests.md` this macro is required at module scope in every test
  binary that touches `tracing`. Its cost is per test binary, once, at startup —
  a teardown/startup cost the philosophy explicitly deprioritises, and it is in
  test code regardless.
* **`crates/testing_aids/src/tracing_logs/output.rs` (766 lines, the largest
  file in the group).** Read for hot-path patterns. It does allocate freely and
  formats eagerly, but every path in it runs inside a test assertion. Optimising
  it would trade test-code clarity for speed nobody experiences. **Deliberately
  ruled out.**
* **Whether `test-util`-style features on other crates pull `testing_aids` into
  a normal dependency edge.** Checked `ohno`'s `test-util` feature — it gates
  in-crate helpers (`crates/ohno/src/test_util.rs`) and does not add a
  `testing_aids` dependency edge. Clean.

---

## Crate: benchmarking

### Summary

This crate matters more than its size suggests, and it is where this group's
**single most important finding** lives — not because the cost is large, but
because it is a *measurement* defect. `benchmarking` produces the numbers that
every other optimisation decision in this workspace is made from. A constant
additive offset in those numbers biases the entire evidence base, including the
evidence for and against every other finding in this document.

Two genuinely excellent properties should be preserved: the crate has **no
`[dependencies]` at all** (only `alloc_tracker` and `criterion` as
dev-dependencies), so its production footprint is exactly zero; and the
measurement-guard drop ordering is exactly right and already regression-tested.

### Findings

#### F36. `Vec::push` is inside the timed region of `time_sample_with_inputs`

- **Location:** `crates/benchmarking/src/lib.rs:144-146`; the timer starts at
  `:142` and stops at `:148`. The `Vec` is created with capacity at `:140`.
- **Issue:** The loop body is `outputs.push(black_box(bench(input)))`. The
  `push` — not just the benchmarked call — sits between `Instant::now()` and
  `start.elapsed()`. Because capacity was reserved before the timer started,
  there is no *allocation* inside the region; but a `push` still performs a
  capacity check (a compare and a predictable branch), a store of the output
  value (which for a large `T` is a memcpy proportional to `size_of::<T>()`),
  and a length increment. That is a constant additive offset per iteration on
  every benchmark written with this helper.
- **Impact:** **High — the highest-impact finding in this group.** The absolute
  cost is small, but it is not the absolute cost that matters. Three
  consequences: (1) every allocation-tracked benchmark in the workspace reports
  a time larger than the operation actually takes, by an amount that varies with
  the size of the operation's return type; (2) that makes benchmarks of
  different operations non-comparable, since an operation returning a large
  struct absorbs more offset than one returning a `u64`; (3) for a fast
  elementary operation — precisely what `docs/benchmarks.md` instructs authors
  to benchmark — the offset can be a substantial *fraction* of the measurement.
  A measurement defect that inflates cheap operations more than expensive ones,
  in a repository whose stated benchmark policy is to measure elementary
  operations, systematically biases the team away from the very optimisations
  the philosophy asks for.
- **Remediation:** Move the storage out of the timed region. The outputs are
  collected only to defer their destructors past `elapsed()` (which is correct —
  see the positive below), so what is needed is a way to retain the values
  without a `push` in the loop. Options, in order of preference:
  (a) **Write through a raw slice with a pre-set length** — allocate the `Vec`
  at full length before the timer using a `Default`/`MaybeUninit` scheme and
  index-assign inside the loop, so the loop body is a single store with no
  capacity check and no length update. Indexed assignment still bounds-checks,
  but the bound is loop-invariant and LLVM hoists it.
  (b) **Restructure so the timed region contains only `bench(input)`** and the
  retention happens by pushing a `ManuallyDrop`/`MaybeUninit` handle outside —
  structurally cleaner but a larger change to the helper's shape.
  (c) The `unsafe` route (`ptr::write` into reserved capacity, set len after the
  timer) is the fastest and is what a benchmark harness would normally do, but
  see the philosophy note.
  Whichever is chosen, the change must preserve the existing drop ordering.
- **Evidence:** inferred from code reading — verified by reading the exact line
  range and confirming the `Instant::now()` / `elapsed()` bracket. Cannot be
  empirically quantified here: measuring the offset requires building the crate
  with `criterion`, which the environment cannot do. Quantifying it should be
  the first thing anyone with a working build does, because it calibrates every
  other number in the repository.
- **Philosophy note:** CONFLICTING, for remediation option (c) only.
  `benchmarking` is currently entirely `unsafe`-free, and introducing `unsafe`
  into it would be a notable change to the crate's character — and into a crate
  whose job is to be trustworthy, no less. Options (a) and (b) avoid `unsafe`
  and are therefore the recommended path even though (c) is faster. The finding
  itself does not conflict with the philosophy; the tempting fix does.

#### F37. `time_sample` and `time_sample_with_inputs` measure different things — one includes destructor cost, the other excludes it

- **Location:** `crates/benchmarking/src/lib.rs:50-60` (`time_sample`, with the
  output dropped at `:57`) versus `:140-152` (`time_sample_with_inputs`, which
  defers all drops until after `start.elapsed()` at `:148`).
- **Issue:** In `time_sample`, the benchmarked call's result is bound to `_` and
  therefore dropped at the end of the statement — **inside** the timed loop. In
  `time_sample_with_inputs`, results are retained and dropped after the timer
  stops. The two helpers therefore have opposite policies on whether the
  operation's destructor cost counts as part of the operation.
- **Impact:** **High.** This is worse than either policy would be on its own,
  because both helpers are in the same crate with near-identical names and no
  documentation of the difference. A benchmark author choosing between them
  based on whether they happen to need inputs will silently change what is being
  measured. For an operation that returns something expensive to drop — a
  `Vec`, an `Arc` chain, anything owning heap memory, which describes most of
  what this workspace benchmarks — the two helpers will produce materially
  different numbers for the same operation, and nothing warns the author.
- **Remediation:** Pick one policy, apply it to both, and document it
  prominently. The correct policy is `time_sample_with_inputs`' — defer drops
  past the timer — because the philosophy's "benchmark elementary operations"
  instruction means measuring the operation, not the operation plus the
  teardown of its result, and because teardown cost is separately
  deprioritised by `docs/performance.md`. Change `time_sample` to retain its
  outputs the same way, and state the policy in the crate docs so authors know
  that destructor cost is excluded and must be benchmarked separately if it
  matters.
- **Evidence:** inferred from code reading — verified by reading both timed
  regions and confirming the binding and drop points.

#### F38. `time_sample_async` constructs the future inside the timed region

- **Location:** `crates/benchmarking/src/lib.rs:69-83`.
- **Issue:** `bench(iteration)` is called inside the timer, so the measurement
  includes constructing the future as well as polling it to completion.
- **Impact:** Low. For an async benchmark this is arguably correct — future
  construction *is* part of invoking an async operation, and separating them
  would measure something no caller experiences. Recorded so the choice is
  explicit rather than accidental, and because it interacts with `ohno`'s F12
  (a nested async closure makes future construction more expensive, and this
  helper would attribute that to the operation).
- **Remediation:** No code change. Document that `time_sample_async` includes
  future construction, so authors comparing a sync and an async variant of the
  same operation know the async number carries an extra component.
- **Evidence:** inferred from code reading.

#### F39. POSITIVE — measurement-guard drop ordering is exactly right, and regression-tested

- **Location:** `crates/benchmarking/src/lib.rs:149-151`, with the regression
  test at `:210-247`.
- **Finding:** After `start.elapsed()`, the code drops the measurement guard
  first, then the outputs, then the inputs. This is the correct order: dropping
  the measurement first means the outputs' and inputs' destructors — including
  their deallocations — are not attributed to the benchmarked operation by the
  allocation tracker. Getting this wrong is the classic allocation-tracking bug,
  and not only is it right here, there is a test asserting it stays right.
- **Impact:** N/A — desired state. Recorded because F36's remediation must not
  disturb it, and whoever implements F36 needs to know this ordering is
  load-bearing and tested.
- **Evidence:** inferred from code reading (drop order and the test both read
  directly).

#### F40. POSITIVE — zero production footprint

- **Location:** `crates/benchmarking/Cargo.toml`.
- **Finding:** The crate has **no `[dependencies]` section at all**.
  `alloc_tracker` and `criterion` appear only under `[dev-dependencies]`. A
  benchmark harness that cannot possibly enter a release build is exactly what
  is wanted.
- **Evidence:** empirically verified by manifest inspection.

### Benchmark coverage

`crates/benchmarking/benches/` does not exist and **should not**. Benchmarking
the benchmark harness with the benchmark harness is circular: the offset F36
identifies would be present in both the measurement and the thing measured.

The right instruments for this crate's correctness are the ones it already
partly has — the drop-ordering regression test at
`crates/benchmarking/src/lib.rs:210-247` — extended with a test that asserts the
timed region contains no allocation, and with the F36/F37 fixes. Once F36 is
fixed, a one-off calibration measurement (time a no-op closure through
`time_sample_with_inputs` before and after) would quantify how much the
workspace's historical benchmark numbers were inflated. That is a one-time
exercise, not a `benches/` directory.

### Considered and ruled out

* **The `Vec::with_capacity` at `crates/benchmarking/src/lib.rs:140`
  allocating inside the measurement.** Ruled out — it is on line 140, and
  `measure(iterations)` is called on line 141. The allocation happens before the
  measurement guard exists, so it is correctly excluded. Good as written.
* **Missing `black_box` on the benchmarked output.** Ruled out — `black_box` is
  applied at `:145`. Present and correct.
* **Missing `black_box` on the inputs.** Considered. The inputs come from a
  runtime-constructed `Vec` that the compiler cannot constant-fold through, so
  the omission is harmless in practice. Not worth a change, but worth stating so
  a future author does not assume inputs are protected.
* **`Box::pin` on the measured path in the async helper.** Checked against
  `docs/benchmarks.md`'s explicit rule; the helper does not `Box::pin`. Clean.
* **Multi-threaded benchmark support.** `docs/benchmarks.md` says benchmarks
  should be single-threaded synchronous unless prompted otherwise; the helpers
  match that default. Correct as written.

---

## Crate: automation

### Summary

Build tooling: `publish = false`, invoked by developers and CI at human
timescales, with no runtime consumers anywhere in the workspace (verified by
grepping every manifest — nothing depends on it). Performance findings here are
correspondingly low-stakes, and the honest recommendation for both is "no action
unless it actually hurts".

### Findings

#### F41. `kill_by_pid` spawns an external process instead of signalling directly

- **Location:** `crates/automation/src/process.rs:100-109`.
- **Issue:** The function shells out to `kill` on Unix and `taskkill` on
  Windows, which means a `fork`/`exec` (or `CreateProcess`), a PATH lookup, and
  a wait — to deliver a signal that `Child::kill()` or a direct `libc::kill`
  would deliver with one syscall.
- **Impact:** Low. This is on the timeout path of a build tool: it runs when
  something has already gone wrong, at most a handful of times per invocation,
  in a program whose baseline unit of work is compiling Rust. The cost is
  invisible.
- **Remediation:** No action recommended on performance grounds alone. If this
  code is touched for another reason, `Child::kill()` is simpler as well as
  faster and does not depend on `kill`/`taskkill` being on `PATH` — a
  robustness argument that is stronger than the performance one. Note the
  external-process approach may have been chosen deliberately to kill a whole
  process *group*, which `Child::kill` does not do; if so that is a correctness
  requirement and the current code is right. **Verify intent before changing.**
- **Evidence:** inferred from code reading.

#### F42. Depending on `ohno` with `features = ["app-err"]` turns that feature on workspace-wide

- **Location:** `crates/automation/Cargo.toml`, the `ohno` dependency entry.
- **Issue:** Cargo unifies features across a workspace build, so
  `cargo build --workspace` enables `ohno/app-err` for every crate, not just
  `automation`. Any code compiled behind that feature is built even for crates
  that never use it.
- **Impact:** Low, and bounded to the workspace's own builds — an external
  consumer of `ohno` is entirely unaffected, since `automation` is
  `publish = false` and is not in anyone's dependency graph. The cost is a small
  amount of extra compilation in local and CI workspace builds.
- **Remediation:** No action recommended. The alternative (removing the
  dependency, or duplicating `app-err`'s functionality) costs more than the
  feature does. Recorded so that if `app-err` ever grows expensive or
  behaviour-affecting, the workspace-wide reach of this single line is already
  documented.
- **Evidence:** inferred from code reading (manifest inspection plus the
  `resolver = "2"` declaration at root `Cargo.toml:5`; note resolver 2 does not
  unify across *dev*-dependencies, but `ohno` here is a normal dependency, so
  unification does apply).

Beyond F41 and F42, **no performance issues were found**.

### Benchmark coverage

`crates/automation/benches/` does not exist and **should not**. This is build
tooling with no runtime consumers; every operation in it is dominated by process
spawning and filesystem I/O at human timescales. A Criterion benchmark would
measure the operating system, not this crate. **No benchmark recommended.**

### Considered and ruled out

* **`crates/automation/src/cargo_metadata.rs` parsing cost.** Read it; it
  deserialises `cargo metadata` output. The parse is dwarfed by the `cargo
  metadata` invocation that produced the input, which itself resolves the
  dependency graph. Optimising the parse would be optimising the wrong end.
* **Allocation patterns throughout the crate.** The crate allocates freely —
  `String`s for command construction, `Vec`s for arguments. Correct for its
  domain; all of it is dwarfed by process spawning.
* **`#[inline]` on `automation`'s public functions.** Ruled out — the crate has
  no external consumers, so cross-crate inlining has nothing to inline into.
  Rule 1 does not apply.

---

## Appendix: findings index

| ID | Crate | Title | Impact | Evidence |
|---|---|---|---|---|
| F1 | ohno | `Box`→`Arc` re-allocation per error construction | Medium | empirical |
| F2 | ohno | `Clone` deep-copies the enrichment `Vec` | Medium | empirical |
| F3 | ohno | Derived `Display` allocates a `String` per format | Medium | empirical |
| F4 | ohno | `ErrorExt::message()` allocates twice | Medium | inferred |
| F5 | ohno | No build-time backtrace opt-out; 12–26x bytes when enabled | Medium | empirical |
| F6 | ohno | `ErrorLabel::from_error_chain` allocates for single-label chains | Low | inferred |
| F7 | ohno | `app_err!` with a literal allocates via `format!` | Low | inferred |
| F8 | ohno | Eager `to_string()` in non-lazy `IntoAppErr` methods | Low | inferred |
| F9 | ohno | Zero `#[inline]` across 25 public functions | Low | inferred |
| F10 | ohno_macros | `syn` `extra-traits` in `[dependencies]` | Medium (compile) | inferred |
| F11 | ohno_macros | Generated `Display` materialises a `String` (see F3) | Medium | empirical |
| F12 | ohno_macros | `#[enrich_err]` nests an async closure in `async fn` | Low–Medium | inferred, unverified |
| F13 | ohno_macros | Eight impl blocks emitted per derived error type | Low (compile) | inferred |
| F14 | data_privacy | Two strings hashed per redaction before any work | Medium | inferred + empirical layout |
| F15 | data_privacy | `redacts()` + `redact()` hash twice | Medium | inferred |
| F16 | data_privacy | Payload formatted then discarded under default policy | Medium | inferred + empirical |
| F17 | data_privacy | 128 stack bytes zeroed per redacted format | Low–Medium | inferred |
| F18 | data_privacy | `Sensitive<T>` embeds a 48-byte `DataClass` | Medium | empirical |
| F19 | data_privacy | `write!` where `write_str` would do (5 sites) | Low | inferred |
| F20 | data_privacy | `<` vs `<=` sends 120-byte values down the slow path | Low | inferred |
| F21 | data_privacy | One `#[inline]`, not on the hot path | Low | inferred |
| F22 | data_privacy_core | `DataClass` is a 48-byte two-`Cow` hash key | Medium | empirical layout |
| F23 | data_privacy_core | `DataClass::clone` allocates twice when deserialised | Low | empirical |
| F24 | data_privacy_core | No `#[inline]` on hot-path accessors | Low | inferred |
| F25 | data_privacy_macros_impl | Generated code duplicates the buffer pattern | Medium | inferred |
| F26 | data_privacy_macros_impl | `data_class()` returns 48 bytes by value | Low–Medium | inferred + empirical |
| F27 | data_privacy_macros_impl | Per-type codegen instead of shared helpers | Low (compile) | inferred |
| F28 | fundle | "Zero-cost" claim not honoured by two of three macros | Low | inferred |
| F29 | fundle_macros_impl | `#[deps]` deep-clones every field | Low–Medium | inferred |
| F30 | fundle_macros_impl | `#[newtype]` clones on construction | Low | inferred |
| F31 | fundle_macros_impl | `syn` `extra-traits` in `[dependencies]` | Medium (compile) | inferred |
| F32 | fundle_macros_impl | O(N) impls with N type params per bundle | Low (compile) | inferred |
| F33 | recoverable | Eight trivial `const fn` accessors lack `#[inline]` | Low | inferred |
| F34 | testing_aids | Heavy deps inflate test build time only | Low (compile) | inferred |
| F35 | testing_aids | POSITIVE — zero release-path leakage, 8/8 verified | — | empirical |
| F36 | benchmarking | `Vec::push` inside the timed region | **High** | inferred |
| F37 | benchmarking | Two helpers disagree on whether drops are timed | **High** | inferred |
| F38 | benchmarking | Async helper times future construction | Low | inferred |
| F39 | benchmarking | POSITIVE — drop ordering correct and tested | — | inferred |
| F40 | benchmarking | POSITIVE — zero production footprint | — | empirical |
| F41 | automation | `kill_by_pid` spawns a process | Low | inferred |
| F42 | automation | `ohno/app-err` enabled workspace-wide | Low | inferred |

**42 findings across all thirteen crates** (39 issues, 3 recorded positives),
plus the group-wide benchmark-coverage gap and the additional positives recorded
in each crate's "Considered and ruled out" section.

### Recommended order of work

1. **F36 and F37** (`benchmarking`) — first, unconditionally. Until the harness
   measures the right thing, no number produced in this repository can be
   trusted, including any number produced to validate the findings below.
2. **F14 / F22** (`data_privacy` / `data_privacy_core`) — the top product-code
   cost, on a request-rate path, with a surgical fix (cached hash) available.
3. **F16** (`data_privacy`) — skip formatting under an erasing policy. Pure
   fast-path addition, no behaviour change.
4. **F3 / F11** (`ohno_macros`) — write to the formatter instead of allocating.
   Fixing it also halves F4.
5. **F1** (`ohno`) — construct the `Arc` directly. One allocation and one
   deallocation removed from every error.
6. **F18** (`data_privacy`) — `&'static DataClass` in `Sensitive`. 56 bytes to
   16.
7. **F10 / F31** (`ohno_macros` / `fundle_macros_impl`) — drop
   `syn/extra-traits` from `[dependencies]`. Compile-time only, zero risk,
   already proven achievable in-repo.
8. **F19, F20, F6, F7, F28** — small, safe, mechanical.
9. **F9, F21, F24, F33** (`#[inline]`) — only after the release/bench profile
   discrepancy noted in the cross-cutting section is resolved, since until then
   no benchmark in this repository can demonstrate their effect.
10. **F2** (`ohno`) — measure first, then very probably decline. Architectural.
