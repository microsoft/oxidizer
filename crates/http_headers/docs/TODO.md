# TODO

This file tracks outstanding work for the `http_headers` family. Completed
items are deleted rather than retained as history.

## Contents

### Security

- [SEC2](#sec2) — Pin code executed with the GitHub release token
- [SEC3](#sec3) — Preserve sensitivity across typed reconstruction and forwarding

### Conformance

- [CON1](#con1) — Make checked byte-to-text conversions explicit
- [CON3](#con3) — Establish current unsafe-code verification evidence

### Performance

- [P1](#p1) — Accelerate Cache-Control fallback token validation
- [P6](#p6) — Reduce HTTP-date digit-validation overhead
- [P9](#p9) — Reuse accelerated validation and searches for entity tags
- [P10](#p10) — Evaluate Content-Type common-literal dispatch
- [P11](#p11) — Resume Content-Type lookup after the cached parameter prefix
- [P12](#p12) — Reuse delimiter searches in negotiation validation
- [P13](#p13) — Reduce Host literal and international-fallback overhead
- [P14](#p14) — Investigate Authorization validation data flow
- [P15](#p15) — Reduce repeated CORS list traversal and projection
- [P18](#p18) — Tighten CORS scalar token decoding
- [P19](#p19) — Investigate Content-Length singleton and decimal decoding
- [P21](#p21) — Reduce shared source and result construction overhead
- [P24](#p24) — Reduce retained URI and authority metadata costs
- [P25](#p25) — Compare streaming negotiation with bounded retained member metadata
- [P26](#p26) — Evaluate inline general Content-Type metadata
- [P27](#p27) — Transfer completed Allow and Vary construction buffers

### Features

- [F1](#f1) — Classify semantically sensitive typed headers with `data_privacy_core`
- [F2](#f2) — Add byte-oriented redaction before redacting typed headers
- [F3](#f3) — Convert `templated_uri::Uri` into `LocationOwned`

### Documentation

- [D1](#d1) — Document typed headers in the workspace HTTP APIs

### Benchmarks

- [B1](#b1) — Cover semantic reads separately from decode-and-drop
- [B2](#b2) — Measure source representations and ownership boundaries
- [B3](#b3) — Cover nonliteral grammar and fallback distributions
- [B4](#b4) — Gate parser instruction and allocation regressions
- [B5](#b5) — Measure downstream release code generation and layouts
- [B6](#b6) — Establish workload shares and benchmark repeatability

### Testing

- [T1](#t1) — Make counter-lock rendezvous tolerate spurious failure and abort safely

## Security

<a id="sec2"></a>
### SEC2 — Pin code executed with the GitHub release token

**Area:** shared GitHub release build boundary used by the `http_headers` family · **Priority:** Medium · **Effort:** Medium

**Severity:** Medium · **Confidence:** High · **Kind:** Hardening · **Threat:** compromise of an upstream dependency or tool release selected by a subsequent trusted release run · **Scope:** 1 credential-bearing release job with 2 mutable code-selection paths, exhaustive for this workflow

Make the code receiving the release credential a reviewed, reproducible
selection. The contents-write workflow exports `GH_TOKEN` while invoking
`just`, which launches `cargo -Zscript`. The script declares `argh = "0.1"`
without a repository-controlled script lock at this invocation, so a fresh
runner can select dependency updates not reviewed with the release commit.
Their build scripts or procedural macros inherit the token before the release
program runs. The setup action also selects any `just >=1.46.0`; that selected
executable later runs directly in the credential-bearing step.

- `.github/workflows/publish-gh-release.yml:11` — contents-write token scope
- `.github/workflows/publish-gh-release.yml:39` — token-bearing invocation
- `justfile:54` — recipe compiles and executes the Rust script
- `scripts/publish-gh-release.rs:12` — script dependency selection
- `.github/actions/anvil-setup/action.yml:115` — minimum-version rather than exact `just` selection

An upstream compromise could consequently modify repository contents or
GitHub Releases using the job token. This is conditional supply-chain
hardening, not evidence of compromise, fork-PR escalation, or exposure of a
crates.io publishing credential. Keep compilation/bootstrap outside the
credential-bearing step where possible, and ensure any code that still
receives the token uses reviewed dependency/tool identities. Change generated
Anvil setup through its generator/configuration rather than hand-editing it.

**Done when:** a clean release runner cannot select unreviewed tool or script
dependency versions, dependency compilation does not inherit the release
token, and an isolated workflow regression check with a dummy token fails for
the current mutable selections/inherited build environment without creating
a real release or accessing a real credential.

---

<a id="sec3"></a>
### SEC3 — Preserve sensitivity across typed reconstruction and forwarding

**Area:** `http_headers` normalization, compact storage and borrowed forwarding · **Priority:** High · **Effort:** Small

**Severity:** Medium · **Confidence:** Medium · **Kind:** Hardening · **Threat:** a reader of lower-trust diagnostics after an application reconstructs or forwards explicitly sensitive typed values · **Scope:** 5 typed header families, exhaustive for the normalization, compact-storage and borrowed-forwarding paths cited below

Carry sensitivity through normalization rather than reconstructing only the
wire bytes. Cache-Control and Accept-Ranges feed bare member slices into
`normalized_comma_value`, which creates a fresh nonsensitive `FieldValue`.
Their `Field::insert` implementations then send that value to the sink, even
when retained input lines were marked sensitive. The HTTP adapter faithfully
copies the already-cleared flag, so normal downstream Debug output can reveal
marked extension values. Accept-Ranges additionally discards the marker when
its owned or borrowed decoder replaces canonical `bytes`/`none` storage with
the compact representation.

- `crates/http_headers/src/headers/cache_control.rs:182` — normalization drops the retained lines' metadata; insertion uses it at line 450
- `crates/http_headers/src/headers/range/accept_ranges.rs:138` — second normalization caller; insertion uses it at line 374
- `crates/http_headers/src/headers/range/accept_ranges.rs:379` — borrowed and owned canonical shortcuts discard original storage; direct construction does likewise at line 435
- `crates/http_headers/src/headers/shared.rs:364` — helper accepts only byte slices and constructs a fresh value
- `crates/http_headers/src/field_value.rs:624` — byte-vector conversion calls the nonsensitive constructor at line 329
- `crates/http_headers/src/http_adapter.rs:189` — insertion forwards the value through the marker-preserving HTTP conversion

Preserve the marker when any contributing line is sensitive, including mixed
repeated lines and compact canonical representations. Keep existing wire
normalization, grammar validation, and nonsensitive behavior; no new privacy
dependency is needed.

**How to disprove:** show that the consuming application never marks these
headers sensitive or never exposes the resulting sink to lower-trust
diagnostics. No deployed disclosure is established by this static path.

**Done when:** custom and HTTP sink round-trip tests fail on the current code
and demonstrate retained sensitivity for single and mixed repeated lines,
owned insertion, and borrowed/owned canonical Accept-Ranges forwarding;
downstream Debug omits marked extension content and nonsensitive cases retain
their existing behavior.

**See also:** P18, P21, P24, P25
(preserve this marker before evaluating compact storage or forwarding);
F1, F2 (future privacy features, not preservation of this marker).

The same metadata-reconstruction mechanism also affects three additional
typed forwarding surfaces, exhaustively identified in these paths:

- `crates/http_headers/src/headers/cors/access_control_request_method.rs:85`
  — the registered-method shortcut retains only a static method name;
  `into_field_value` at line 95 creates `FieldValue::from_static(method)`
  without the original marker. Direct `TryFrom<FieldValue>` repeats the
  shortcut at line 397, whereas extension methods retain their input value.
- `crates/http_headers/src/sink/field_sink_ext.rs:455` — borrowed conditional
  insertion reconstructs a wildcard with `FieldValueRef::new(b"*")`, or tags
  with `FieldValueRef::new(tag.as_bytes())` at line 465. The macro instantiates
  both `IfMatchView` and `IfNoneMatchView` at lines 477–479; neither path
  transfers the contributing source lines' sensitivity.

**Additional acceptance criteria:** the same marker-preservation regressions
cover sensitive registered request methods through source decoding, direct
construction, serde and insertion, plus both borrowed conditional views
through wildcard and tagged single/mixed repeated lines. Preserve the
existing method spelling and conditional-tag wire behavior. A sensitive
entity tag must remain redacted when the resulting sink is formatted; these
are extensions of this existing root, not new privacy-classification work.

**Guideline obligation:** [M-STRONG-TYPES-GUARD](https://microsoft.github.io/rust-guidelines/guidelines/libs/resilience/#M-STRONG-TYPES-GUARD)
requires strong types to enforce their encoded invariants where applicable.
Here the retained sensitivity property must survive implicit reconstruction;
explicit caller-directed sensitivity changes remain allowed. Test the final
sink representation as well as the intermediate typed value.

---

## Conformance

<a id="con1"></a>
### CON1 — Make checked byte-to-text conversions explicit

**Area:** borrowed byte-oriented text accessors · **Priority:** Low · **Effort:** Small

**Guideline:** [C-CONV](https://rust-lang.github.io/api-guidelines/naming.html#c-conv) — should use `as_` for free borrowed projections and `to_` for expensive conversions, explicitly including borrowed UTF-8 validation · **Confidence:** High · **Scope:** 2 checked accessors, exhaustive for `UserAgentView` and `ServerView`; sampled across the wider conversion API

Make `to_str` the canonical spelling for these checked conversions. They can
scan the complete byte sequence and reject invalid UTF-8; they are not free
projections of a retained string. The two opaque header views expose
the checked operation as `as_str` while internally calling `to_str`.

- `crates/http_headers/src/headers/user_agent.rs:158` — `as_str` calls `.to_str()` at line 160
- `crates/http_headers/src/headers/negotiation/server.rs:161` — the same checked delegation is at line 163

Retain compatibility aliases if needed and explicitly document their
validation cost; do not replace validation with unchecked conversion or
rename constant-time getters such as `LocationView::as_str`. This is a naming
and discoverability correction, not a claim of measured performance loss.

**Done when:** both types offer and teach the checked `to_str` operation,
any retained `as_str` aliases clearly identify the same checked cost, and
package-scoped documentation/API checks confirm existing callers still work.
Public-API tests preserve valid ASCII and multibyte UTF-8 results and invalid
byte rejection, including unchanged header-specific error kinds. Regenerate
crate README examples from rustdoc rather than editing generated files.

---

<a id="con3"></a>
### CON3 — Establish current unsafe-code verification evidence

**Area:** `http_headers_simd` unsafe acceptance evidence · **Priority:** Medium · **Effort:** Small

**Guideline:** [M-UNSAFE](https://microsoft.github.io/rust-guidelines/guidelines/correctness/#M-UNSAFE) — unsafe code must have a valid reason, safety reasoning and passing Miri verification; performance-motivated unsafe should follow benchmarking · **Confidence:** High that current acceptance is unassessed, not that the code fails · **Scope:** 1 unsafe implementation crate; sampled ASCII, vector-load and allocator boundaries plus the existing Miri recipe

Establish revision-attributable safety verification using the existing
Anvil checks. Static inspection finds local proof comments and a configured
Miri gate, but neither proves that the current revision passes it. This
static-only audit did not run Miri or observe current CI artifacts. It found
no confirmed undefined behavior; do not describe this evidence gap as an
unsoundness finding or as missing Miri tooling.

- `crates/http_headers_simd/src/api.rs:39` — `str::from_utf8_unchecked(bytes)` follows the ASCII proof
- `crates/http_headers_simd/src/x86.rs:33` — `_mm_loadu_si128(pointer)` has a local slice-bound proof
- `crates/http_headers_simd/src/tracking.rs:127` — `unsafe impl GlobalAlloc for TrackingAllocator` delegates allocator obligations to `System`
- `justfiles/anvil/checks/miri.just:35` — existing package-scoped `anvil-miri` recipe; its implementation selects `--all-features` test targets

**Done when:** current-commit artifacts from
`just anvil-miri --package http_headers_simd` and
`just anvil-miri --package http_headers` establish the applicable checks'
outcomes, with exact toolchain, target, feature selection and test selection.
Account explicitly for any unsupported intrinsic, target, ignored test or
unselected path instead of implying full coverage. Investigate observed
failures if any; do not invent them from configuration alone. Ordinary
isolated no_std correctness checks are separate from this unsafe-code
verification, and the repository's no_std-only coverage/mutation exemptions
remain unchanged.

**See also:** B3, B5, B6 own the fresh performance evidence for unsafe
optimization decisions; this item does not duplicate their measurement work.

---

## Performance

Evaluate these candidates against the typed semantic APIs. They are
hypotheses to re-evaluate, not patches to apply blindly or promised gains.
Establish fresh equivalent-work baselines for the resulting APIs, pin the
compiler and target flags, and preserve executable paths, fixture inventories,
and measurement boundaries. Judge CPU cost using instruction counts rather
than short wall-time samples, and independently reject increases in measured
allocation counts or allocated bytes:
every existing case must remain equal or improve, including owned/borrowed,
raw/HTTP, repeated-line, fallback, and error cases. Never offset a regression
with gains elsewhere or weaken validation, source limits, error precedence,
or safety checks. Inspect assembly and additive Callgrind attribution after
each improvement. Leave `PERF.md` regeneration to the maintainer.

Prioritize the shared representation/proof boundary (P21), retained semantic
layout (P24), and negotiation traversal strategy (P25) before polishing their
inner loops. These are alternative experiments, not permission to redesign
the public facade API or introduce unsafe code into it. Every candidate below
still awaits measurement; no current workload share or end-to-end speedup is
established. Resolve the linked security prerequisites first.
Candidate redesigns with unmeasured benefit do not outrank confirmed contract
defects. B1's observable-reader repair precedes trusting affected baselines;
B4 then makes accepted performance reproducible and enforceable rather than a
one-off measurement.

<a id="p1"></a>
### P1 — Accelerate Cache-Control fallback token validation

**Area:** Cache-Control fallback validation · **Priority:** Medium · **Effort:** Small

**Evidence:** Reasoned · **Expected impact:** fewer classification instructions per nonnumeric/overflow token, still linear in token bytes; size and overall workload share unknown · **Risk:** short-input regressions and changed error precedence; no new API or unsafe · **Scope:** 1 fallback validator, exhaustive

Each directive routed through `validate_token_value` pays this fallback after
an unsuccessful seconds parse, or directly for a nonnumeric value.
`crates/http_headers/src/headers/cache_control.rs:1025` uses
`for byte in bytes.iter().copied()`; the proposed replacement changes that
scan, not the need to validate the entire token. The end-to-end ceiling is
limited to the unknown share spent in these directive scans.

Evaluate the existing word-based token validator as a replacement for the
scalar fallback after unsuccessful decimal parsing and for nonnumeric
directive values. Keep complete validation without introducing another
numeric pass or changing the separate quoted-value grammar.

- `crates/http_headers/src/headers/cache_control.rs:1016` — `validate_token_value`

**Done when:** complete per-case instruction measurements support retaining
or rejecting the replacement, and
oracles cover `u64::MAX`, overflow, long leading zeroes, nondigit tokens,
invalid suffixes, and the separate quoted-value path.

**Decision:** use B3's `http_headers_policy_shapes` overflow/token/quoted
strata and B1 directive reads with B4's per-case gate. Retain a measured win
or close the candidate as not worthwhile with the comparison recorded.

---

<a id="p6"></a>
### P6 — Reduce HTTP-date digit-validation overhead

**Area:** conditional-header shared date parser · **Priority:** Medium · **Effort:** Medium

**Evidence:** Reasoned · **Expected impact:** lower fixed digit-validation cost per date decode; no measured magnitude or known overall date share · **Risk:** accepting bad digits, calendar changes, or precision drift; no new API or unsafe · **Scope:** 2 shared digit helpers, exhaustive

The fixed IMF parser calls `two_digits` six times; relaxed normalization
uses the short-decimal helper for day, year and three clock fields.
`crates/http_headers/src/headers/conditional/shared.rs:1107` checks
`value.as_bytes().iter().all(u8::is_ascii_digit)` before `value.parse()` at
line 1110. This is a bounded per-date cost, not an unbounded parsing
bottleneck; its unknown request share caps the possible overall gain.

Evaluate packed two-digit validation and the separate digit-validation pass
before accumulation in `parse_short_decimal`. Preserve checked conversions,
calendar validation, and the allocation-free fixed-width normalization path.

- `crates/http_headers/src/headers/conditional/shared.rs:1106` — short decimal validation and accumulation
- `crates/http_headers/src/headers/conditional/shared.rs:1230` — two-digit validation

**Done when:** instruction measurements across every date header and If-Range
support retaining or rejecting each candidate; exhaustive byte-pair checks
and an independent date parser preserve normalization, leap/calendar validity,
supported years, and malformed forms.

**Decision:** B3's conditional/range date strata and B1 cached date-read
controls decide acceptance or closure as not worthwhile, under B4. Preserve
If-Range metadata's agreement with wire precision when comparing date paths.

---

<a id="p9"></a>
### P9 — Reuse accelerated validation and searches for entity tags

**Area:** ETag construction and conditional tag lists · **Priority:** Medium · **Effort:** Medium

**Evidence:** Reasoned · **Expected impact:** fewer instructions in linear opaque-byte and delimiter scans per constructed/read tag; magnitude and overall share unknown · **Risk:** conflating opaque tag bytes with list grammar; no new API or unsafe · **Scope:** 4 named constructor/validator/iterator routines, sampled call paths

`crates/http_headers/src/headers/etag.rs:372` performs
`opaque.iter().copied().all(valid_opaque_byte)` once per constructor call.
Conditional readers traverse each requested tag list anew. Long tags offer
more scan work to amortize a wide helper, while early invalid and short tags
may lose; the unknown construction/read frequency limits end-to-end impact.

Evaluate using the existing word validator in ETag constructors instead of
the bytewise predicate. For If-Match and If-None-Match, investigate the
existing delimiter-search helpers for closing quotes and long-tag scans;
preserve the distinct roles of commas, semicolons, backslashes, and whitespace
inside opaque tags.

- `crates/http_headers/src/headers/etag.rs:371` — `construct`
- `crates/http_headers/src/headers/etag.rs:444` — existing `opaque_is_valid`
- `crates/http_headers/src/headers/conditional/shared.rs:776` — `TagIter`
- `crates/http_headers/src/headers/conditional/shared.rs:952` — `validate_tag_line_with`

**Done when:** constructor, decode, and semantic-reader measurements establish
each retained gain, with short/long/obs-text tags, weak markers, wildcard
mixing, repeated lines, and exact error precedence unchanged.

**See also:** B1, B3.

**Decision:** compare measured-body constructors and decode-plus-tag-reader
cases in B1/B3, not constructors hidden in fixture setup. Retain only B4
nonregressing wins or close as not worthwhile with the measured rejection. SEC3's borrowed
conditional-tag forwarding marker fix remains a prerequisite for forwarding
comparisons.

---

<a id="p10"></a>
### P10 — Evaluate Content-Type common-literal dispatch

**Area:** Content-Type parse pipeline · **Priority:** Medium · **Effort:** Medium

**Evidence:** Speculative · **Expected impact:** possibly fewer literal-dispatch instructions per Content-Type decode; compiler lowering, magnitude and overall share unknown · **Risk:** redundant manual dispatch, near-miss regressions and code-size growth; no new API or unsafe · **Scope:** 1 recognizer with 8 literal spellings, exhaustive

Every metadata parse probes `common_metadata` before the general parser.
`crates/http_headers/src/headers/content_type.rs:696` first compares
`bytes == b"application/json; charset=utf-8"` and line 699 starts
`match bytes`. The compiler may already group comparisons by length; source
comparison count is not emitted instruction count. The ceiling is the
unknown share spent recognizing these values, not all Content-Type work.

Investigate length dispatch for common literal recognition, with original-code
hit and early/late near-miss controls before changing the recognizer. A faster
hit is not useful if equal-length unknown values pay for additional probes.

- `crates/http_headers/src/headers/content_type.rs:695` — `common_metadata`

**Done when:** representative hit, miss, casing, and fallback measurements
support retaining or rejecting the candidate under the complete instruction
gate. Preserve semantic metadata, exact wire bytes, error positions, and
allocation behavior.

**See also:** B3.

**Decision:** B3's hit/early-miss/late-miss strata and B5 ordinary
release versus fat-LTO disassembly decide whether explicit dispatch adds
anything. Accept a B4-gated win or close as not worthwhile. Keep wire-value
identity independent of cache representation; coordinate P26's layout study.

---

<a id="p11"></a>
### P11 — Resume Content-Type lookup after the cached parameter prefix

**Area:** parameter semantic accessors · **Priority:** Medium · **Effort:** Medium

**Evidence:** Structural · **Expected impact:** avoid rescanning up to the cached prefix on each later/missing lookup; saved bytes and overall lookup share unknown · **Risk:** noncontiguous/large offsets and quoted/duplicate semantics; no new API or unsafe · **Scope:** 1 lookup function and its 2-entry prefix cache, exhaustive

`crates/http_headers/src/headers/content_type.rs:957` starts
`ParameterScanner::new(bytes, ...)` at `head.parameter_start`, then calls
`.skip(usize::from(head.inline_count))`. Thus a caller making repeated
uncached lookups pays again for the cached wire prefix. The saving is bounded
by that prefix per lookup, not the entire remaining parameter list, and
application lookup frequency is unknown.

Lookup beyond the inline cache restarts a scanner at `parameter_start` and
skips already cached entries. Investigate resuming at the last cached value
end instead. The existing cached `charset` lookup is not evidence for a
third-parameter hit or a missing-name lookup.

- `crates/http_headers/src/headers/content_type.rs:946` — `find_parameter`

**Done when:** original-code baselines cover later/missing lookups in both
ownership modes; retained gains preserve checked offsets, cache-contiguity
invariants, first-match duplicate semantics, quoted values, empty slots, and
large-offset fallbacks.

**See also:** B1, B3.

**Decision:** use B1/B3's first/third/last/missing lookups with quoted long
prefixes and B5 layout controls. Record either a retained B4 nonregressing
win or closure as not worthwhile; coordinate P26 so cursor storage does not
silently enlarge every common result.

**Guideline connection:** [C-INTERMEDIATE](https://rust-lang.github.io/api-guidelines/flexibility.html#c-intermediate)
recommends useful intermediate results that avoid duplicate work. Preserve
the existing retained metadata and public semantic APIs; this experiment is
about the uncached lookup's resume point, not reparsing every getter or
requiring new public intermediate types.

---

<a id="p12"></a>
### P12 — Reuse delimiter searches in negotiation validation

**Area:** Accept and Accept-Language scanners · **Priority:** Medium · **Effort:** Medium

**Evidence:** Reasoned · **Expected impact:** fewer delimiter-search instructions per media/language member; remains linear, with magnitude and overall share unknown · **Risk:** extra short-token dispatch and changed malformed-input precedence; no new API or unsafe · **Scope:** 2 member validators, exhaustive

`crates/http_headers/src/headers/negotiation/accept.rs:225` uses
`bytes.splitn(2, |byte| *byte == b'/')`; language validation similarly discovers
subtag boundaries per member. A header decode reaches these routines for
members not settled by earlier recognition paths, so the traffic share of
the fallback, not the mere presence of Accept, bounds the benefit.

Evaluate the existing safe `find_either` helper for Accept's
first-slash split and language-subtag delimiter discovery. Preserve the cheap
overlong-subtag rejection before character validation rather than assuming
a single pass is always better.

- `crates/http_headers/src/headers/negotiation/accept.rs:225` — media-range split and repeated-slash error precedence
- `crates/http_headers/src/headers/negotiation/accept_language.rs:141` — language-range subtag validation

**Done when:** each retained change passes canonical, raw, repeated, long,
wildcard, and malformed cases without losing the typed semantic output;
vector-boundary slash positions and language length boundaries have exact
result/error oracles and fresh instruction baselines.

**See also:** B1.

**Decision:** compare B3 negotiation shapes with B1 typed read/selection
controls after considering P25's broader traversal design. B4 measurements
must support acceptance; an unchanged or slower result closes this candidate
as not worthwhile without a forced implementation.

---

<a id="p13"></a>
### P13 — Reduce Host literal and international-fallback overhead

**Area:** Host parsing · **Priority:** Medium · **Effort:** Medium

**Evidence:** Reasoned · **Expected impact:** reduce literal delimiter scans or common-path code footprint per Host decode; magnitude and overall literal/IDNA share unknown · **Risk:** short-host regressions, normalization/framing drift and unhelpful outlining; no new API or unsafe · **Scope:** 2 fallback routines, exhaustive

`crates/http_headers/src/headers/negotiation/host.rs:853` separately checks
`bytes.contains(&b'@')`; line 858 searches
`.position(|byte| *byte == b']')`. These execute on bracketed literal
decodes, while IDNA belongs only to relaxed international fallback.
Their frequencies are not established, and a cold-boundary gain requires
emitted-code evidence rather than assuming the compiler inlined either path.

Examine a cold boundary around the international-name fallback and reuse safe
delimiter searches for `@` and closing brackets. Measure both malformed
framing and valid long literals rather than assuming a scanner improves
short hostnames.

- `crates/http_headers/src/headers/negotiation/host.rs:747` — IDNA fallback
- `crates/http_headers/src/headers/negotiation/host.rs:759` — normalized-name delimiter rejection
- `crates/http_headers/src/headers/negotiation/host.rs:853` — literal framing and closing-bracket search

**Done when:** owned and borrowed Unicode-IDNA controls are baselined alongside
strict/relaxed ASCII, IPv6/IPvFuture, delimiter boundaries, and malformed
ports; only per-case nonregressing changes remain, with original error
precedence and typed components intact.

**See also:** P24, B3.

**Decision:** B3 literal/IDNA shapes, the authority semantic target, and B5
cross-profile attribution decide acceptance or closure as not worthwhile
under B4. Preserve sensitive Host diagnostic redaction and SEC3 sensitivity
when comparing retained metadata or forwarding.

---

<a id="p14"></a>
### P14 — Investigate Authorization validation data flow

**Area:** Basic and Bearer validation · **Priority:** Medium · **Effort:** Medium

**Evidence:** Speculative · **Expected impact:** possibly lower validation instructions per credential block or token; compiler behavior, magnitude and overall authorization share unknown · **Risk:** padding/colon/zeroization mistakes and code-size regressions; no new API or unsafe · **Scope:** 2 scheme-validation paths, exhaustive; codegen effects unmeasured

`crates/http_headers/src/headers/authorization.rs:863` visits
`body.chunks_exact(4)` and line 873 accumulates `colons |= colon_marks(group)`.
This is once per Basic decode, not a fresh decoded allocation. Bearer's
local scalar/SIMD crossover and the companion's crossover need matched
length/backend controls. The optimizer may already remove the suspected
boundary overhead; unknown decode/extraction shares cap any overall gain.

Inspect Basic's validator boundary and decoded-colon data flow, and Bearer's
short scalar tail versus long-token dispatch. Look for redundant work in
generated code rather than assuming extra inlining or outlining helps.

- `crates/http_headers/src/headers/authorization.rs:857` — `validate_basic`
- `crates/http_headers/benches/http_headers_auth_cors_shapes.rs:18` — source/ownership and adverse-shape measurement target

**Done when:** each concrete change has attributed instruction gains across
short/long tokens, early/late invalid bytes, padding, and missing-colon
inputs; extraction semantics, sensitive storage, and zeroization are
preserved. Unsupported hypotheses do not become production changes.

**See also:** B3.

**Decision:** use B3 auth shapes and separately bounded cold/warm extraction,
with B5 disassembly and B4 per-case measurements. Retain a justified change
or close as not worthwhile; a boundary annotation without an attributed win
does not complete the item.

---

<a id="p15"></a>
### P15 — Reduce repeated CORS list traversal and projection

**Area:** Allow-Headers, Expose-Headers, Request-Headers, and Allow-Methods · **Priority:** Medium · **Effort:** Medium

**Evidence:** Structural · **Expected impact:** reduce repeated linear line/member work per decode-plus-reader lifecycle; dispatch savings and overall CORS share unknown · **Risk:** code duplication, larger results and changed wildcard/error-index semantics; no new API or unsafe · **Scope:** 4 list families using the shared validator, exhaustive

Each decode validates physical lines, and each requested semantic traversal
projects their members again. `crates/http_headers/src/headers/cors/shared.rs:926`
takes `values: &mut dyn Iterator<Item = FieldValueRef<'_>>`; line 935
iterates `(1_usize..).zip(values)`. The traversal exists structurally, but
whether indirect calls survive depends on the profile. Repeated readers and
line counts determine the possible saving; their application share is unknown.

Investigate repeated list traversal, erased-validator call boundaries, and
member projection after validation. Preserve existing wildcard, method/name
comparison, custom-source preflight, and error-index semantics; use existing
typed item machinery rather than an alternate parser.

- `crates/http_headers/src/headers/cors/shared.rs:910` — shared list validator
- `crates/http_headers/src/headers/cors/shared.rs:1043` — per-member validation

**Done when:** decode and semantic-reader baselines independently demonstrate
the retained gains for singleton/repeated, wildcard, mixed-case, extension,
and early/late error cases, with no regression in other users of shared code.

**See also:** B1.

**Decision:** compare B1 decode-only/1/2/8 reads, B2 representation/line-count
strata and B5 erased-versus-specialized codegen. Accept only B4-supported
wins or close as not worthwhile with a measured rejection. Like P21, retain the exact validated
`FieldLines`; never reacquire a changing source to reuse its validation proof.

---

<a id="p18"></a>
### P18 — Tighten CORS scalar token decoding

**Area:** Allow-Credentials and Request-Method · **Priority:** Low · **Effort:** Medium

**Evidence:** Speculative · **Expected impact:** possibly fewer fixed framing/comparison instructions per scalar decode; magnitude and overall share unknown · **Risk:** trading cheap failures for wider work or dropping sensitivity; no new API or unsafe · **Scope:** 2 scalar decoders, exhaustive

`crates/http_headers/src/headers/cors/access_control_allow_credentials.rs:217`
calls `is_credentials_true(lines.exactly_one()?.as_bytes())`;
`crates/http_headers/src/headers/cors/access_control_request_method.rs:350` calls
`request_method_of(value.as_bytes())?`. Each is one scalar decode after
source checks. Their already-small operation and unknown request frequency
bound the opportunity; emitted comparison lowering may already be optimal.

Inspect framed-whitespace/range data flow around the `true` literal and
known-method literal comparison lowering. These already-cheap paths need
attribution-backed candidates, not unconditional wider comparisons that
penalize early failures.

- `crates/http_headers/src/headers/cors/access_control_allow_credentials.rs:209` — singleton borrowed decode
- `crates/http_headers/src/headers/cors/access_control_request_method.rs:341` — method borrowed decode

**Done when:** any retained change improves instructions while preserving
owned/borrowed, padded, duplicate, case-sensitive, unknown-method, and
same-length near-miss behavior; stop without a code change if no such
candidate remains.

**Decision:** resolve SEC3's registered-method sensitivity loss before
comparing compact states or forwarding. B3 scalar near-miss/framing strata
and B5 generated code decide acceptance under B4 or closure as not worthwhile.

---

<a id="p19"></a>
### P19 — Investigate Content-Length singleton and decimal decoding

**Area:** numeric header decoding · **Priority:** Medium · **Effort:** Medium

**Evidence:** Structural · **Expected impact:** avoid a separate singleton comma scan, with possible decimal-loop instruction savings; magnitude and overall Content-Length share unknown · **Risk:** overflow, full-consumption and duplicate/error-precedence changes; no new API or unsafe · **Scope:** 1 singleton decision and 2 decimal/list routines, exhaustive

`crates/http_headers/src/headers/content_length.rs:125` probes
`!first.as_bytes().contains(&b',')` before `parse_decimal_ows` at line 126.
The decimal loop at line 179 uses
`value.checked_mul(10)?.checked_add(...)` for every digit. Fuse discovery
before considering grouped accumulation; both are conditional experiments,
not permission to omit checked arithmetic or the custom-source preflight.
Unknown source and numeric-length distributions limit end-to-end benefit.

Inspect short-decimal accumulation, singleton scanning, and call boundaries
for unnecessary work before falling back to duplicate/list handling. Preserve
checked arithmetic and validation of every supplied value.
In particular, investigate recognizing a singleton number and its first comma
in one pass without adding a speculative numeric parse before the comma-list
fallback. Measure malformed and comma-joined inputs as well as bare numbers.

- `crates/http_headers/src/headers/content_length.rs:158` — `parse_content_length_line`
- `crates/http_headers/benches/http_headers_auth_cors_shapes.rs:18` — existing Content-Length shape coverage

**Done when:** retained gains satisfy canonical, 19/20-digit, leading-zero,
equal/conflicting duplicate, overflow, and malformed-list instruction gates
with identical values and exact errors.

**Decision:** B3's Content-Length shape series, B2's raw/HTTP source matrix
and B5 attribution decide whether either transformation earns a B4-gated
win. Close as not worthwhile if it does not; count long leading zeroes as
validation work even though at most 20 significant digits fit in `u64`.

---

<a id="p21"></a>
### P21 — Reduce shared source and result construction overhead

**Area:** `Field`, `FieldLines`, and owned/view forwarding · **Priority:** Medium · **Effort:** Medium

**Evidence:** Structural · **Expected impact:** reduce repeated preflight passes, list growth or raw/result movement per typed operation; emitted copies, magnitude and overall share unknown · **Risk:** proof reuse across changing sources, larger layouts and lost markers/atomicity; no new API or unsafe · **Scope:** 4 source representations and 2 generic forwarding entry points, exhaustive; downstream consumers and serde seam sampled

For example, `crates/http_headers/src/source/field_lines.rs:356` calls
`self.validate_custom_source()?` again when acquiring repeated owned values.
Consider a private validated-ownership state tied to the exact immutable
snapshot, rather than deleting checks or asking `FieldSource::lines()` again.
At line 390, the HTTP `len()` arm uses `values.iter().count()`: do not add a
pre-count traversal merely to reserve a collector.

Compare singleton promotion and closed-enum dispatch across an entire
decode/read/forward lifecycle, not only a wrapper. Also price the serde name
boundary: `crates/http_headers/src/serde_impls.rs:482` uses
`String::deserialize(deserializer)?` even for well-known enum names; a
borrowed string visitor is an alternative to that temporary, not a request
for a new borrowed public deserialization API. Grammar checks and custom
budgets still apply. The shared route has broad reach, but actual caller
frequency and its fraction of request cost remain unknown.

After the typed APIs settle, inspect source lookup, representation dispatch,
temporary result/list copies, singleton owned conversion, and budget-policy
boundaries separately from grammar parsing. Shared changes must be followed
by fresh inspection of every affected header, not just a canonical count.

- `crates/http_headers/src/field.rs:403` — generic single-value borrowed forwarding
- `crates/http_headers/src/field.rs:420` — owned forwarding
- `crates/http_headers/src/source/field_lines.rs:465` — custom-source bounds
- `crates/http_headers/src/source/field_lines.rs:491` — source validation

**Done when:** concrete source/result-copy candidates are measured across
all representations and affected headers; only per-case instruction wins
remain, with custom-name behavior, source/list limits, sharing, and defensive
checks preserved.

**See also:** B2, B5 (deciding measurements). Preserve the independent
admission-policy oracles in `tests/source_limits.rs` and `tests/serde.rs`.

**Decision:** B2's complete source/ownership/serde matrix and B5's ordinary
release layouts/copies decide acceptance or closure as not worthwhile under
B4; B6 bounds the potential request-level gain. SEC3 precedes forwarding
experiments, including registered methods and borrowed conditional tags.
Preserve direct/source and borrowed/owned admission parity in equivalent-work
comparisons.

**Guideline connection:** [C-INTERMEDIATE](https://rust-lang.github.io/api-guidelines/flexibility.html#c-intermediate)
recommends reuse of useful intermediate results. Any private validation proof
must remain attached to the original immutable `FieldLines`; a later
`FieldSource::lines()` call is not the same proof-bearing input. Keep that
condition in correctness checks for any measured optimization.

---

<a id="p24"></a>
### P24 — Reduce retained URI and authority metadata costs

**Area:** Location, Host, and Access-Control-Allow-Origin decoding · **Priority:** Medium · **Effort:** Large

**Evidence:** Structural · **Expected impact:** reduce retained result footprint or repeated boundary discovery per decode; layout, CPU magnitude and overall workload share unknown · **Risk:** larger fallback variants, normalization lifetimes, semantic/redaction drift; no new public facade API, unsafe only in the existing companion if required · **Scope:** 3 semantic header families, exhaustive; layout/codegen alternatives sampled

Retained semantic metadata removes caller-side parsing but increases
decode-only work. Investigate compact component boundaries, result movement,
and classification costs without discarding parsed addresses, normalized
backing, exact port states, or original wire storage. Do not recover cheap
decoding by moving URI parsing, IDNA conversion, or allocation into getters.

**Provenance constraint:** the comparison numerals in the next paragraph are
retained legacy backlog notes, not measurements established by this audit.
`crates/http_headers/docs/PERF.md:3` explicitly identifies its table as an
imported 0.1.0 snapshot. No current raw artifact/compiler/hardware provenance
for the newer numbers has been verified here; do not use them as a gate or
as evidence of the size of this candidate.

Canonical Callgrind instruction counts against the existing `PERF.md`
baseline are Location borrowed 595 → 786 and owned 822 → 1029; Host borrowed
527 → 683 and owned 548 → 708; Allow-Origin borrowed 555 → 583 and owned
670 → 694. These are decode-only costs, not complete consumer workloads.
The semantic benchmarks must remain a separate acceptance axis; aggregate
improvements must not conceal a slower individual case.

The structural trace is independent of those numbers:
`crates/http_headers/src/headers/location.rs:367` sets
`metadata: Metadata::from_simple(text)` after subset validation;
`crates/http_headers/src/headers/location/metadata.rs:51` begins further
`find_either` boundary searches.
That metadata stores `Range<usize>` and optional full-width boundaries
at lines 14–18. Compare emitting boundaries during the original safe scan
and compact checked offsets with a full-width fallback. Include Host/Origin
classification and retained normalization backing in the same layout study,
without replacing useful retained semantics with a lazy parser. Per-decode
work and result movement may shrink, but the common/general/relaxed mix and
the share of callers doing semantic reads are unknown.

- `crates/http_headers/benches/http_headers_per_header.rs:355` — canonical Host decode workloads
- `crates/http_headers/benches/http_headers_per_header.rs:401` — canonical Location decode workloads
- `crates/http_headers/benches/http_headers_per_header.rs:272` — canonical Allow-Origin decode workloads
- `crates/http_headers/benches/http_headers_location_semantics.rs` — decode, repeated component reads, normalization, and forwarding
- `crates/http_headers/benches/http_headers_authority_semantics.rs` — retained authority semantics versus caller parsing

**Done when:** fresh measurements with the baseline compiler/target flags
reduce retained-metadata overhead across both ownership modes without
regressing semantic workloads, grammar/error behavior, or allocations;
value layouts and generated-code effects explain the retained changes.
Keep `PERF.md` updates with the maintainer.

**See also:** SEC3 (retained-marker prerequisite); P13, B4, B5.

**Decision:** use B1's Location/authority decode, retained and 1/2/8-read
targets, B2 ownership/forwarding, and B5 layouts; B6 prices workload shares.
The Location caller baseline already retains its parsed URI, and both
authority comparison arms use the current decoder: neither is a historical
decoder baseline. Accept a measured nonregressing design or close as not
worthwhile. Preserve sensitive Host diagnostic redaction and SEC3 marker
propagation throughout.

---

<a id="p25"></a>
### P25 — Compare streaming negotiation with bounded retained member metadata

**Area:** Accept, Accept-Encoding, Accept-Language, Allow and Vary semantic lifecycles · **Priority:** Medium · **Effort:** Large

**Evidence:** Structural · **Expected impact:** avoid repeated wire/member projection work proportional to reader count times wire length; no measured magnitude or known overall share · **Risk:** eager decode work, larger results, cache overflow complexity and semantic drift; no new public API or unsafe
**Scope:** 5 typed list families, exhaustive; proposed private representations are experimental

Keep streaming as the control, but compare a bounded inline member index
collected during validation with today's wire-only retained lists and with
caller-retained yielded entries. Each new Accept iterator traverses raw
members and reconstructs media/quality/parameter boundaries. Encoding and
language entries similarly project token/range and quality; Allow/Vary
iterate validated tokens without allocating, but still rediscover members.
This is a representation/validation-to-reader boundary question, not P12's
smaller choice of delimiter-search primitive.

- `crates/http_headers/src/headers/negotiation/accept.rs:111` — owned `.flat_map(...)` followed by `.map(AcceptEntry::from_validated)`; borrowed traversal starts at line 139 with `.validated_comma_items()` at line 141
- `crates/http_headers/src/headers/negotiation/accept_entry.rs:59` — `bytes.iter().position(|byte| *byte == b';')` begins per-entry boundary reconstruction
- `crates/http_headers/src/headers/negotiation/quality.rs:120` — `from_validated` derives a compact or exact fractional view
- `crates/http_headers/src/headers/negotiation/allow.rs:78` — `self.items().map(MethodView::from_validated)`
- `crates/http_headers/src/headers/negotiation/vary.rs:129` — `self.items().map(VaryEntryView::from_validated)`
- `crates/http_headers/src/headers/negotiation/accept_scan.rs:367` — the acceptance-only DFA visits `bytes.chunks_exact(8)`; an unsupported shape is subsequently handled by the general parser

For `r` fresh traversals of `n` wire bytes, the repeated projection component
is O(r × n). A bounded retained index might exchange this for one
validation/indexing pass and cheap member projections, but decode-only and
one-read consumers may lose. Read counts and input frequencies are unknown,
so no overall speedup or minimum useful cache size can be promised. Include
early-decline versus full acceptance-recognizer fallback as a smaller
pipeline alternative; it may add an unhelpful common-path branch.

Do not build a policy engine, sort, deduplicate, round qualities, or use an
unbounded per-member allocation. Preserve exact relaxed fractions, duplicate
and wildcard behavior, quoted parameter/extension boundaries, physical lines,
error kinds/indices and budgets. An overflow path must retain today's
streaming semantics. Validated views must retain the original immutable
`FieldLines`, never reacquire a mutable source.

**Done when:** B1/B3 extend `http_headers_negotiation_semantics` and
`http_headers_typed_tokens` with decode-only, 1/2/8 fresh traversals,
caller-retained entries, owned readers, first/last/missing matches,
quoted/relaxed/repeated and index-overflow controls. B5 records result layout
and downstream codegen; B6 establishes the read-count/workload crossover.
Accept a design only with B4's per-case instruction/allocation nonregression,
including decode-only, or close the experiment as not worthwhile with its
measurements. Preserve SEC3 behavior in inspect-and-forward comparisons.

**See also:** SEC3 (retained-marker prerequisite); P12, P21, B1, B3, B5, B6.

---

<a id="p26"></a>
### P26 — Evaluate inline general Content-Type metadata

**Area:** nonliteral Content-Type decode, construction and clone · **Priority:** Medium · **Effort:** Medium

**Evidence:** Structural · **Expected impact:** potentially remove one explicit metadata Box allocation per successful general parse/construction or deep metadata clone; time/footprint trade and overall nonliteral share unknown · **Risk:** larger common values/results and changed cache identity; no new API or unsafe
**Scope:** 1 metadata enum and 3 production Box-construction sites, exhaustive

General Content-Type metadata is a fixed-size head with two inline parameter
ranges, yet it sits behind a box. Compare a compact inline general form with
the existing boxed form and literal variants. Do not assume saving the
allocation outweighs moving a larger owned/view result through every common
decode. This also differs from P10's recognizer and P11's lookup cursor.

- `crates/http_headers/src/headers/content_type.rs:158` — `Parsed(Box<ContentTypeHead>)`
- `crates/http_headers/src/headers/content_type.rs:283` — construction uses `Box::new(ContentTypeHead { ... })`
- `crates/http_headers/src/headers/content_type.rs:677` — strict general parsing ends with `ContentTypeMetadata::Parsed(Box::new(head))`
- `crates/http_headers/src/headers/content_type.rs:692` — relaxed general fallback uses the same allocation

The parser pays this once per successful nonliteral decode, including a
borrowed decode; literal hits avoid it. Derived metadata cloning also clones
the box. These are structural allocation sites, not current allocation
measurements or evidence that nonliteral inputs dominate deployed traffic.
That unknown fraction and subsequent read/clone frequency cap the overall
gain. Keep eager validation and constant-time scalar access; do not shift
parsing or allocation into getters to make decode look cheaper.

**Done when:** use B1/B3's literal controls and nonliteral
0/1/2/3/16/49-parameter, quoted, relaxed and large-offset cases. B2 includes
direct construction and clone lifecycles; B5 compares layouts and emitted
copies under ordinary release and fat LTO.
Record allocations, allocated bytes, instructions and time per ownership
mode. Accept only B4-nonregressing results, with identical wire/semantic
identity and parameter behavior, or close as not worthwhile with the
comparison. Coordinate P11's resume cursor rather than creating competing
metadata layouts.

**Guideline connection:** [M-AVOID-INDIRECTION](https://microsoft.github.io/rust-guidelines/guidelines/performance/#M-AVOID-INDIRECTION)
recommends avoiding needless indirection in hot types. Whether this box is
needless is unassessed until the common/general mix and result-layout
tradeoff are measured; the guideline does not mandate unconditional inlining
or justify a new cache identity.

**See also:** P10, P11, B1, B2, B3, B5.

---

<a id="p27"></a>
### P27 — Transfer completed Allow and Vary construction buffers

**Area:** typed response-list construction into owned field storage · **Priority:** Low · **Effort:** Small

**Evidence:** Structural · **Expected impact:** avoid the final O(n) wire copy and potentially its additional backing allocation above 64 bytes; frequency and overall share unknown · **Risk:** short-result layout regressions or added validation work; no new API or unsafe
**Scope:** 2 constructor bodies, exhaustive; Vary's name constructor delegates here

`AllowOwned::from_methods` and `VaryOwned::from_entries` append into a String
and immediately construct final storage from its borrowed bytes. Long output
is copied into another backing allocation before that String is dropped.
Compare safe ownership transfer of the completed long buffer while retaining
the short inline result behavior; account for any new validation pass in the
comparison rather than presuming conversion is free.

- `crates/http_headers/src/headers/negotiation/allow.rs:87` — `let mut wire = String::new()`; line 95 calls `FieldValue::from_validated_bytes(wire.as_bytes(), false)`
- `crates/http_headers/src/headers/negotiation/vary.rs:112` — the corresponding String builder; line 120 repeats the borrowed final conversion
- `crates/http_headers/src/field_value.rs:124` — the long `Repr::new` arm uses `Bytes::copy_from_slice(bytes)`
- `crates/http_headers/src/field_value.rs:636` — consuming String conversion already uses `Self::from_shared(Bytes::from(value))`

This is once per typed list construction, not every request decode. The
constructed length grows with caller-supplied tokens; token lengths, list
sizes and construction frequency are unknown. Preserve empty lists,
spelling, order, duplicates and mixed wildcard/name semantics. Do not replace
the borrowed decoding API or introduce another grammar implementation.

**Done when:** B2 measures construction inside the operation, including
0/1/many members, exact/unknown iterator hints, 63/64/65-byte results and
construct-to-sink lifecycles. Compare instructions, allocations and bytes
with B1 semantic/wire controls. Retain a B4-nonregressing transfer or close as
not worthwhile with the comparison, without changing validation or
sensitivity contracts.

**See also:** B1, B2, B4.

---

## Features

<a id="f1"></a>
### F1 — Classify semantically sensitive typed headers with `data_privacy_core`

**Area:** `http_headers` typed header values · **Priority:** Medium · **Effort:** Medium

Integrate at the typed-header boundary, where the header name supplies stable
semantics, rather than on `FieldValue`. The raw value types can represent both
sensitive and nonsensitive data, while `Classified::data_class` must always
return a class. Handwritten implementations should depend on
`data_privacy_core`, not the full macro and redaction-engine crate. The
existing `is_sensitive` marker remains the transport and secret-safe `Debug`
signal used by `http::HeaderValue` interoperability.

- `crates/http_headers/src/headers/authorization.rs` — Authorization descriptor and typed owned/view values
- `crates/http_headers/src/headers/set_cookie.rs` — Set-Cookie descriptor and typed owned/view values
- `crates/http_headers/src/headers/location.rs` — Location descriptor and typed owned/view values
- `crates/http_headers/src/field_value.rs` — representation-level Boolean sensitivity marker
- `crates/data_privacy_core/src/classified.rs` — `Classified` requires an unconditional `DataClass`

**Done when:** an approved taxonomy defines classes for Authorization,
Set-Cookie, and Location; their borrowed and owned typed values implement
`Classified` through `data_privacy_core`; API tests cover every implementation;
and encoding still preserves the existing `is_sensitive` behavior.

---

<a id="f2"></a>
### F2 — Add byte-oriented redaction before redacting typed headers

**Area:** `data_privacy_core` redaction API and `http_headers` typed values · **Priority:** Medium · **Effort:** Large

Do not expose policy-driven redaction for typed headers through the current
string-only API. HTTP field values can contain valid non-UTF-8 `obs-text`, and
Set-Cookie exposes its values as bytes. Converting such values lossily before
hashing or redaction would change their identity; rejecting them would leave a
valid sensitive-value path unsupported.

- `crates/data_privacy_core/src/redactor.rs` — `Redactor::redact` accepts only `&str` and `fmt::Write`
- `crates/http_headers/src/headers/set_cookie.rs` — owned cookies iterate as byte-valued `FieldValue`s
- `crates/http_headers/src/headers/set_cookie.rs` — borrowed cookies iterate as byte-valued `FieldValueRef`s
- `crates/http_headers/src/headers/set_cookie.rs` — valid cookie field values are checked as bytes

**Done when:** `data_privacy_core` provides an explicit byte-oriented
redaction contract, typed header redaction uses it without UTF-8 coercion, and
tests cover arbitrary valid field bytes including `obs-text` for pass-through,
replacement, and hashing policies.

---

<a id="f3"></a>
### F3 — Convert `templated_uri::Uri` into `LocationOwned`

**Area:** `templated_uri` integration with `http_headers::headers::LocationOwned` · **Priority:** Low · **Effort:** Small

Provide an opt-in, one-way construction adapter for applications that build
classified redirect targets with `templated_uri`. Keep the integration in
`templated_uri` behind an optional dependency on `http_headers`; the
lower-level header crate should not acquire a dependency on the templating and
privacy stack. The adapter must materialize the URI for transmission and
produce a normal sensitive `LocationOwned`, without claiming to preserve the
template's per-component classifications after conversion.

This conversion is deliberately not a replacement for `Location` validation.
`Location` accepts the complete RFC 3986 URI-reference grammar, including
relative, empty, and fragment-only references, while `templated_uri` models
HTTP request URIs and does not support fragments.

- `crates/http_headers/src/headers/location.rs` — `Location` accepts RFC 3986 URI-references
- `crates/http_headers/src/headers/location.rs` — string conversion validates and marks the value sensitive
- `crates/templated_uri/src/lib.rs` — URI templates intentionally exclude fragments
- `crates/templated_uri/src/uri.rs` — `Uri` already materializes fallibly as `http::Uri`

**Done when:** an optional `http_headers` integration feature in
`templated_uri` implements `TryFrom<templated_uri::Uri> for LocationOwned`
with an error that preserves materialization and Location-validation failures;
converted values remain sensitive; and tests cover absolute and relative
path/query targets plus rejection or exclusion of unsupported fragment forms.

---

## Documentation

<a id="d1"></a>
### D1 — Document typed headers in the workspace HTTP APIs

**Area:** `http_extensions`, `fetch`, and `rest_over_grpc` examples · **Priority:** Medium · **Effort:** Small

Add concise, compiling examples showing how the workspace's HTTP-facing APIs
use `http_headers` without duplicating its parsing or encoding methods.
`http_extensions` and `fetch` should demonstrate typed fields on their built
`http::Request` and `http::Response` values and through builder
`headers_mut()` access. `rest_over_grpc` should use the explicit request and
response header-map accessors so the direction of each operation remains
clear.

- `crates/http_extensions/src/http_request_builder.rs` — mutable request-builder headers
- `crates/http_extensions/src/http_response_builder.rs` — mutable response-builder headers
- `crates/fetch/src/lib.rs` — public `http` request, response, and header-map re-exports
- `crates/rest_over_grpc/src/context.rs` — explicit request-header access
- `crates/rest_over_grpc/src/context.rs` — explicit response-header access
- `crates/rest_over_grpc/src/http_response.rs` — mutable neutral-response headers

**Done when:** checked examples decode Authorization or User-Agent from
requests and encode representative Location, ETag, and repeated Set-Cookie
response fields in each applicable crate, with feature requirements documented
and malformed input handled as `DecodeError`.

---

## Benchmarks

<a id="b1"></a>
### B1 — Cover semantic reads separately from decode-and-drop

**Area:** per-header metabench coverage · **Priority:** High · **Effort:** Medium

**Purpose:** maintain and improve. Several per-header rows only decode and
drop, so they cannot price repeated semantic reads.
Measure both `Field::view` and `Field::owned`, then separately consume semantic
outputs: weighted-list items, Content-Type parameters/lookups, Referrer
preference, CORS counts/wildcards, conditional tags, Range specs and WebSocket
extensions including parameters. Include cheap cached scalar/date getters as
controls. Use 1/2/8 reads and short versus multi-member inputs; do not infer
traffic weights from fixtures.

Measure ETag construction inside the benchmark body; constructors used only
to prepare fixtures do not measure that operation. Cover short, long, obs-text,
and invalid opaque values to unblock P9 independently of decode benchmarks.

Extend the typed-token, negotiation, URI, and authority semantic targets with
owned consumption and first/last/missing lookups rather than treating their
current fixtures as a complete matrix. Compare allocation-free streaming
negotiation members with a bounded inline metadata cache, including overflow
fallback, before choosing any additional decode-time indexing.

Use P25's streaming, caller-retained-entry and bounded-index alternatives
with the same selection policy and exact output oracles. The existing
negotiation target already measures fixed three-offer selection, retained
entry scalar reads and select-and-forward; the Location target already
retains the caller's parsed URI. Extend their missing axes rather than
inventing a repeatedly reparsing comparison arm. Include decode-only,
owned/predecoded-owned reads and the complete decode/read/forward lifecycle
for Location, Host, Origin, Accept/Encoding/Language, Allow and Vary.

Make the repeated-read multiplier observable. In the policy target,
`referrer_preferred_8`, `cache_max_age_8` and `auth_access_8` XOR the same pure
answer eight times and black-box only the final zero. This permits hoisting
or cancellation; it does not prove eight calls survive optimization. Guard
the per-read inputs/results and check B5's emitted operation boundary before
using those cases to price getter cost.

- `crates/http_headers/benches/http_headers_operations.rs:44` — generic owned operation ends in `consume(header)`
- `crates/http_headers/benches/http_headers_operations.rs:52` — borrowed operation does the same
- `crates/http_headers/benches/http_headers_per_header.rs:221` — negotiation registrations use these operations
- `crates/http_headers/benches/http_headers_micro.rs:663` — existing Content-Type semantic cases are a starting point, not complete coverage
- `crates/http_headers/benches/http_headers_policy.rs:127` — `result ^= value.preferred().unwrap() as usize`; the cached-age and credential loops repeat this pattern at lines 143 and 164
- `crates/http_headers/benches/http_headers_negotiation_semantics.rs:463` — `forward` and its caller-parsed comparison both start from current owned decoding at line 467
- `crates/http_headers/benches/http_headers_location_semantics.rs:100` — caller-side URI retention in `caller_reads`, parsed once at line 102

**Done when:** cases report wall time, instructions and allocations separately
for decode-only and decode-plus-read, consume actual semantic values, and are
wired through B4 to fail on unexpected allocation increases or any instruction
increase against per-case baselines.

This unblocks P9/P11/P12/P15/P24/P25/P26 independently of decode-only
microbenchmarks. Reader counts and allocations must be recorded, not inferred
from loop syntax or target names; reject a comparison whose two arms perform
different policy or ownership work. SEC3 precedes trusting sensitivity
preservation in forwarding controls.

---

<a id="b2"></a>
### B2 — Measure source representations and ownership boundaries

**Area:** source acquisition and raw conversion benchmarks · **Priority:** High · **Effort:** Medium

**Purpose:** maintain and improve. Current per-header setup uses HTTP storage,
which is exempt from custom budgets and can share owned data. Add equivalent
`Single`, `Borrowed`, stored-`FieldValue` slice and HTTP cases. Cover 1/2/3/4/5/
16/128 lines, 63/64/65-byte values, totals around 64 KiB and 1,024/1,025 items.
Measure canonical/noncanonical Accept-Ranges and repeated collectors explicitly.
Separately compare CORS `TryFrom<FieldValue>`, `FromStr`, vector-taking conversion
and source `owned` without counting caller setup as parser allocation.

The five shape targets already compare HTTP and borrowed raw sources; retain
those arms and fill the `Single`/stored-slice/size-limit intersections.
Compare default `FieldSink` and HTTP insertion, direct typed construction,
ownership transfer, cloning and destruction as separate operations and as a
complete lifecycle. Include Location/Host/Origin component construction,
Accept-family typed entries, and P27's Allow/Vary construction across the
inline boundary. For P26, distinguish the metadata box from wire storage.
For P21, add serde well-known/custom name deserialization and typed
deserialization with caller setup excluded.

Keep setup and teardown explicit when measuring the shared storage operations
and bounded name corpora. Avoid substituting an artificial decode-and-leak
workload for a consumer that eventually drops its results.

- `crates/http_headers/benches/http_headers_per_header.rs:32` — `fn map(...) -> HeaderMap`
- `crates/http_headers/tests/common/http_headers_storage_operations.rs:38` — shared storage operations return owned maps for post-measurement teardown
- `crates/http_headers/tests/common/http_headers_name_corpus.rs:76` — reusable name corpus setup; HTTP and crate-name corpora are at lines 86 and 97
- `crates/http_headers/src/source/field_lines.rs:273` — representation-aware singleton acquisition
- `crates/http_headers/src/source/field_lines.rs:352` — repeated acquisition
- `crates/http_headers/src/headers/range/accept_ranges.rs:399` — ownership probes
- `crates/http_headers/benches/http_headers_shapes_common.rs:49` — fixture stores both HTTP and raw sources
- `crates/http_headers/src/serde_impls.rs:482` — temporary owned name before recognition; typed owned reconstruction is at line 594
- `crates/http_headers/src/headers/negotiation/allow.rs:86` — typed constructor whose final buffer transfer needs a measured-body case; Vary's corresponding constructor is at `crates/http_headers/src/headers/negotiation/vary.rs:111`

**Done when:** instruction, wall-time, allocation-count and allocated-byte
baselines cover validation and ownership transitions and preserve
sharing/singleton properties; expected limit errors remain distinct from
HTTP-exempt outcomes. Maintain cases fail through B4 on allocation increases
or any instruction increase. Include unknown size hints rather than testing
only exact slices.

These cases unblock P21/P24/P26/P27 and protect SEC3 marker propagation,
wire order, sharing and fail-before-mutation behavior. Also gate unexpected
allocated-byte increases through B4; a lower allocation count alone can hide
a larger retained representation. Measure peak/live bytes with deliberate
setup/drop boundaries, without presenting that as an observed production
memory bottleneck.

---

<a id="b3"></a>
### B3 — Cover nonliteral grammar and fallback distributions

**Area:** parser success/fallback corpus · **Priority:** High · **Effort:** Large

**Purpose:** maintain and improve. Add a stratified corpus rather than only
whole-line literals that coincide with fast paths. Measure wall time,
instructions and allocations, reporting each stratum without inventing
production frequencies. This supplies evidence for fallback paths and input
shapes that the common-case fixtures do not exercise.

- `crates/http_headers/benches/http_headers_per_header.rs:331` — fixed Content-Length example `"348"`
- `crates/http_headers/benches/http_headers_per_header.rs:355` — Host is `example.com:8443`
- `crates/http_headers/benches/http_headers_micro.rs:69` — one three-parameter Content-Type fixture
- `crates/http_headers_simd/benches/http_headers_simd_no_std_dispatch.rs:51` — existing crossover-length scanner cases
- `crates/http_headers_simd/src/api.rs:16` — general UTF-8 validation has alignment-dependent work

Include numeric 19/20-digit and leading-zero inputs, equal/conflicting
duplicates, long/obs-text tags, all date formats, range unit/casing/order forms,
and errors competing with malformed delimiters. Cover Content-Type common and
nonliteral forms, 0/1/2/3/16/49 parameters, first/last/missing/duplicate lookups
and compact-offset boundaries. Exercise late quoted Accept/WebSocket fallback,
weighted relaxed fractions, canonical/noncanonical HSTS, CORS wildcard/mixed-case
tokens and IPv6, Host percent/IPv6/IPvFuture/IDNA, and relaxed Location
backslashes. For Authorization, include scheme spacing, early/late decoded
colon, padding, fresh/warm extraction and success/failure reuse; retain all
zeroization in the measured lifecycle. Include large-to-small credentials and
lengths around the 64-KiB retention cap, recording retained capacity as well as
allocations. Scanner cases cover every tail length, invalid-byte position and
supported backend. Literal-recognition cases pair hits with equal-length misses
and casing variants.

Add explicit slice-offset strata for ASCII/UTF-8 projection and vector
scanners, spanning at least one vector width. Record the input alignment and
compare matching offsets: byte-identical static fixtures can move when
unrelated code changes. Separate alignment effects from parser work through
raw-profile attribution rather than averaging away a slower case.

Reuse the substantial existing `*_shapes` corpora: they already include
nonliteral, repeated, relaxed, absent and malformed HTTP/raw cases. The
outstanding work is the remaining boundary/reader/backend intersections
and trustworthy current baselines, not recreating those fixtures.
P25 needs strict and exact long-fraction qualities, duplicate policy,
quoted parameters/extensions and inline-index overflow; P26 needs literal
controls as well as every general metadata form.

Separate cold credential extraction from a genuinely prewarmed measured
call. `crates/http_headers/benches/http_headers_policy.rs:184` calls
`value.extract(&mut credentials)` before another extraction at line 185
inside the same measured function, so its current warm label includes both.
Keep a combined lifecycle case too, explicitly named, with zeroization.

Record actual backend selection and compiler target flags. The companion's
`http_headers_simd_no_std_dispatch` name does not ensure `no_std`: the
configured all-features bench build and facade feature unification enable
`std`, while `.cargo/config.toml:3` enables x86-64-v3. Add isolated performance
strata for supported generic x86/std/no_std, available accelerated backends,
and AArch64, without executing unsupported instructions. This protects
threshold choices at `crates/http_headers_simd/src/dispatch.rs:17` and
line 30, including Authorization's separate local threshold. The isolated
no_std correctness checks do not replace these crossover measurements;
the no_std-only coverage/mutation exemptions remain unchanged.

**Done when:** value and exact error-kind/index assertions cover these strata,
current baselines are recorded, and B4 gates selected maintain cases at any
unexpected allocation increase or any instruction increase.

These axes unblock P1/P6/P9/P10/P11/P12/P13/P14/P18/P19/P24/P25/P26.
Also apply B4's allocated-byte gate. Retain observed/backend/configuration
provenance so compile-only success cannot be presented as measured fallback
performance or evidence of a vectorized ASCII helper.

**Guideline connection:** [M-UNSAFE](https://microsoft.github.io/rust-guidelines/guidelines/correctness/#M-UNSAFE)
says performance-motivated unsafe should follow benchmarking. These
backend/fallback strata and B5's consumer profiles supply that evidence;
historical threshold comments alone do not. CON3 separately owns the
unassessed current Miri acceptance requirement.

---

<a id="b4"></a>
### B4 — Gate parser instruction and allocation regressions

**Area:** benchmark execution and regression enforcement · **Priority:** High · **Effort:** Medium

**Purpose:** maintain. The generated benchmark check only compiles benchmarks;
it does not notice slower code. Wire a bounded, representative parser suite to
execute instruction/allocation comparisons against a current recorded baseline.
Preserve generated Anvil ownership: express supported configuration upstream
or add justified repository-specific performance automation, not a hand edit
to the generated recipe.

- `justfiles/anvil/checks/bench.just:17` — `cargo ... bench ... --all-features --no-run`
- `justfiles/anvil/groups/scheduled-exhaustive.just:22` — scheduled tier invokes that compile-only recipe
- `crates/http_headers/docs/PERF.md:3` — timing table is explicitly an imported snapshot

Protect short borrowed/owned singleton decode, retained shared HTTP storage,
all header-family semantic readers, custom-source validation and SIMD
crossovers. Proposed deterministic gates are any unexpected allocation-count
increase and any instruction increase per fixed case. Compare identical inputs,
compiler versions, target flags and operation boundaries; never offset a
regression with gains in other cases. Calibrate execution cost and wall-time
repeatability through B6 rather than claiming hosted timing is stable.

Include allocated bytes in the zero-increase gate, not only allocation
count. Version the exact operation identity, consumed semantic work,
input/alignment, ownership and setup/drop boundaries, compiler, profile,
features, target flags and baseline artifacts together. Add B1's repeated-read
barriers before protecting a nominal eight-read workload.

The report script is not a hidden regression gate:
`crates/http_headers/scripts/perf_report.rs:287` selects `--no-baseline`;
its `--check` mode compares rendered documentation with an existing artifact,
not old/new parser costs. The configured Anvil scheduled path remains
compile-only, and no successful runtime CI comparison has been observed
in this audit. Keep both required-check fan-in display names stable.

**Done when:** an intentional regression fails the chosen automated entry point,
baseline/toolchain/hardware provenance and update rules are documented, and B1–B3
maintain cases actually execute. A reporting-only job does not close this item.

---

<a id="b5"></a>
### B5 — Measure downstream release code generation and layouts

**Area:** consumer-profile instruction and layout evidence · **Priority:** Medium · **Effort:** Medium

**Purpose:** improve, then maintain accepted wins. Compare a downstream caller
under ordinary release settings with the current fat-LTO benchmark profile.
Black-box input and semantic results. Inspect retained calls, indirect dispatch,
bounds/overflow checks, aggregate copies, type sizes and emitted error/panic
machinery; include retained Location/Host/origin metadata and Content-Type
layout controls. Account for monomorphized build
cost, code-size growth and hot-field/cache-line layout
instead of assuming all inlining is beneficial.
Include isolated before/after code-size attribution for bounded IPv6 origin
ASCII projection; whole-binary size differences containing other edits do not
establish that helper's footprint.

Expand the retained representation controls to P25's bounded member index
and P26's inline/general metadata, with decode-only and semantic-reader
consumers so a larger result is not hidden by LTO. Inspect actual record
sizes/offsets, `Result` movement, surviving bounds checks, indirect calls,
panic paths and mono-item/code-size attribution. Do not infer any of these
from a Rust field order, missing inline annotation, or a comment claiming
vectorization.

For the conditional recommendations in
[M-AVOID-INDIRECTION](https://microsoft.github.io/rust-guidelines/guidelines/performance/#M-AVOID-INDIRECTION),
[M-BOX-DST](https://microsoft.github.io/rust-guidelines/guidelines/performance/#M-BOX-DST)
and [M-SHRINK-TO-FIT](https://microsoft.github.io/rust-guidelines/guidelines/performance/#M-SHRINK-TO-FIT),
establish applicability before choosing a representation. Record whether
private sequences are immutable, frequently instantiated, large or
long-lived, and their retained capacity slack. Compare reduced indirection
or retained bytes against larger values, construction copies and allocation
costs. Reusable credential buffers and caller-visible mutable collections
are not blanket candidates for boxing or shrinking.

Record clean/incremental consumer build time and code size for minimal,
selected-family, default, HTTP and serde features. The negotiation feature
selects IDNA compiled data at `crates/http_headers/Cargo.toml:73`; that is a
real configuration, not proof of unnecessary dependency cost. Compare a
generic supported CPU with repository x86-64-v3 and relevant AArch64 output.
Use isolated builds where feature unification would obscure the target
configuration; do not change the public defaults just to improve a benchmark.

- `Cargo.toml:423` — release config specifies debug information, not fat LTO
- `Cargo.toml:428` — bench enables fat LTO and one codegen unit
- `crates/http_headers/src/headers/cors/shared.rs:911` — erased validator boundary
- `crates/http_headers/src/headers/content_type.rs:158` — boxed/general metadata tradeoff
- `crates/http_headers/src/headers/negotiation/host.rs:649` — retained host and port metadata
- `crates/http_headers/src/headers/cors/access_control_allow_origin.rs:942` — bounded IPv6 validation and ASCII projection

**Done when:** each codegen hypothesis has actual layout/disassembly and
instruction/time/code-size evidence supporting acceptance or rejection.
Report-only experiments satisfy the improve portion; retained improvements
must gain B4 maintain cases with unchanged allocation expectations and the
zero-regression instruction gate. No unchecked access or removed invariant is an acceptable
substitute for compiler-visible proof.

This is the explicit unblocker for currently unknown retained-call,
vectorization, bounds-check, panic-path, field-layout, monomorphization and
build-footprint questions. B3/B6 supply the relevant inputs and workload
shares; do not turn an emitted-code difference alone into an overall
performance claim.

---

<a id="b6"></a>
### B6 — Establish workload shares and benchmark repeatability

**Area:** request-shaped parsing and measurement calibration · **Priority:** Medium · **Effort:** Medium

**Purpose:** improve. Existing per-header microbenchmarks cannot establish what
fraction of a real request each parser consumes, its input frequencies, or
tail-latency effects. Assemble a non-network request-shaped decode/read bundle
using explicitly sourced, sanitized workload distributions; until those are
available, report separate JSON-request, browser-negotiation and response-policy
strata without claiming a representative weighted average. Vary absent fields,
wire lengths, repeated lines, reader counts and borrowed/owned retention.

- `crates/http_headers/benches/http_headers_micro.rs:87` — existing request fixtures can seed separate strata
- `crates/http_headers/benches/http_headers_per_header.rs:47` — 60-sample Criterion configuration does not establish current variance
- `crates/http_headers/docs/PERF.md:8` — imported per-header timing/instruction/allocation dimensions are not end-to-end shares

Measure total bundle time, per-parser contribution, allocation traffic and
latency distribution; repeat across relevant architectures/profiles. Establish
run-to-run noise and suite duration before selecting timed gates or CI cadence.
This resolves currently unassessed workload shares, tail behavior,
instruction-cache tradeoffs, variance and execution-budget practicality.

Keep attribution additive: measure total bundle cost and identified parser,
source, semantic-reader, construction and sink components against equivalent
work, while retaining the executable paths and raw profiles. Price the
reader-count crossover for P25 and common/nonliteral fractions for P26;
rank P21/P24/P27 only after their actual operation frequencies are known.
Current fixtures cannot supply those probabilities. Synthetic strata remain
report-only improve baselines, not a deployed latency SLO or weighted win.

Make B1's semantic reads observable before collecting repeated runs. Record
setup, drop, cold/warm state and which allocation instrument is actually installed.
The companion's opt-in tracker at
`crates/http_headers_simd/src/tracking.rs:83` uses coherent process-wide
counters; its existence does not establish that every metabench target uses
it. Compare timing and allocation-counting modes separately rather than
attributing harness synchronization to production parsing.

**Done when:** reporting baselines bound plausible end-to-end benefits for
parser optimizations, record distribution provenance and repeatability, and
determine a practical B4 suite/cadence. This is an improve-only measurement: a report is
sufficient here, but it does not replace B4's regression gate.

**Guideline obligations:** [M-HOTPATH](https://microsoft.github.io/rust-guidelines/guidelines/performance/#M-HOTPATH)
recommends identifying, benchmarking and profiling performance-relevant
paths; [M-THROUGHPUT](https://microsoft.github.io/rust-guidelines/guidelines/performance/#M-THROUGHPUT)
recommends throughput and items-per-CPU-cycle evidence. Current compliance
with those measurement recommendations is unassessed, not established by
configured targets. Preserve revision, executable, compiler, target/ISA,
feature, workload and raw-profile provenance, including the retention
distribution needed for B5's conditional representation comparisons.

---

## Testing

<a id="t1"></a>
### T1 — Make counter-lock rendezvous tolerate spurious failure and abort safely

**Area:** `http_headers_simd` allocation-counter concurrency tests · **Priority:** Medium · **Effort:** Small

**Gap type:** Weak · **Would catch:** a broken retry loop that gives up after a spurious weak-CAS failure, without rejecting a correct retry · **Scope:** 1 rendezvous test and its attempt hook, exhaustive · **Blocks / blocked by:** none

Make the test enforce the lock contract rather than guaranteed success of a
particular weak compare-exchange. The production loop correctly retries.
After releasing the guard, however, the controller asserts that the very
next attempt acquired the lock. A permitted spurious failure produces
`false`, so correct production code can fail this assertion. The worker
then waits for another resume message; unwinding through `thread::scope`
waits for that worker while the controller's channel endpoints remain alive.
This can also strand test cleanup and the shared test mutex. This is a
static permitted schedule, not an observed flake or production locking bug.

- `crates/http_headers_simd/src/tracking.rs:43` — retry loop uses `compare_exchange_weak` at line 47; the hook only observes its result
- `crates/http_headers_simd/src/tracking.rs:417` — nearest and affected test creates three zero-capacity channels
- `crates/http_headers_simd/src/tracking.rs:427` — worker reports an attempt, then blocks awaiting resume
- `crates/http_headers_simd/src/tracking.rs:440` — controller requires immediate post-unlock success before sending the final resume
- `crates/http_headers_simd/src/tracking.rs:292` — the other concurrency test checks aggregate accounting, not this retry/abort schedule

Keep this a deterministic unit test. Exercise an injected spurious failure
after unlock followed by success, and make controller failure disconnect or
cancel the worker before joining it. Preserve the assertion that acquisition
cannot complete while the original guard is held. The legal extra failure
is observably different from giving up or acquiring early; neither should be
hidden by weakening assertions or retrying the whole test.

**Done when:** the controlled spurious-failure case completes with exactly one
successful acquisition after release, and fails if the retry is removed or
acquisition is allowed while the first guard is held. A deliberately aborted
controller also releases the worker and test lock with bounded, observable
completion instead of blocking scoped-thread cleanup. Correct production
weak-CAS behavior must not produce a test failure.
