# g4-routing-http findings

Scope: `crates/routerama`, `crates/routerama_build`, `crates/routerama_macros`,
`crates/http_extensions`, `crates/http_path_template`.

Round 2 of an exhaustive workspace performance review. This file re-verifies,
corrects and extends the (lost) round-1 result set for the same five crates.

## Environment caveat — read before acting on any finding

The analysis container has **no egress to `index.crates.io` / `static.crates.io`**
(403 `CONNECT tunnel failed`), no cargo registry cache and no prebuilt `target/`.
`cargo build`, `cargo test`, `cargo clippy`, `cargo bench`, `cargo build --offline`
and every `just` recipe fail at dependency resolution. This was confirmed once and
not retried.

Consequently:

- Every finding is labelled `inferred from code reading` unless explicitly marked
  `empirically verified`.
- The two things that **could** be measured here were measured:
  1. **Type layouts**, via a throwaway dependency-free replica program compiled
     with plain `rustc` and printing `size_of`/`align_of` (deleted afterwards,
     never added to the repo).
  2. **The `#[inline]` census**, via `grep` over the five crates.
- Every finding names the specific benchmark that would confirm or refute it, so
  the work can be finished in an environment that can build.

## Cross-group context that colours several findings

- `[profile.bench]` in the workspace manifest sets `lto = "fat"` and
  `codegen-units = 1`; `[profile.release]` sets neither. **Every benchmark number
  in this repository therefore comes from a build configuration that no consumer
  of these crates gets.** This matters directly here: fat LTO across the whole
  benchmark binary can inline across crate boundaries without any `#[inline]`
  annotation, which is *exactly* the defect flagged in `http_extensions` (H1) and
  `http_path_template` (P1). The missing annotations are invisible under
  `[profile.bench]` and expensive under a consumer's `[profile.release]`.
- Benchmarks are built with `--all-features`, so feature-gated cost (for example
  `http_extensions`'s `json`) is always present in benchmark numbers and never
  isolated.
- Sibling group g9 reports that `just bench` / `just bench-cg` do not exist despite
  `docs/performance.md` referring to them, and that no benchmark runs in CI. All
  benchmark-coverage findings below should be read against that backdrop: coverage
  gaps are never caught automatically.

## Corrections to the round-1 result set

Round 1's summary contained five claims that do not survive checking. They are
corrected here so they are not propagated:

1. **The SIMD query parser lives in `routerama`, not `http_extensions`.** The path
   is `crates/routerama/src/query/parser.rs`. `crates/http_extensions/src/query/`
   does not exist.
2. **`routerama_mixed/dispatch` *does* isolate the dynamic-hit-after-static-miss
   case.** `crates/routerama/benches/routerama_mixed.rs:19-21` defines a
   `dynamic_fallback_hit` benchmark backed by
   `crates/routerama/benches/common/mixed_scenarios.rs`. What is genuinely
   unbenchmarked is the *verb-splitting* variant of that path (see B2) and the
   >16-segment spill path (see R1).
3. **`http_extensions` is not benchmark-free on the routing path.**
   `crates/http_extensions/benches/router_resolve.rs` and `router_resolve_cg.rs`
   exist and are correctly paired. The real gaps are body collection, JSON and the
   extension traits.
4. **The "triple scan" is conditional, not unconditional.** The generated code at
   `crates/routerama_build/src/macro_impl/resolver.rs:370-371` short-circuits on
   `splits_verbs(__state) && …`, so the third pass only happens when a dynamic
   route actually declares a `:verb`. The finding stands; its blast radius is
   smaller than stated.
5. **Round 1 missed `http_path_template` entirely on `#[inline]`.** It has 19
   public functions and zero `#[inline]` annotations (see P1).

## Empirical data

### `#[inline]` census (empirical, `grep` over `crates/<crate>/src`)

| crate | `pub fn` count | `#[inline*]` count |
|---|---|---|
| `routerama` | 60 | 48 |
| `routerama_build` | 22 | 9 |
| `routerama_macros` | 3 | 0 (correct — proc-macro entry points) |
| `http_extensions` | **114** | **0** |
| `http_path_template` | **19** | **0** |

### Type layouts (empirical, `rustc` replica, x86_64-unknown-linux-gnu)

```
VarPlan                                    size=72   align=8
Leaf                                       size=104  align=8
RtNode                                     size=72   align=8
(Box<str>, RtNode)          [LiteralEdge]  size=88   align=8
(Box<str>, Box<str>, RtNode)  [AffixEdge]  size=104  align=8
WalkAction                                 size=24   align=8
ScannedPath                                size=72   align=8
ParseError                                 size=16   align=8
Result<PathTemplate, ParseError>           size=32   align=8
Segment                                    size=56   align=8
PathTemplate                               size=32   align=8
HttpMethod                                 size=24   align=8
inline scratch in resolve_scanned_checked  = 2 * 16 * usize = 256 bytes
```

---

## Crate: routerama

### Summary

`routerama` is the strongest crate in this scope by a wide margin: 48 of 60 public
functions carry `#[inline]`, every Criterion benchmark file has a matching
Callgrind `_cg` file, and the design deliberately avoids allocation on the resolve
path (`SmallVec` inline captures, stack scratch for segment offsets,
`HttpMethodRepr::Standard(&'static str)` for the nine standard verbs). The
remaining opportunities are second-order: a redundant re-scan on an uncommon
spill path, fixed-size scratch that is zeroed regardless of how much of it is
used, array-of-structures trie edges with a large stride relative to the key
being compared, and a SIMD query path that makes more passes than the scalar path
it replaces.

Eleven findings, none High in isolation. R1, R3, R5 and R10 are the ones worth
measuring first.

### Findings

#### R1. `resolve_scanned_checked` re-scans the whole path and heap-allocates on the >16-segment spill path

- **Location:** `crates/routerama/src/raw_resolver.rs:158-202`
- **Issue:** The function declares two fixed `[0usize; 16]` scratch arrays
  (`:158-159`), scans the path into them at `:178`, then at `:182` checks whether
  the scan reported `path.count() > 16`. If it did, it calls
  `resolve_with_heap_offsets`, which allocates `vec![0usize; capacity * 2]`
  (`:198`) and **re-runs the entire segment scan from byte 0** (`:200`). A route
  table whose deepest route exceeds 16 segments therefore pays two full SIMD
  scans plus a heap allocation on *every single resolve*, including for shallow
  request paths that would have fitted in the inline scratch — the branch is on
  the scanner's reported count for this particular request, so shallow requests
  do take the fast path, but any request deeper than 16 segments pays double.
  The mirror helper in `codegen_helpers/scanned_path.rs:149-173` has the same
  double-scan shape but is smarter: it sizes its inline capacity as
  `max_segments.min(16)`, so it at least does not over-reserve.
- **Impact:** Medium — bounded to deep paths, which are uncommon in REST APIs but
  are exactly the shape an attacker controls. The allocation is on the hot path,
  which `docs/performance.md` calls out explicitly.
- **Remediation:** Surgical. Have the scanner report the required capacity without
  discarding the offsets it already computed — either scan into a growable
  buffer, or make the spill path resume from the point the inline scratch filled
  rather than restarting at byte 0. The count is already known before the second
  scan begins, so the second scan is pure re-derivation. Alternatively, adopt the
  `scanned_path.rs` approach of sizing inline capacity from `max_segments` and
  make the spill genuinely rare.
- **Evidence:** inferred from code reading. Confirming benchmark: a new
  `routerama_dynamic/dispatch/deep_path` case (≥ 20 segments) in
  `crates/routerama/benches/routerama_dynamic.rs` with a `_cg` twin — Callgrind
  will show the doubled instruction count and the `malloc` unambiguously, which
  wall-clock at this magnitude will not.

#### R2. 256 bytes of stack scratch are zero-initialised on every resolve regardless of table depth

- **Location:** `crates/routerama/src/raw_resolver.rs:158-161`
- **Issue:** `let mut starts = [0_usize; 16]; let mut ends = [0_usize; 16];` are
  unconditionally zero-initialised, then immediately sliced to
  `[..self.max_segments]` at `:161`. For a route table whose deepest route is 3
  segments — the overwhelmingly common case — 26 of the 32 `usize` slots are
  zeroed and never read. Empirically these two arrays are 256 bytes of stack.
  LLVM *may* elide the initialisation since every live element is overwritten by
  the scan before being read, but that is not guaranteed across the opaque SIMD
  scan call, and the 256-byte stack frame itself is real regardless.
- **Impact:** Low — 256 bytes of stack touch per resolve is at most four cache
  lines, likely already hot. Worth confirming rather than assuming.
- **Remediation:** Use `[MaybeUninit<usize>; 16]` (the scan writes every element
  it reports), or simply size the arrays from a `const` generic threaded from
  `max_segments`. Note that `MaybeUninit` trades a defensive property for speed;
  `docs/performance.md` asks that defensive runtime checks be preserved, so this
  should only be done if Callgrind shows the zeroing is not elided.
- **Evidence:** array sizes empirically verified (rustc layout replica); the
  non-elision is inferred from code reading. Confirming benchmark:
  `routerama_static_cg` — compare instruction counts for a 2-segment table
  against a 16-segment table resolving the same 2-segment path. If the zeroing is
  elided the counts match.

#### R3. Literal-edge lookup is a linear scan over an 88-byte stride to compare a 16-byte key

- **Location:** `crates/routerama/src/walk.rs:49-52`; layout at
  `crates/routerama/src/rt_node.rs:17-24`
- **Issue:** `descend_iterative` finds the matching literal child with
  `node.literals.iter().find(…)`. `node.literals` is
  `Box<[(Box<str>, RtNode)]>` — an array of structures whose element is
  **88 bytes** (empirically verified), of which only the leading 16 bytes (the
  `Box<str>` fat pointer) participate in the comparison. Scanning eight siblings
  therefore touches ~704 bytes / 11 cache lines to read 128 bytes of key. The
  prefilter at `:51` (`key.len() == bytes.len() && key.as_bytes().first() ==
  bytes.first()`) is good and avoids most `memcmp` calls, but it still has to
  *load* each key pointer, and the pointer chase to the string data itself is a
  second dependent load.
- **Impact:** Medium — this is the innermost loop of every dynamic resolve, and it
  is O(siblings). `rt_node.rs:161-179` orders literals by descendant weight, which
  helps average case but does not change the asymptotics or the stride.
- **Remediation:** Split the array of structures into two parallel arrays:
  `Box<[Box<str>]>` for keys (16-byte stride, 4 keys per cache line) and
  `Box<[RtNode]>` for children, indexed by the position the key scan returns.
  This is a contained change to `RtNode` plus the two call sites in `walk.rs`. A
  more aggressive variant — storing a `(len, first_byte)` prefilter array of
  `u16` for a 2-byte stride — is possible but is an architectural change and
  should not be attempted before the SoA split is measured. For large sibling
  counts a perfect hash would beat both, but `docs/performance.md` asks for
  surgical over architectural and real route tables rarely have wide fan-out at a
  single node.
- **Evidence:** stride empirically verified (rustc layout replica); the cache
  behaviour is inferred from code reading. Confirming benchmark:
  `routerama_dynamic_cg` with a scenario whose root node has 32+ literal
  siblings — Callgrind's `Dr`/`D1mr` counters will show the difference directly.
  `crates/routerama/benches/criterion_routers.rs` (the `matchit` / `path-tree`
  comparison) is the right place to check the change does not regress against
  competitors.

#### R4. The affix-edge predicate is evaluated twice for the selected edge

- **Location:** `crates/routerama/src/walk.rs:53-55` and `:77-84`
- **Issue:** `descend_iterative` first locates the first viable affix edge with a
  `.find(…)` over `node.affixes` (`:53-55`), remembering its index as `skip`.
  When the walk later backtracks into the affix branch (`:77-84`), it iterates
  edges from `skip` onward and **re-tests the same prefix/suffix predicate on the
  edge at `skip`** — the one the earlier `.find` already proved matches. The
  affix element is 104 bytes (empirically verified), so this is a redundant
  re-load of a wide element plus two `starts_with`/`ends_with` calls.
- **Impact:** Low — only on paths with affix routes, and only one duplicated
  predicate evaluation per node.
- **Remediation:** Have the `.find` at `:53` return the matching edge (or its
  index *and* the fact that it matched) and have the loop at `:77` start from
  `skip + 1`, handling the known-good edge first without re-testing.
- **Evidence:** inferred from code reading. Confirming benchmark: add an
  `affix_backtrack` case to `crates/routerama/benches/routerama_dynamic.rs` and
  its `_cg` twin — Callgrind instruction delta only; wall-clock will not resolve
  two `starts_with` calls.

#### R5. Method dispatch is a linear scan of 104-byte leaves comparing heap `String`s

- **Location:** `crates/routerama/src/walk.rs:134-138`; `Leaf` layout in
  `crates/routerama/src/rt_node.rs`
- **Issue:** `dispatch` selects the matching leaf with
  `leaves.iter().find(|leaf| leaf.method == self.method && …)`. `Leaf` is
  **104 bytes** (empirically verified) and stores `method`, `verb` and `name` as
  `String` / `Option<String>`, so every comparison is a pointer chase to
  heap-allocated string data followed by a `memcmp`. This happens after the trie
  descent has already succeeded, i.e. on every successful resolve.
  The crate *already has* an `HttpMethod` enum
  (`crates/routerama/src/http_method.rs:81-94`) whose `Standard(&'static str)`
  representation covers the nine standard verbs without allocation — but `Leaf`
  does not use it, so the fast representation is thrown away at trie-build time
  and re-derived by string comparison at resolve time.
- **Impact:** Medium — one pointer chase and `memcmp` per leaf per successful
  resolve, on a 104-byte stride. Most nodes have 1–4 leaves so the constant is
  small, but the fix is cheap and this is the last step of the hottest path.
- **Remediation:** Store `HttpMethod` (or a `u8` discriminant for the nine
  standard verbs, falling back to a string only for extension methods) in `Leaf`
  and compare discriminants first. Separately, `Leaf`'s three `String`s and one
  `Vec` can become `Box<str>` / `Box<[VarPlan]>`, dropping the type from 104 to
  72 bytes (the capacity field of each is dead after trie construction) — 1.4
  leaves per cache line becomes 0.9, and more importantly leaves pack denser.
- **Evidence:** `Leaf` size empirically verified (rustc layout replica); the
  comparison cost is inferred from code reading. Confirming benchmark:
  `routerama_static_cg` and `routerama_dynamic_cg` — Callgrind instruction counts
  on any successful-resolve case will move.

#### R6. `Leaf` retains `Vec`/`String` capacity fields that are dead after trie construction

- **Location:** `crates/routerama/src/rt_node.rs` (`Leaf` definition);
  `crates/routerama/src/raw_match.rs:52-56` (consumer)
- **Issue:** `Leaf` is built once at trie-compile time and never mutated
  afterwards, yet stores growable containers. Empirically it is 104 bytes; the
  equivalent with `Box<str>` / `Option<Box<str>>` / `Box<str>` /
  `Box<[VarPlan]>` / `usize` is 72 bytes. The same argument applies to `VarPlan`
  (72 bytes). Since leaves are scanned linearly (R5), the 30% size reduction is a
  direct reduction in cache lines touched.
- **Impact:** Low — a locality improvement rather than an algorithmic one, and it
  compounds with R5 rather than standing alone.
- **Remediation:** Change the field types at trie-compile time. `RtNode` already
  uses `Box<[…]>` for its edge arrays, so this is consistency with the crate's
  own established pattern, not a new one.
- **Evidence:** empirically verified (rustc layout replica of both variants).
  Confirming benchmark: `routerama_dynamic_cg` `D1mr` counters on a wide table.

#### R7. `RawMatch::capture` is a linear scan with string comparison

- **Location:** `crates/routerama/src/raw_match.rs:52-56`
- **Issue:** `capture(name)` does
  `self.leaf.vars.iter().position(|plan| plan.key() == name)` — an O(vars) scan
  with a `str` comparison per variable, executed once per capture the handler
  reads. `VarPlan` is 72 bytes (empirically verified), so the scan stride is
  wide.
- **Impact:** Low — routes rarely have more than 2–3 captures, and the generated
  (macro) API resolves captures by index at compile time, so this is the
  `RawResolver` / dynamic-builder path only.
- **Remediation:** Leave as is for small `vars`. If it ever matters, sort `vars`
  by key at compile time and binary search, or have `RawMatch` expose
  `capture_at(index)` (it already stores captures positionally in a `SmallVec`)
  and document index-based access as the fast path.
- **Evidence:** inferred from code reading; `VarPlan` size empirically verified.
  Confirming benchmark: none exists for `RawResolver` capture extraction — a new
  `routerama_dynamic/captures` group would be needed.

#### R8. `split_verb` is a scalar reverse byte scan, executed as a separate pass from the SIMD segment scan

- **Location:** `crates/routerama/src/codegen_helpers/scan.rs:28-43` (scalar
  `split_verb`) vs `:73-98` and `:181+` (SSE2 / NEON `scan_segments`)
- **Issue:** `split_verb` walks the path backwards with `rposition` looking for
  `:`, one byte at a time, while the segment scanner immediately afterwards walks
  the same bytes forwards with 16-byte SIMD loads. The verb delimiter could be
  detected inside the same vectorised pass (it is a single-byte search over the
  same buffer, which is precisely what the existing SIMD helper does for `/`).
- **Impact:** Low — one extra linear pass over a short string, and the reverse
  scan usually terminates early at the last `/`. It compounds with B1/B2, where
  the same `split_verb` is executed two or three times per request.
- **Remediation:** Extend the SIMD scanner to report the last `:` offset alongside
  the segment offsets, and have `split_verb` become a lookup on the scan result.
  This is a contained change but it does widen the scanner's contract; measure
  first, since the reverse scan is usually only a handful of bytes.
- **Evidence:** inferred from code reading. Confirming benchmark:
  `routerama_static_cg` with a `:verb` route versus without — the delta isolates
  `split_verb`.

#### R9. Path-capture percent-decoding uses a scalar search and allocates per escaped capture, while the crate has SIMD helpers it does not reuse

- **Location:** `crates/routerama/src/decode.rs:15-38`
- **Issue:** The decode fast path uses `str::find('%')` (scalar) to decide whether
  decoding is needed. When it is, it builds a `Vec::with_capacity(…)` and
  finishes with `String::from_utf8` — one heap allocation per escaped capture, on
  the request path. Meanwhile `crates/routerama/src/query/scan.rs` contains
  vectorised single-byte and either-of-two-bytes searches that this module does
  not use.
- **Impact:** Low — the no-escape fast path (the common case) correctly returns a
  borrowed `&str` with no allocation, so the cost only lands on genuinely
  percent-encoded captures. Reported because "no allocation on the hot path" is
  an explicit house rule and this is a request-path allocation.
- **Remediation:** Reuse `query::scan`'s `find_byte` for the `%` search — a
  one-line substitution. The allocation itself is unavoidable for an owned
  decoded string; the API already returns `Cow`, so callers that can tolerate
  borrowing already do.
- **Evidence:** inferred from code reading. Confirming benchmark: no benchmark
  currently exercises an escaped capture. Add `routerama_dynamic/captures/encoded`
  plus a `_cg` twin.

#### R10. The SIMD query parser makes up to four passes per pair where the scalar parser fuses into one

- **Location:** `crates/routerama/src/query/parser.rs:56-92` (SIMD) vs `:99-141`
  and `:147-173` (scalar); threshold at `crates/routerama/src/query/scan.rs:10`
- **Issue:** `next_pair_simd` performs, per pair: a `find_byte` for the pair
  delimiter `&` (`:69`), a `find_byte` for `=` within the pair (`:82`), and then
  `decode::<true>` on the key (`:87`) and on the value (`:89`), each of which runs
  its own `contains_either` scan for `%`/`+`. That is up to four passes over the
  same bytes. `next_pair_scalar` (`:99-141`) delegates to `scan_pair`
  (`:147-173`), which finds `&`, `=`, and the need-decoding flags for key and
  value **in a single forward pass**. The SIMD path is chosen when the remaining
  query exceeds `SIMD_THRESHOLD = 32` bytes — i.e. for the *larger* inputs, where
  the extra passes cost the most.
- **Impact:** Medium — this is the query-parsing hot path and the two
  implementations have different asymptotic constants in the wrong direction. It
  is plausible the SIMD path still wins on throughput because each pass is 16×
  wider, but "4 wide passes vs 1 narrow pass" is only a 4× win at best, and it is
  4× the memory traffic.
- **Remediation:** Write a fused SIMD `scan_pair` that computes the `&` offset,
  the `=` offset and both needs-decoding flags from the same vector loads —
  mirroring the scalar `scan_pair`'s structure. This is a contained change inside
  `query/scan.rs` + `query/parser.rs`.
- **Evidence:** inferred from code reading. Confirming benchmark:
  `crates/routerama/benches/routerama_query.rs` / `routerama_query_cg.rs` already
  exist; they need a case with a query string comfortably above 32 bytes so the
  SIMD path is selected, benchmarked against the same input with the threshold
  raised so the scalar path runs. If the scalar path wins, `SIMD_THRESHOLD` is
  simply set too low — a one-constant fix.

#### R11. `ToQuery::to_query_string_with` starts from an uncapacitied `String`

- **Location:** `crates/routerama/src/query/to_query.rs:65-69`
- **Issue:** `let mut output = String::new();` followed by `write_query_with`.
  Producing a 60-byte query string from an empty `String` costs the usual
  doubling sequence (8 → 16 → 32 → 64), i.e. four allocations and three `memcpy`s
  where one allocation would do. Notably the `Encoder` itself
  (`crates/routerama/src/query/encoder.rs`) is exemplary — it writes through
  `fmt::Write` with `itoa::Buffer` for integers and never allocates a temporary,
  and `pair_display` explicitly documents "without allocating a temporary
  string". The only allocation in the whole encode path is this one, and it is
  larger than it needs to be.
- **Impact:** Low — client-side request construction rather than server-side
  request handling, so it is off the server hot path. Cheap to fix.
- **Remediation:** Add a `size_hint`-style method to `EncodeFields` that the
  derive can populate from the sum of parameter-name lengths (known at compile
  time) plus a per-value allowance, and use
  `String::with_capacity(hint)`. A cruder but still effective fix is a fixed
  starting capacity such as `QueryLimits`' typical output size.
- **Evidence:** inferred from code reading. Confirming benchmark: a
  `routerama_query/encode/to_query_string` case in the existing
  `routerama_query.rs` / `_cg` pair; Callgrind will show the `malloc` count drop
  from 4 to 1.

### Benchmark coverage

**Excellent — the model for the rest of the workspace.** Every Criterion file has a
matching Callgrind file, satisfying `docs/naming.md`'s rule that each `*_cg.rs`
has a Criterion counterpart (and going beyond it, since the reverse is not
required):

| Criterion | Callgrind | Covers |
|---|---|---|
| `routerama_static.rs` | `routerama_static_cg.rs` | generated static dispatch |
| `routerama_dynamic.rs` | `routerama_dynamic_cg.rs` | `RawResolver` trie walk |
| `routerama_mixed.rs` | `routerama_mixed_cg.rs` | static+dynamic, incl. `dynamic_fallback_hit` |
| `routerama_query.rs` | `routerama_query_cg.rs` | query parse/decode |
| `criterion_routers.rs` | `gungraun_routers.rs` | comparison vs `matchit`, `path-tree`, `route-recognizer`, `regex` |

Shared scenario definitions live in `benches/common/`, which keeps the Criterion
and Callgrind pairs genuinely measuring the same thing — a pattern the other
crates should copy.

Gaps:

- **The >16-segment spill path (R1) is not exercised.** No scenario builds a route
  table deeper than 16 segments, so the double scan and the `vec![…]` allocation
  never run under measurement.
- **The verb-splitting mixed case (B2) is not exercised.** The `MixedScenario` in
  `benches/common/mixed_scenarios.rs` declares no `:verb` route, so
  `splits_verbs()` is false and the third path scan never happens under
  measurement. This is the single most valuable benchmark to add for this group.
- **Percent-encoded path captures (R9) are not exercised** — no scenario uses an
  escaped capture, so the decode allocation is never measured.
- **Wide-fan-out literal nodes (R3) are not exercised** — scenarios have modest
  sibling counts, so the linear scan's O(n) term stays invisible.
- **Query *encoding* (R11) is not benchmarked at all** — `routerama_query.rs`
  covers parsing only.
- **Capture extraction via `RawMatch::capture` (R7) is not benchmarked.**

### Considered and ruled out

- **`HttpMethod` representation** (`crates/routerama/src/http_method.rs:81-94`).
  `HttpMethodRepr::Standard(&'static str)` avoids any allocation for the nine
  standard verbs; the type is 24 bytes. This is well done. (The complaint in R5 is
  that `Leaf` does not *use* it, not that it is badly designed.)
- **`RawMatch` inline capture storage** (`crates/routerama/src/raw_match.rs:17,22`).
  `SmallVec<[&str; 4]>` with `INLINE_CAPTURES = 4` covers essentially all real
  routes without allocating, and `capture_values` returns an empty `SmallVec`
  when there is nothing to capture. Raising the threshold would grow `RawMatch`
  for no benefit.
- **`RtNode` construction, `Drop` and `discard_source`**
  (`crates/routerama/src/rt_node.rs`). These are deliberately iterative to avoid
  stack overflow on deep tries — a correctness property that must be preserved.
  They are also build-time and teardown, which `docs/performance.md` explicitly
  deprioritises.
- **`DynBuilder::add`** (`crates/routerama/src/dyn_builder.rs:37-81`). Several
  `to_string()` calls, a `Vec<String>` and a `sort`. This is route-registration
  (configuration) time, executed once per route at startup —
  `docs/performance.md`'s "first-insert costs are usually not worth optimising"
  applies squarely.
- **`Encoder`** (`crates/routerama/src/query/encoder.rs`). Streams through
  `fmt::Write`, uses `itoa::Buffer` for all integer widths, and `pair_display`
  formats through an adapter rather than into a temporary `String`. No
  allocation anywhere. Exemplary.
- **`query::Error`** (`crates/routerama/src/query/error.rs:28-32`). `Copy`, three
  small fields, `&'static str` for the parameter name — no allocation on the
  error path and no `Result` inflation.
- **`#[inline]` coverage.** 48/60 is appropriate; the uncovered ones are generic
  or cold, which is exactly what `docs/performance.md` rule 2 asks for.
- **The literal prefilter at `walk.rs:51`** (`len` and first-byte check before
  `memcmp`). This is a good micro-optimisation already present; R3 is about the
  container layout around it, not this check.

---

## Crate: routerama_build

### Summary

For a build-time crate, "performance" has two faces: the compile-time cost it
imposes on every downstream crate, and the quality of the code it emits. Both
have findings.

The emitted code carries this group's only **High** finding: a mixed
static+dynamic route table scans the request path twice on every request that
falls through to a dynamic route, and three times when verb splitting is in play.
The emitted code is also unconditionally `#[inline]` and emits the entire route
trie into a single function body, which for a 5000-route table is a code-size and
compile-time hazard that no benchmark currently observes.

On the compile-time side, the crate pulls `syn` with `full` + `visit` + `derive`
as a **default** feature, so every downstream user of `routerama_macros` pays for
the heaviest `syn` configuration.

Eight findings.

### Findings

#### B1. Mixed static+dynamic resolvers scan the request path twice on every dynamic hit

- **Location:** `crates/routerama_build/src/macro_impl/resolver.rs:367-397`;
  consumer at `crates/routerama/src/raw_resolver.rs:145-187`
- **Issue:** The generated `Resolver::resolve` for a table containing both static
  and dynamic routes emits (at `:375`) a call to `__static_resolve`, which
  performs a full `split_verb` plus a full SIMD `scan_path`. If that returns
  `ResolveError::NotFound` (`:378`), it calls `__dynamic_resolve` (`:379`), which
  enters `RawResolver::resolve_scanned_checked` and does `split_verb` again at
  `raw_resolver.rs:156` and `scan_path` again at `:161`/`:178` — **starting from
  byte 0, re-deriving offsets that were computed microseconds earlier and thrown
  away.**
  The information is not merely re-derivable, it is *already in a suitable form*:
  `Walk` — the trie-descent engine — already consumes a `&ScannedPath`. Nothing
  about the design requires the second scan; it exists only because there is no
  entry point that accepts a pre-scanned path.
- **Impact:** **High** — this is the per-request cost of every dynamic route in
  any application that mixes static and dynamic routing, which is the normal
  configuration. It doubles the string-scanning work on those requests, and
  string scanning is the dominant cost of routing.
- **Remediation:** Surgical, and the pieces already exist. Add
  `RawResolver::resolve_prescanned(&self, scanned: &ScannedPath, verb:
  Option<&str>, method: …)` next to the existing entry points at
  `raw_resolver.rs:110-135`, delegating straight to the `Walk` that already takes
  a `&ScannedPath`. Then change the codegen at `resolver.rs:367-397` to hoist the
  `split_verb` and `scan_path` above the static/dynamic branch and pass the
  result to both. `resolve_scanned_checked` stays as the standalone entry point
  for dynamic-only tables. No behavioural change, no new abstraction.
- **Evidence:** inferred from code reading (both the generator and the runtime it
  targets were read line by line). Confirming benchmark:
  `crates/routerama/benches/routerama_mixed.rs:19-21` already has a
  `dynamic_fallback_hit` case and `routerama_mixed_cg.rs` measures it under
  Callgrind — **this finding is directly measurable today** by comparing the
  instruction count of `dynamic_fallback_hit` against
  `routerama_dynamic`'s equivalent single-scan case. Round 1's claim that this
  case was unbenchmarked was wrong.

#### B2. The same request path is scanned a third time when dynamic routes use `:verb`

- **Location:** `crates/routerama_build/src/macro_impl/resolver.rs:367-376`
- **Issue:** The branch guarded by `has_dynamic && has_static && !static_any_verb`
  emits `splits_verbs(__state) && split_verb(__path).1.is_some()` at `:370-371`
  as a pre-check *before* the static attempt. Because of the `&&` short-circuit
  this third `split_verb` only executes when `splits_verbs()` is true, i.e. when
  a dynamic route actually declares a `:verb` and the static routes do not. When
  it does execute, the request path is byte-scanned three times: once by the
  pre-check, once inside `__static_resolve`, once inside `__dynamic_resolve`.
  The alternative branch at `:384` (taken when `static_any_verb` holds) does two
  scans, matching B1.
- **Impact:** Medium — narrower than B1 because it requires a specific route-table
  shape, but where it applies it adds a third full pass. `split_verb` is also the
  scalar reverse scan of R8, so it is the least efficient of the three passes.
- **Remediation:** Falls out of B1's fix for free: once the verb split is hoisted
  above the branch and its result threaded into both resolvers, the pre-check
  reads a local rather than re-scanning.
- **Evidence:** inferred from code reading. Confirming benchmark: **none exists.**
  `crates/routerama/benches/common/mixed_scenarios.rs` declares no `:verb` route,
  so `splits_verbs()` is false and this path never runs under measurement. Adding
  a `mixed_verb_fallback` scenario to `routerama_mixed.rs` + `_cg` is the single
  highest-value benchmark addition for this group.

#### B3. Generated `resolve` and `__resolve_checked` are unconditionally `#[inline]`, regardless of route-table size

- **Location:** `crates/routerama_build/src/codegen.rs:223` and `:235`; also
  `crates/routerama_build/src/macro_impl/resolver.rs:440` and `:457`
- **Issue:** The generator emits `#[inline]` on the resolve entry points with no
  regard for how large the emitted body will be. Combined with B4 (`emit_node`
  recursively inlines the *entire* trie into one function body), a 5000-route
  table produces one enormous `#[inline]` function. `#[inline]` makes the function
  a candidate for cross-crate inlining and forces its MIR into every consuming
  crate's metadata; at 5000 routes this is both an instruction-cache hazard at
  runtime and a compile-time cost at every call site.
  `docs/performance.md` asks for `#[inline]` on *small* exported hot-path
  functions and asks to "be judicious"; an unconditional annotation on a
  size-unbounded generated body is not judicious.
- **Impact:** Medium — harmless for the small tables the benchmarks use, and
  potentially significant for the large tables the crate explicitly advertises
  support for (`generator_scaling.rs` benchmarks up to 5000 routes).
- **Remediation:** Have the generator count emitted routes (it already knows them)
  and emit `#[inline]` only below a threshold — a few hundred routes, tuned by
  measurement. Above it, emit nothing (letting the optimiser decide) rather than
  `#[inline(never)]`, since `docs/performance.md` rule 3 requires justification
  for `inline(never)`.
- **Evidence:** inferred from code reading. Confirming benchmark:
  `crates/routerama_build/benches/generator_scaling.rs` already generates 50 /
  500 / 5000-route tables, but it measures only `Generator::generate()` (the
  token-stream production), not compiling or running the result. The confirming
  measurement is `cargo llvm-lines` or a plain `.text` size comparison on a crate
  that uses a 5000-route table, plus a `routerama_static_cg` variant at 5000
  routes to see whether i-cache pressure shows up.

#### B4. `emit_node` inlines the entire route trie into one function body via unbounded recursion

- **Location:** `crates/routerama_build/src/codegen.rs:427-521`
- **Issue:** `emit_node` recurses over the trie and emits each child's dispatch
  inline into the parent's `match` arm. There is no depth or size cap and no
  mechanism to break a large trie into separate functions. The result is a single
  function whose size is linear in the total route count. Beyond the runtime
  concern in B3, this imposes a **compile-time** cost on every downstream crate:
  rustc's borrow checker, MIR optimisations and LLVM all scale super-linearly in
  some passes with function body size, and a single 5000-route function is a
  known pathological shape.
  A secondary consequence: the recursion itself is unbounded, so a deeply nested
  route table could overflow the *generator's* stack at build time. (`routerama`'s
  runtime `RtNode` construction was explicitly made iterative to avoid exactly
  this — see the "considered and ruled out" note for that crate — so the hazard is
  recognised elsewhere in the codebase but not here.)
- **Impact:** Medium — compile-time cost is paid by every downstream build, on
  every build, and is invisible to every benchmark in the repository.
- **Remediation:** Emit a separate `#[inline(never)]`-free helper function per
  subtree once a node's emitted size exceeds a threshold, and call it. This keeps
  each function small enough for LLVM to handle well and lets the optimiser inline
  the hot ones. Converting `emit_node`'s recursion to an explicit worklist would
  additionally remove the build-time stack-overflow hazard, matching what
  `rt_node.rs` already does at runtime.
- **Evidence:** inferred from code reading. Confirming measurement: `cargo build
  --timings` on a crate with a 5000-route table, and `cargo llvm-lines` to show
  the single-function blowup. `generator_scaling.rs` cannot see this because it
  stops at token-stream production.

#### B5. Generated literal dispatch is a linear byte-string `match` chain

- **Location:** `crates/routerama_build/src/codegen.rs:444-459`
- **Issue:** For each trie node the generator emits
  `match __seg { b"foo" => …, b"bar" => …, … }` over the sibling literals. rustc
  lowers byte-string slice patterns as a sequence of length checks and `memcmp`
  calls rather than as a jump table or a hash, so a node with 200 literal
  siblings becomes up to 200 comparisons. This is the compile-time-generated
  analogue of R3, and it is arguably worse: the runtime trie at least orders
  siblings by descendant weight (`rt_node.rs:161-179`), whereas the emitted match
  arms are in whatever order the trie iteration produces.
- **Impact:** Medium for wide tables, negligible for narrow ones. rustc does group
  slice patterns by length before comparing, which turns the worst case into
  "linear within a length class" rather than "linear overall" — that materially
  softens the finding, and is the reason this is Medium rather than High.
- **Remediation:** For nodes above a sibling threshold, emit a `match
  __seg.len()` outer dispatch (making the length grouping explicit rather than
  relying on rustc's lowering) and, above a larger threshold, a compile-time
  perfect hash over the sibling keys — the keys are all known at generation time,
  which is the ideal case for a perfect hash. Both are contained changes to
  `emit_node`. Order the arms by descendant weight in the meantime; that is a
  one-line sort and matches what the runtime trie already does.
- **Evidence:** inferred from code reading. **Low confidence on the exact rustc
  lowering** — confirming this properly needs disassembly of a generated
  resolver, which cannot be done in this container. Confirming benchmark: a
  `routerama_static_cg` scenario with 200+ literal siblings at one node, compared
  against `criterion_routers.rs`'s `matchit` baseline for the same table.

#### B6. `syn` is pulled with `full` + `visit` + `derive` as a *default* feature, imposing the heaviest configuration on every downstream compile

- **Location:** `crates/routerama_build/Cargo.toml` (the `syn` dependency and the
  `codegen` default feature); usage at
  `crates/routerama_build/src/macro_impl/field.rs:9,91-112` and
  `crates/routerama_build/src/macro_impl/query.rs:822-823`
- **Issue:** The `syn` feature list is
  `["clone-impls", "parsing", "proc-macro", "printing", "derive", "full",
  "visit"]`. `full` (parse arbitrary Rust items, not just derive input) and
  `visit` (generated visitor traits over the whole AST) are the two most
  expensive `syn` features — together they roughly triple `syn`'s own compile
  time and code size. `routerama_macros` depends on `routerama_build` with
  `default-features = false, features = ["codegen"]`, but `codegen` is what pulls
  `syn`, so the reduction buys nothing for macro users.
  `visit` **is** genuinely used (the `field.rs` and `query.rs` sites above), so
  this is not dead weight — but it is worth confirming that `full` is required.
  The usage sites are derive-input traversal, which `derive` alone may cover.
- **Impact:** Medium (compile time) — `syn` with `full` is one of the most
  expensive dependencies in the Rust ecosystem, and this cost lands on every
  crate that uses `routerama`'s macros, on every clean build.
  `docs/performance.md` asks for deviations from ecosystem defaults to be
  justified; `full` + `visit` is a deviation from the usual derive-macro
  configuration and no justification is recorded in the manifest.
- **Remediation:** Audit whether `full` is reachable. If the macro only ever
  parses derive input and attribute arguments, `derive` + `parsing` +
  `printing` + `visit` suffices and `full` can be dropped. If it is required,
  record why in a manifest comment. Separately, consider whether the `visit`
  traversals in `field.rs`/`query.rs` can be replaced by a hand-written match —
  they appear to be shallow.
- **Evidence:** inferred from code reading (feature list and usage sites both
  read). Confirming measurement: `cargo build --timings` on a downstream crate,
  with and without `full`.

#### B7. Generated query `decode_field` is `#[inline(always)]` over a linear key-match chain

- **Location:** `crates/routerama_build/src/macro_impl/query.rs` (the
  `decode_field` emission, ~`:750-780`)
- **Issue:** The generated field decoder is
  `#[inline(always)] fn decode_field(key: &str, …) { match key { "a" | "alias" =>
  …, "b" => …, … } }` — a linear string-comparison chain, force-inlined at every
  call site. For a query schema with many parameters this duplicates a large
  match at each call site, and the match itself is O(parameters) per query pair.
- **Impact:** Low — query schemas are typically small (a handful of parameters),
  and `#[inline(always)]` here is **rule-3 compliant**: the emission carries a
  documented `#[expect(clippy::inline_always, reason = …)]` citing a Callgrind
  measurement, which is exactly the justification `docs/performance.md` requires.
  Reported for completeness rather than as an action item.
- **Remediation:** None recommended at current schema sizes. If schemas grow, the
  same length-dispatch/perfect-hash argument as B5 applies, and the
  `#[inline(always)]` should be reconsidered above a parameter threshold.
- **Evidence:** inferred from code reading. Confirming benchmark:
  `crates/routerama/benches/routerama_query_cg.rs` with a wide schema
  (20+ parameters) versus the current narrow one.

#### B8. Per-node `Vec` allocation and sort in `affix_edges_in_match_order`

- **Location:** `crates/routerama_build/src/trie.rs:304-313`; related build-time
  work at `:210-231` (`check_bucket` builds a `BTreeMap` and `format!`s
  diagnostics) and `crates/routerama_build/src/codegen.rs` `emit_leaves`
  (quadratic group `find`)
- **Issue:** `affix_edges_in_match_order` allocates a `Vec` and sorts it for every
  node it is called on, and is called from both the runtime-trie compile and the
  code generator. `check_bucket` builds a `BTreeMap` and formats strings even when
  no diagnostic is produced. `emit_leaves` does a quadratic `find` over groups.
- **Impact:** Low — all of this is build-time, executed once per node per build,
  and `docs/performance.md` deprioritises first-insert cost. It becomes relevant
  only at the 5000-route scale that `generator_scaling.rs` benchmarks, where it
  contributes to the downstream compile-time cost noted in B4/B6.
- **Remediation:** Only if `generator_scaling.rs` shows super-linear growth: hoist
  the sort out of the per-node call by computing match order once at trie
  construction, and make `check_bucket` build its diagnostic lazily.
- **Evidence:** inferred from code reading. Confirming benchmark:
  `crates/routerama_build/benches/generator_scaling.rs` already parameterises over
  50 / 500 / 5000 routes — if the per-route cost is flat across those three
  points, this is a non-issue; if it grows, this is where to look. **That data is
  already obtainable from the existing benchmark.**

### Benchmark coverage

`crates/routerama_build/benches/generator_scaling.rs` (70 lines) is the only
benchmark. It parameterises `Generator::generate()` over three route shapes
(literals, captures, affixes) × three table sizes (50, 500, 5000), with
`sample_size(10)` and a 3-second measurement time — appropriate settings for an
expensive build-time operation.

There is **no `_cg` twin**. `docs/naming.md` requires every `*_cg.rs` to have a
Criterion counterpart but not the reverse, so this is compliant. It is also the
right call: the operation is far too large for instruction-count measurement to
add anything wall-clock does not.

Gaps:

- **Only `generate()` is measured.** Route *registration* (`Generator::add_all`,
  `Route::new`) and trie *construction* (`trie.rs`) run inside the benchmark's
  setup function, outside the measured region — so B8's per-node allocations are
  invisible.
- **The generated code is never compiled or executed by any benchmark in this
  crate.** B3, B4 and B5 are all about the *output*, and nothing measures the
  output. The `routerama` benchmarks do exercise generated code, but only for
  small hand-written tables — never at the 5000-route scale this benchmark
  generates. Bridging that gap (generating a 5000-route resolver into
  `routerama`'s benchmark fixtures) would make B3/B4/B5 measurable.
- **Downstream compile time is not measured at all** — no `cargo build --timings`
  or `llvm-lines` gate, so B4 and B6 can regress silently.
- **The query-derive generator (`macro_impl/query.rs`) has no scaling benchmark**,
  so B7's wide-schema case is unmeasured.

### Considered and ruled out

- **Token-stream construction style.** The generator builds `proc_macro2::
  TokenStream`s with `quote!`, which is the ecosystem-standard approach.
  Hand-rolling token construction would be faster but is a clear violation of
  "stay idiomatic" and "justify deviations from ecosystem patterns".
- **`Generator::full_api(false)`.** Trimming the generated API surface is already
  offered as a knob and is used by the benchmark itself.
- **String formatting in diagnostics.** `format!` in error paths is fine; errors
  are cold by construction.
- **`generator_scaling.rs`'s `sample_size(10)`.** Correct for an operation this
  expensive; Criterion's default 100 would make the benchmark take minutes.

---

## Crate: routerama_macros

### Summary

**No performance issues found in this crate's own code.**

`crates/routerama_macros/src/lib.rs` is 187 lines and contains exactly three
public functions, each a `#[proc_macro]` / `#[proc_macro_derive]` entry point that
does nothing but forward its `TokenStream` to the corresponding function in
`routerama_build::macro_impl`. There is no logic, no data structure, no
allocation beyond what the forwarded call makes, and no runtime code — the crate
produces nothing that executes in a consumer's binary.

The census showing 0 `#[inline]` across 3 public functions is **correct and should
not be changed**: `#[proc_macro]` entry points cannot be inlined and annotating
them would be meaningless.

The only performance surface this crate has is the compile-time cost it transitively
imposes, and that cost belongs entirely to `routerama_build` — it is recorded as
B6 (heavy `syn` configuration) rather than duplicated here. The manifest already
does the right thing by depending on `routerama_build` with
`default-features = false, features = ["codegen"]`, minimising what it pulls in;
the residual cost is that `codegen` itself is what requires `syn`'s `full` +
`visit`.

### Findings

None.

### Benchmark coverage

No benchmarks, and none are appropriate. A proc-macro shim has no measurable
runtime behaviour of its own, and its compile-time cost is a property of
`routerama_build` (covered by `generator_scaling.rs`) and of the `syn`
dependency graph (which needs `cargo build --timings`, not a Criterion harness).

The absence of a `benches/` directory here is correct and should not be flagged by
any future coverage audit.

### Considered and ruled out

- **`#[inline]` on the three entry points.** Impossible and meaningless for
  `#[proc_macro]` functions — the census's 0/3 is the right answer.
- **Inlining `macro_impl` into this crate to avoid a crate boundary.** The
  boundary exists so `routerama_build` can be used directly from a `build.rs`
  without going through a proc macro. Removing it would break a supported use
  case for no measurable gain, since proc-macro invocation cost is dominated by
  parsing and code generation, not by the call.
- **Reducing the `routerama_build` dependency further.** `default-features =
  false, features = ["codegen"]` is already the minimum that makes the macros
  work. The remaining weight is inside `codegen` — see B6.
- **The `TokenStream` → `proc_macro2::TokenStream` conversions at the boundary.**
  These are unavoidable (the `proc_macro` types are not usable outside a
  proc-macro context) and are ecosystem-standard.
- **Panic-vs-`compile_error!` error reporting.** A correctness/diagnostics
  question, not a performance one.

---

## Crate: http_extensions

### Summary

This is the weakest crate in the scope on both axes.

**Inlining:** 114 public functions, **zero** `#[inline]` annotations
(empirically verified). Many of these are one-or-two-line accessors on extension
traits that are, by construction, called across a crate boundary on every
request — `HeaderMapExt::get_str_value`, `StatusExt::ensure_success`,
`RequestExt::path_and_query`, `UriTemplateLabel::as_str`, `RouterContext`'s
getters, `RequestUris::original` / `routed` / `set_routed`,
`HttpBody::content_length` / `is_empty`. `docs/performance.md` rule 1 says such
functions should carry `#[inline]` even without a measured benefit. The reason
this was never noticed is almost certainly `[profile.bench]`'s `lto = "fat"`,
which inlines them anyway inside the benchmark binary — a consumer building with
plain `[profile.release]` gets real cross-crate calls.

**Cloning:** the routing path clones `Uri` up to three times per attempt, one of
which is provably dead work on the common path.

**Allocation:** `ExtensionsExt::uri_template_label` allocates a `String` per call
on the telemetry path for the common (non-templated) URI shape.

Ten findings.

### Findings

#### H1. Zero `#[inline]` across 114 public functions, including per-request accessors

- **Location:** crate-wide. Representative sites:
  `crates/http_extensions/src/extensions/header_map_ext.rs` (all methods),
  `crates/http_extensions/src/extensions/status_ext.rs` (`ensure_success`,
  `recovery`), `crates/http_extensions/src/extensions/request_ext.rs:40-55`,
  `crates/http_extensions/src/uri_template_label.rs` (`as_str`),
  `crates/http_extensions/src/routing/router_context.rs` (all getters),
  `crates/http_extensions/src/body/mod.rs` (`content_length`, `is_empty`)
- **Issue:** Every one of these is a small, non-generic, exported function on a
  per-request path — the exact category `docs/performance.md` rule 1 names. With
  no `#[inline]`, rustc does not export their MIR, so a downstream crate compiled
  with the default `[profile.release]` (no LTO, 16 codegen units) emits a real
  call for each. A request handler that reads three headers, checks a status and
  fetches the template label pays five cross-crate calls that should be zero
  instructions each. Compare `routerama`, which annotates 48 of its 60 public
  functions.
- **Impact:** Medium-to-High in aggregate — individually trivial, collectively a
  measurable per-request tax, and it applies to every consumer.
- **Remediation:** Add `#[inline]` to the small non-generic public accessors. This
  is mechanical and low-risk. Do **not** blanket-annotate all 114 — generic
  functions and anything with a substantial body fall under rule 2 and need
  measurement first. The right set is roughly: all extension-trait accessors, all
  `RouterContext` / `RequestUris` getters, `UriTemplateLabel::as_str`,
  `HttpBody::content_length` / `is_empty`, and the `StatusExt` predicates.
- **Evidence:** **empirically verified** (grep census: 114 `pub fn`, 0
  `#[inline*]`). The *consequence* is inferred from code reading. Confirming
  benchmark: `crates/http_extensions/benches/router_resolve_cg.rs` will **not**
  show this, because `[profile.bench]`'s `lto = "fat"` inlines them regardless —
  that is precisely the trap. Confirming it requires either building the
  benchmark with `lto = "off"`, or `cargo asm` / `objdump` on a downstream crate
  built with default `[profile.release]` to observe the `call` instructions.
  **This interaction with `[profile.bench]` is the reason the defect survived.**
- **Philosophy note:** none — this finding *aligns* with `docs/performance.md`
  rule 1. The conflict is between the repository's benchmark profile and its own
  inlining guidance, not between this finding and the guidance.

#### H2. `Router::resolve_request_uri` performs a `Uri` clone that is dead on the common path

- **Location:** `crates/http_extensions/src/routing/router.rs:284-326`,
  specifically `:294`
- **Issue:** The function clones a `Uri` at `:291` (`uris.original().clone()`),
  `:292` (`request.uri().clone()`) and again at `:294` (`original.clone()`). The
  `:294` clone exists to keep `original` alive for the
  `get_or_insert_with(|| RequestUris::new(original))` at `:322` — but that
  closure **only fires when the `RequestUris` extension is absent**. On the
  common path (a request built by `HttpRequestBuilder`, which inserts
  `RequestUris` at build time — see H7), the extension is present, the closure
  never runs, and the clone at `:294` is pure waste. This runs on every attempt,
  including every retry and every hedged request.
- **Impact:** Medium — `http::Uri` is `Bytes`-backed so the clone is an atomic
  refcount increment rather than an allocation, but an unnecessary atomic RMW per
  request per retry is real, and it is on a contended cache line when the same
  `Bytes` is shared across hedged attempts.
- **Remediation:** Surgical. Restructure so `original` is moved into the
  `get_or_insert_with` closure only on the branch that can actually need it —
  e.g. match on `extensions.get::<RequestUris>()` explicitly and clone only in the
  `None` arm, instead of cloning eagerly to satisfy the borrow checker.
- **Evidence:** inferred from code reading (all four clone sites and the
  `get_or_insert_with` consumer were traced). Confirming benchmark:
  `crates/http_extensions/benches/router_resolve_cg.rs` — Callgrind will show the
  atomic-increment pair disappearing. This is measurable with the benchmarks that
  already exist.

#### H3. `Router::resolve_request_uri` clones the resolved `Uri` for hand-built requests

- **Location:** `crates/http_extensions/src/routing/router.rs:306-307`
- **Issue:** `resolved.clone()` on the branch taken when the request did not come
  from `HttpRequestBuilder`. Same `Bytes` refcount cost as H2, but here the clone
  is genuinely needed by the current structure (the value is both stored and
  returned).
- **Impact:** Low — one atomic increment, on the less common path.
- **Remediation:** Return a borrow, or restructure to store last and return the
  moved value. Only worth doing alongside H2, as part of the same touch.
- **Evidence:** inferred from code reading. Confirming benchmark:
  `router_resolve_cg.rs` with a hand-constructed `http::Request` (the current
  fixture uses the builder).

#### H4. A `BaseUri` is cloned per request by the fixed resolver and by the fallback closure

- **Location:** `crates/http_extensions/src/routing/router.rs:332` (`Resolver::
  Fixed(base_uri) => Some(base_uri.clone())`) and `:162` (the `fallback`
  closure's capture clone)
- **Issue:** `BaseUri` is `{ origin: Origin { scheme: http::uri::Scheme,
  authority: http::uri::Authority }, path: BasePath { inner: PathAndQuery } }`.
  Every leaf is a `Bytes`-backed `http` crate type, so a `BaseUri` clone is
  **three atomic refcount increments**, not three allocations. But it happens on
  every request for the `Fixed` resolver, which is the simplest and therefore
  most common configuration.
- **Impact:** Low — three atomics per request. Under high concurrency the
  refcount cache lines are shared across all worker threads, so this is
  contention, not just instruction count.
- **Remediation:** Have `Resolver::Fixed` return `&BaseUri` where the caller can
  use a borrow, or store the `BaseUri` in an `Arc` so a clone is one atomic
  instead of three. The latter is a one-field change.
- **Evidence:** inferred from code reading; the `Bytes`-backed representation was
  confirmed by tracing `templated_uri`'s `BaseUri` / `Origin` / `BasePath`
  definitions. Confirming benchmark: `router_resolve_cg.rs` — Callgrind's
  instruction counts will show the `lock inc` sequences; a multi-threaded
  contention benchmark does not exist and would be needed to see the sharing cost.

#### H5. `ExtensionsExt::uri_template_label` allocates a `String` per call for non-templated URIs

- **Location:** `crates/http_extensions/src/extensions/extensions_ext.rs:22-33`,
  specifically `:28`; root cause at
  `crates/templated_uri/src/path_and_query.rs:71-76`
- **Issue:** The method returns an owned `UriTemplateLabel`. At `:25` it first
  tries `path.label()`, which for a `PathAndQuery` in the `Static` classification
  returns `None` (`templated_uri/src/path_and_query.rs:82-86`). It then falls
  through to `path.template()` at `:28` — and `template()`'s `Static` arm returns
  `Cow::Owned(classified_pq.declassify_ref().to_string())`, **a real heap
  allocation**. So for any request whose URI is not a captured template — the
  common case for a plain HTTP client, and the case for every static route — this
  accessor allocates a `String` on the telemetry path, which is called once per
  request per emitted metric or span.
  The `Captured` arm at `:25` is cheap: `UriTemplateLabel` wraps
  `Cow<'static, str>` and the captured label is already `'static`.
- **Impact:** Medium — a per-request allocation on the observability path, which
  is exactly what `docs/performance.md`'s "no allocation on the hot path" rule
  targets. Telemetry that allocates per request is a classic source of
  tail-latency noise.
- **Remediation:** Two options, both contained. (a) Have `template()` return
  `Cow::Borrowed` for the `Static` arm — it is borrowing from data the
  `PathAndQuery` already owns, so this is a lifetime plumbing change in
  `templated_uri`, not a redesign. (b) Add a borrowing
  `uri_template_label_ref(&self) -> Option<&str>` to `ExtensionsExt` so telemetry
  callers, which only need to format the value, never force the owned form. (b)
  is the smaller change and does not touch another crate.
- **Evidence:** inferred from code reading (both `extensions_ext.rs` and
  `templated_uri`'s `PathAndQuery::template` / `label` were read). Confirming
  benchmark: **none exists** — no benchmark touches `ExtensionsExt` at all. A
  `http_extensions_ext/uri_template_label` Criterion group plus a `_cg` twin,
  parameterised over `Static` and `Captured` path shapes, would show the `malloc`
  on the `Static` case immediately.

#### H6. `RequestExt::resolve_uri` clones a `Uri` per call with no borrowing alternative in the public API

- **Location:** `crates/http_extensions/src/extensions/request_ext.rs:40-55`
- **Issue:** `resolve_uri` returns an owned `Uri`, cloning per call. There is no
  `resolve_uri_ref` or equivalent, so a caller that only wants to inspect the path
  — the common case for logging, metrics and authorisation checks — is forced
  into the clone by the API shape. This is the "public API forcing callers into
  slow paths" pattern.
- **Impact:** Medium — one `Bytes` refcount increment per call, and the API gives
  callers no way to avoid it. The cost is small; the fact that it is unavoidable
  is the finding.
- **Remediation:** Add a borrowing variant. `Uri` is not trivially borrowable as a
  whole, but `path_and_query()` and `path()` are, and those cover the inspection
  use cases. Adding `fn resolve_path_and_query(&self) -> Option<&PathAndQuery>`
  is additive and non-breaking.
- **Evidence:** inferred from code reading. Confirming benchmark: none exists;
  fold into the proposed `http_extensions_ext` group above.

#### H7. `HttpRequestBuilder::build` performs three separate `Extensions::insert` calls plus a path render

- **Location:** `crates/http_extensions/src/http_request_builder.rs:359-391`,
  specifically `:378-379` and `:382`, `:384`, `:387`
- **Issue:** `build` calls `uri.to_path_and_query()` at `:378` (renders a string),
  `PathAndQuery::try_from` at `:379` (parses it back), then does three separate
  `request.extensions_mut().insert(…)` calls at `:382`, `:384` and `:387`.
  `http::Extensions` is a lazily-allocated boxed `AnyMap`: the first insert
  allocates the map, and **each insert boxes its value**. So a built request costs
  at least four allocations for the extension bookkeeping alone, plus the string
  round-trip at `:378-379`.
- **Impact:** Medium — this is once per outbound request, not per byte, but it is
  a fixed four-plus-allocation tax on every request the builder produces, and the
  render-then-reparse at `:378-379` is redundant work (the builder already knows
  the components it just serialised).
- **Remediation:** (a) Bundle the three extension values into a single struct and
  insert it once — one box instead of three, and one type lookup instead of three
  on the read side too. (b) Construct the `PathAndQuery` directly from the
  components rather than rendering to a string and reparsing; `PathAndQuery` has
  no public component constructor, so this may require a helper in
  `templated_uri`, making (b) the larger change. Do (a) first.
- **Evidence:** inferred from code reading. Confirming benchmark:
  `crates/http_extensions/benches/http_request_builder.rs` **already exists** and
  measures exactly this function — the allocation count is measurable today. It
  has no `_cg` twin, which is the gap that hides the allocation count (wall-clock
  will not cleanly resolve three `malloc`s). Adding
  `http_request_builder_cg.rs` is the single most valuable benchmark addition for
  this crate.

#### H8. `collect_with_limit` accumulates into an uncapacitied `Vec` and has no single-fragment fast path

- **Location:** `crates/http_extensions/src/body/mod.rs:593-622`, specifically
  `:599` and `:606`
- **Issue:** The fragment accumulator starts as `Vec::new()` at `:599` and grows
  by doubling as frames arrive, and at `:606` the result always goes through
  `BytesView::from_views(fragments)` even when exactly one fragment was
  collected — the overwhelmingly common case for a small JSON response body,
  which arrives in a single frame. The single-fragment case can return that
  fragment directly with no `Vec` and no view assembly at all.
  A capacity hint is available: `Body::size_hint()` is part of the `http_body`
  trait and the function already has the body in hand.
- **Impact:** Medium — body collection is per-response and this is the code path
  every JSON API client takes. Two avoidable allocations (the `Vec`, plus
  whatever `from_views` builds) on the most common shape.
- **Remediation:** Peek the first frame; if the body then reports complete, return
  it directly without ever creating the `Vec`. Otherwise seed the `Vec` from
  `size_hint().lower()`. Both are contained changes inside this one function.
- **Evidence:** inferred from code reading. Confirming benchmark: **none exists** —
  body collection is entirely unbenchmarked. A `http_body_collect` Criterion group
  (single-frame, multi-frame, and limit-exceeded cases) plus a `_cg` twin is
  needed; the `_cg` twin is what shows the allocation-count change.

#### H9. Streaming bodies are `Pin<Box<dyn Body>>`, adding an allocation and a virtual call per poll

- **Location:** `crates/http_extensions/src/body/mod.rs:572-581`, specifically the
  `Kind::Body(Pin<Box<dyn Body<…>>>, HttpBodyOptions)` variant at `:580`
- **Issue:** Every streaming body is boxed and polled through a vtable. The enum
  is also flagged `clippy::large_enum_variant` with an `#[expect]`, meaning every
  `HttpBody` — including the common in-memory `Bytes` case — carries the larger
  variant's footprint.
- **Impact:** Low — one allocation at body construction (not per poll), and one
  indirect call per poll. Genuinely hard to avoid: the alternative is making
  `HttpBody` generic over the body type, which would infect every signature in
  the crate and every consumer.
- **Remediation:** **None recommended.** Making `HttpBody` generic is an
  architectural change, and `docs/performance.md` explicitly prefers surgical over
  architectural. Boxing a `dyn Body` is also the ecosystem-standard shape (`axum`,
  `hyper`, `reqwest` all do it), so deviating would need strong justification.
  Recorded so it is not re-discovered.
- **Evidence:** inferred from code reading. Confirming benchmark: none, and none
  is worth writing given no action is recommended.
- **Philosophy note:** this finding is reported but explicitly **not actionable**
  under house philosophy — it is architectural and it matches ecosystem practice.

#### H10. `tick` is a non-optional dependency pulled with the `fmt` feature

- **Location:** `crates/http_extensions/Cargo.toml`
- **Issue:** `default = []` is good — the crate defaults to nothing, and `json`
  correctly gates `serde_core` / `serde_json`. But `tick` is pulled
  unconditionally with `features = ["fmt"]`, `thread_aware` with
  `["derive", "std"]`, and `bytesbuf` with `["bytes-compat", "std"]`. The `fmt`
  feature on `tick` pulls formatting machinery into every consumer whether or not
  they format a timestamp, which is compile-time cost and binary size for a
  capability most consumers do not use on the request path.
- **Impact:** Low — compile time and binary size rather than runtime.
- **Remediation:** Check whether `tick/fmt` is used outside `Display`
  implementations and error messages. If not, gate it behind an optional feature
  that the timeout/observability paths enable.
- **Evidence:** inferred from code reading (manifest read; usage not exhaustively
  traced — **low confidence**, verify before acting). Confirming measurement:
  `cargo build --timings` and `cargo tree -f '{p} {f}'` with and without.

### Benchmark coverage

| File | Callgrind twin | Covers |
|---|---|---|
| `benches/router_resolve.rs` | `benches/router_resolve_cg.rs` ✅ | `Router::resolve_request_uri` |
| `benches/http_request_builder.rs` | ❌ none | `HttpRequestBuilder::build` |
| `benches/http_response_builder.rs` | ❌ none | response construction |

The routing path is properly covered with a Criterion + Callgrind pair —
**correcting round 1's claim that this crate had no routing benchmark.** Note
however that `router_resolve_cg.rs` cannot see H1 (the missing `#[inline]`s),
because `[profile.bench]`'s fat LTO inlines them anyway.

Gaps, in priority order:

1. **No `_cg` twin for `http_request_builder.rs`.** This is the highest-value
   addition: H7 is an allocation-count finding, and Criterion wall-clock cannot
   resolve three `malloc`s. The Criterion file already exists, so the twin is
   cheap to add and `docs/naming.md` compliance is automatic.
2. **Body collection is entirely unbenchmarked** — `collect_with_limit` (H8),
   `into_bytes`, the limit-exceeded path, and the timeout bodies. This is a
   per-response path with known allocation behaviour and no measurement at all.
3. **JSON is entirely unbenchmarked** — `into_json` / `into_json_ref`, despite
   `json` being a first-class feature and benchmarks building `--all-features`
   (so the code is compiled into every benchmark binary but never executed).
4. **Every extension trait is unbenchmarked** — `HeaderMapExt`, `StatusExt`,
   `ExtensionsExt`, `RequestExt`. This is almost certainly *why* H1 and H5 were
   never noticed: there is no measurement that would have shown them.
5. **No multi-threaded benchmark**, so H4's refcount contention is unobservable.

### Considered and ruled out

- **`default = []` in the manifest.** Correct and commendable — consumers opt in
  to `json` and everything else.
- **`Kind::Bytes(Option<BytesView>)`.** The in-memory body case is already
  allocation-free once constructed.
- **`StatusExt::recovery` / `ensure_success`.** Trivial predicate chains over a
  `u16`; the only issue is the missing `#[inline]`, already covered by H1.
- **`http_utils.rs` helpers.** Small, straightforward, no allocation. Missing
  `#[inline]` only, covered by H1.
- **`RouterContext`.** A plain struct of borrowed/cheap fields; the getters are
  correct, they just need `#[inline]` (H1).
- **`UriTemplateLabel`.** Wrapping `Cow<'static, str>` is the right choice — it
  makes the captured-template case free. The problem is upstream in
  `PathAndQuery::template()` (H5), not here.
- **Error types.** Error construction is cold by definition; no `Result`
  inflation observed on the hot signatures.
- **Making `HttpBody` generic** — see H9's philosophy note.

---

## Crate: http_path_template

### Summary

A small, well-built crate. `ParseError` is a model of how to keep an error type
off the hot path (16 bytes, backtrace boxed only when actually captured, no
`Result` inflation — `Result<PathTemplate, ParseError>` is 32 bytes, identical to
`PathTemplate` itself). Its benchmark pair `hpt_parse.rs` / `hpt_parse_cg.rs` is
the best in this scope: it covers every grammar shape including the error path,
and its module doc explicitly explains *why* the Callgrind twin exists ("wall-clock
cannot reliably resolve a single eliminated allocation or branch"). This is the
pattern the other four crates should copy.

The findings are correspondingly modest: a missing-`#[inline]` census gap that
round 1 overlooked entirely, one genuinely redundant full pass over the template
during parsing, and an eager scan in an iterator constructor.

Five findings.

### Findings

#### P1. Zero `#[inline]` across 19 public functions

- **Location:** crate-wide. Representative sites:
  `crates/http_path_template/src/path_template.rs` (`segments`, `verb`),
  `crates/http_path_template/src/variable.rs` (`name`, `field_path`, `sub`),
  `crates/http_path_template/src/error.rs` (accessors)
- **Issue:** Same category as H1 — small, non-generic, exported accessors that
  cross a crate boundary. `PathTemplate::segments()` and `Variable::name()` are
  called by `routerama_build` during trie construction and by any consumer walking
  a parsed template. `docs/performance.md` rule 1 applies directly.
- **Impact:** Low-to-Medium — lower than H1 because most callers are build-time
  (`routerama_build`), not per-request. Still a rule-1 gap, and round 1 missed it
  entirely.
- **Remediation:** Annotate the trivial accessors. Skip anything generic or
  substantial (rule 2).
- **Evidence:** **empirically verified** (grep census: 19 `pub fn`, 0
  `#[inline*]`). Confirming benchmark: as with H1, `hpt_parse_cg.rs` will not show
  it under `[profile.bench]`'s fat LTO. Needs `cargo asm` on a downstream crate
  built with default `[profile.release]`.

#### P2. `PathTemplate::parse` makes three full passes over the template, one of which exists only to size a `Vec`

- **Location:** `crates/http_path_template/src/path_template.rs:82-111`, with
  `split_verb` at `:236-277`, `segment_count_hint` at `:319-331`, and the real
  segmentation at `:285-313`
- **Issue:** `parse` calls `split_verb` at `:94` (a full byte pass looking for the
  verb delimiter), then `segment_count_hint` at `:288` (**a second full byte pass
  whose sole purpose is to count `/` so the segment `Vec` can be sized**), then
  the actual segmentation pass at `:292-306` — which counts the same delimiters
  again as it goes. Plus `is_valid_literal` per segment. Three traversals of a
  string that is typically 20–60 bytes.
  The hint pass is the clearly redundant one: `split_verb` is already walking the
  whole string and could return the `/` count for free.
- **Impact:** Low-to-Medium. Templates are parsed once per route at build time in
  the `routerama_build` path, which `docs/performance.md` deprioritises — but the
  crate's own benchmark documentation notes that "a router may re-parse templates
  when (re)building its route table", so it is not purely build-time. Also, `parse`
  is `pub` and nothing stops a consumer calling it per request.
- **Remediation:** Fuse the segment count into `split_verb`'s existing pass and
  return it alongside the verb split. This is a contained change to two private
  functions with no API impact.
- **Evidence:** inferred from code reading. Confirming benchmark:
  `crates/http_path_template/benches/hpt_parse_cg.rs` **already exists and
  measures exactly this** — the instruction-count reduction on
  `hpt_parse/parse/literal_only` and `/variables` would be immediately visible.
  This is the most readily confirmable finding in the whole group.

#### P3. `Variable::segments()` scans the sub-template eagerly on every call

- **Location:** `crates/http_path_template/src/variable.rs:89-95`
- **Issue:** The iterator constructor computes
  `self.sub.bytes().filter(|&b| b == b'/').count() + 1` up front to seed a
  `remaining` counter. That is a full byte scan of the sub-template performed
  before the first `next()`, whether or not the caller ever asks for `len()` or
  `size_hint()`. A caller that just does `for s in v.segments()` pays for a scan
  it never uses.
- **Impact:** Low — sub-templates are short and this is build-time in the
  `routerama_build` path.
- **Remediation:** Either compute `remaining` lazily (only when `size_hint`/`len`
  is called, dropping the `ExactSizeIterator` guarantee), or — better — keep the
  count but compute it once at *parse* time and store it in `Variable`, since the
  sub-template is immutable after parsing. The latter preserves
  `ExactSizeIterator`, which is a real API guarantee worth keeping.
- **Evidence:** inferred from code reading. Confirming benchmark: none exists —
  `hpt_parse.rs` benchmarks `parse` only, never iteration. A
  `hpt_parse/segments` group iterating a parsed template would show it.

#### P4. `Segment` is 56 bytes, sized by its widest variant

- **Location:** `crates/http_path_template/src/path_template.rs` (`Segment`
  definition)
- **Issue:** `Segment` is **56 bytes** (empirically verified), dominated by the
  `Affix { prefix, name, suffix }` variant's three `&str` fat pointers. Literal
  and simple-variable segments — the vast majority — need 16–24 bytes but occupy
  56. A 4-segment template's `Box<[Segment]>` is 224 bytes where ~96 would do.
- **Impact:** Low — templates are small and short-lived, and the memory is
  contiguous so the extra bytes cost at most an extra cache line per template.
  Reported for completeness; it is a locality observation, not a hot-path defect.
- **Remediation:** **Not recommended.** Boxing the `Affix` variant's payload
  would trade 56 bytes of inline storage for an allocation and a pointer chase,
  which is worse for the common access pattern. Storing offsets instead of `&str`
  would shrink it but is exactly the kind of hand-rolled layout
  `docs/performance.md` asks to avoid in favour of idiomatic Rust. Recorded so it
  is not re-litigated.
- **Evidence:** **empirically verified** (rustc layout replica). Confirming
  benchmark: `hpt_parse_cg.rs`'s `affix` case already exists if anyone wants to
  test an alternative.
- **Philosophy note:** the obvious "fix" (offset-based segments) conflicts with
  the house preference for idiomatic Rust over hand-rolled layout. Hence the
  recommendation not to act.

#### P5. Affix parsing scans the segment twice

- **Location:** `crates/http_path_template/src/path_template.rs:377-400`
- **Issue:** The affix parser locates the brace delimiters and then re-derives the
  prefix/suffix boundaries from them, walking the segment a second time. Same
  shape as P2 but at segment rather than template granularity.
- **Impact:** Low — only on the extended grammar (`with_segment_affixes`), only
  on segments that actually contain an affix, and segments are short.
- **Remediation:** Capture both brace offsets in the single locating pass and
  slice directly. Contained, private.
- **Evidence:** inferred from code reading. Confirming benchmark:
  `hpt_parse_cg.rs`'s `affix` case already exists and would show the delta.

### Benchmark coverage

**The model for the workspace.** `benches/hpt_parse.rs` and
`benches/hpt_parse_cg.rs` are correctly paired per `docs/naming.md`, the Criterion
group name (`hpt_parse/parse`) is prefixed by the file basename as required, and
the module documentation explains the crate's performance thesis ("parsing a
template is this crate's entire value proposition") and *why* the Callgrind twin is
needed. Six cases cover every branch of the grammar: literal-only, variables, rest
wildcard, verb, extended-grammar affix, and — notably — the **error path**, which
most benchmark suites omit.

Gaps:

- **No scaling dimension.** All six inputs are 20–45 bytes. Nothing measures a
  long template, a deeply nested sub-template, or a template with many segments,
  so any super-linear behaviour in `parse` would be invisible. Parameterising over
  segment count would also make P2's redundant pass show up more sharply.
- **Only `parse` is measured.** `Display` / round-tripping, `Variable::segments()`
  iteration (P3) and `PathTemplate::segments()` walking are all unbenchmarked —
  and these are what `routerama_build` actually calls after parsing.
- **`Grammar` variants are not compared systematically** — `strict` and `extended`
  are each used for the cases that need them, but there is no like-for-like
  comparison isolating the extended grammar's cost.

### Considered and ruled out

- **`ParseError` design** (`crates/http_path_template/src/error.rs:55-107`).
  Empirically 16 bytes; `MaybeBacktrace` boxes only when `RUST_BACKTRACE` capture
  actually succeeds, so the no-backtrace case stores nothing.
  `Result<PathTemplate, ParseError>` is 32 bytes — **identical to `PathTemplate`
  alone**, meaning the error arm costs nothing in the success case. This is
  exactly right and should be held up as the workspace example of how to size an
  error type.
- **The `std` feature.** It gates only backtrace capture; perf-neutral by design,
  and the `no_std` build loses nothing on the parse path.
- **`Box<[Segment]>` for the segment list.** Correct — the list is immutable after
  parsing, so `Box<[…]>` over `Vec` saves the capacity word and signals intent.
  (This is the pattern R6 asks `routerama`'s `Leaf` to adopt.)
- **`&str`-borrowing segments.** `PathTemplate` borrows from the input string
  rather than copying, which is the zero-copy design the rest of this review keeps
  asking for elsewhere. Correct.
- **`is_valid_literal` per segment.** A defensive validation check;
  `docs/performance.md` explicitly says to preserve defensive runtime checks.
  Not a candidate for removal.
- **Error-path allocation.** Cold by construction.

---

## Cross-cutting recommendations

Ordered by expected value.

1. **Fix B1** (hoist the path scan above the static/dynamic branch in the
   generated resolver). It is the only High finding, the remediation is small and
   uses machinery that already exists (`Walk` already takes a `&ScannedPath`), and
   **it is measurable with the benchmark that is already in the repository**
   (`routerama_mixed_cg.rs`'s `dynamic_fallback_hit`).
2. **Add `#[inline]` to `http_extensions`' small public accessors (H1) and
   `http_path_template`'s (P1).** Mechanical, low-risk, and directly mandated by
   `docs/performance.md` rule 1. 0/114 and 0/19 against `routerama`'s 48/60 is not
   a judgement call, it is an omission.
3. **Align `[profile.bench]` with `[profile.release]`, or add a second benchmark
   profile without LTO.** Every benchmark number in the repository currently
   describes a build no consumer gets, and the fat LTO specifically hides H1 and
   P1. This is a cross-group issue but it lands hardest here, because this scope
   contains the two crates whose defect it conceals.
4. **Add `http_request_builder_cg.rs`** — the Criterion file already exists, the
   twin is cheap, and it makes H7's allocation count visible.
5. **Add a `mixed_verb_fallback` scenario** to `routerama_mixed.rs` + `_cg`, so
   B2's third path scan is measurable at all.
6. **Fix P2** (fuse `segment_count_hint` into `split_verb`) — the single most
   readily confirmable finding in the group, since `hpt_parse_cg.rs` already
   measures exactly the affected function.
7. **Benchmark `http_extensions`' body collection, JSON and extension traits.**
   The absence of these benchmarks is the proximate cause of H1, H5 and H8 going
   unnoticed. `http_path_template`'s `hpt_parse.rs` / `hpt_parse_cg.rs` pair,
   including its module documentation explaining the Criterion/Callgrind split, is
   the template to copy.
8. **Investigate the layout findings (R3, R5, R6)** as a single coordinated change
   to `routerama`'s trie: SoA literal edges, `HttpMethod` in `Leaf`, and
   `Box<str>` / `Box<[…]>` throughout `Leaf` and `VarPlan`. All three shrink the
   stride of the linear scans that dominate the trie walk, and the layout numbers
   backing them were measured, not guessed.
