# `compressors` security model

The trust model this crate is written against, the resource budgets it does and
does not enforce, and how those claims are verified. [DESIGN.md](DESIGN.md)
covers why the API is shaped as it is; this covers what an attacker can do and
what stops them.

## 1. Trust model

A compressed byte sequence is **untrusted** whenever its contents, its length, or
its framing are chosen by anyone you would not run code on behalf of. An HTTP
request or response body, an uploaded file, and a message off a queue are all
untrusted. Data your own process compressed a moment ago is not.

**Decompression does not upgrade content trust.** A stream that decodes cleanly,
whose CRC-32 or XXH64 matches, and whose trailer is well formed, is exactly as
untrusted afterwards as it was before. A checksum says the bytes survived the
wire; it says nothing about who chose them. Every format here is unauthenticated:
none carries a signature or a MAC, so a checksum match is not an authenticity
claim, and this crate never treats it as one.

What decompression *does* change is cost. The input is the attacker's budget, the
output is yours, and the exchange rate is the format's business. That asymmetry is
the whole threat.

### In scope

Resource exhaustion driven by the shape of the compressed input: unbounded
expansion, unbounded stream counts, unbounded codec working memory, and engines
that consume input without making progress.

### Not in scope

* **Authenticity and confidentiality.** Sign or encrypt separately, and do it
  before decompressing if the sender is untrusted.
* **Compression side channels.** Compressing attacker-influenced data together
  with a secret leaks the secret through the output length -- the CRIME and BREACH
  family. No bound here helps; do not compress secrets alongside untrusted input.
* **Backend vulnerabilities.** Memory-safety defects in `flate2`/`zlib-rs`,
  `brotli` or `zstd` are upstream. See
  [IMPLEMENTATION.md](IMPLEMENTATION.md) for the codec safety boundary this crate
  maintains on top of them.
* **Process-wide limits.** Total concurrency, per-request deadlines and
  cancellation belong to the application; see §5.

## 2. The budgets are independent

Each bound covers a resource the others do not. None substitutes for another.

| Budget | Bounds | Configured by |
|---|---|---|
| Encoded input | Bytes you feed in | The caller, before `push` |
| Decoded output | Cumulative bytes produced | [`max_output_len`] |
| Expansion ratio | Output relative to input | [`max_ratio`] |
| Stream count | Concatenated members decoded | [`max_streams`] |
| Codec memory | Window/context the engine allocates | `max_window_log` (zstd), format defaults |
| Retained memory | What the caller holds at once | The chunk size, and what the caller keeps |

Two pairings are worth stating outright, because each is a case where the obvious
bound does not fire:

* **Ratio does not bound output.** A ratio loose enough to admit legitimate
  highly-compressible data is loose enough to admit a bomb, in any format without
  a structural expansion ceiling. Only deflate has one (~1032x). For brotli and
  zstd the ratio is a coarse backstop, not protection.
* **Output does not bound stream count.** Many tiny members each pay a full engine
  setup while producing almost no output, so an output ceiling never trips on
  them. That is what the stream cap is for.

## 3. What applies where

The bound that matters depends on what the caller *keeps*, not on how much the
engine produces. A consumer that hands every chunk straight on retains only one
chunk however long the stream is; a consumer that accumulates retains everything.

| Consumption mode | Cumulative bounds | Why |
|---|---|---|
| Driving `Decompressor` directly | Only what you configured | You choose what to keep; a cumulative default would cap legitimate long streams |
| [`CompressionStream`] | Only what you configured | Same: each chunk is yielded and dropped |
| `compressors::decompress` | Yours, else 64 MiB output and 1024 streams | Buffers the whole result |
| `<format>::decompress` and `decompress_with_limits`, and the same pair on `Format` | Yours, else 64 MiB output and 1024 streams | Buffers the whole result |

The defaults are **fallbacks, not overrides**: they fill only bounds left unset.
An explicit value wins, and so does an explicit
[`DecompressorLimits::UNLIMITED`] -- removing a bound is a decision too.

The engine, not the caller, applies them. Every `pull` states whether its output is
being streamed or accumulated, and the pump narrows the slice it offers the engine
by whichever bound is tighter. A bomb therefore stops being produced *at* the
bound rather than being measured after the fact, and no buffering caller can get it
wrong by forgetting to count.

## 4. Format defaults

| Format | Ratio | Output | Streams | Concatenated members | Window |
|---|---|---|---|---|---|
| `deflate` | 1100x | none | none | not decoded | 32 KiB, fixed |
| `zlib` | 1100x | none | none | not decoded | 32 KiB, fixed |
| `gzip` | 1100x | none | none | **decoded by default** | 32 KiB, fixed |
| `brotli` | none | none | none | not decoded | up to 16 MiB (window bits 24) |
| `zstd` | 250 000x | none | none | **decoded by default** | 128 MiB default, `max_window_log` to lower |

Notes that matter for untrusted input:

* **gzip and zstd decode concatenated members by default**, because that is what
  their ecosystems produce. This is what makes the stream cap load-bearing for
  them. Turn it off with `multi_stream(false)` if you expect exactly one.
* **zstd's window is the largest single allocation an attacker controls.** A frame
  header can ask for up to 128 MiB by default, before any output is produced.
  Lower `max_window_log` when the producer is untrusted and you know your data
  does not need a large window.
* **Trailing data is rejected by default.** Bytes after a complete stream are an
  error rather than being silently discarded, because silent discard is a request
  smuggling primitive when a peer and this decoder disagree about where a message
  ends.
* **Output is provisional until `Done`.** A checksum or trailer can reject a stream
  after earlier chunks were handed back. Do not act on decompressed bytes until the
  decompressor reports completion.

## 5. What the caller still owns

These bounds are per operation. They say nothing about how many operations run at
once, and none of them is a substitute for the following:

* **Concurrency.** 64 MiB is a per-operation guardrail; a thousand concurrent
  decompressions of 64 MiB is 64 GiB. Bound how many bodies you decompress at once.
* **Deadlines and cancellation.** This crate performs no I/O and spawns nothing, so
  it never blocks on the network, but it also cannot time an operation out. One
  `pull` is bounded in work -- capped engine steps and input per call -- so a
  caller driving it can always regain control and stop, but the decision to stop is
  the caller's.
* **Input length.** Nothing here bounds how much compressed input you accept. Cap
  it where you read it.
* **Memory provisioning.** Buffers come from the [`Resources`] memory provider the
  caller supplies, so a caller that wants a hard ceiling can impose one there.

## 6. How the model is verified

Claims above are held up by tests rather than by review alone:

* **Boundary tests.** Every cumulative bound is exercised at the bound and one past
  it, so an off-by-one in either direction fails. The four-row fallback table --
  unset, raised, removed, lowered -- is asserted for the output and stream bounds
  through the crate-level `decompress`.
* **The engine applies them, and it is tested there.** Pump-level tests cover the
  tighter-of-two-bounds rule, that a streaming destination is unbounded, and that
  an exactly-at-the-bound stream is admitted.
* **State transitions.** Concatenated members, trailing data, truncation and
  mid-stream corruption are covered per format through a shared contract suite, so
  a format cannot quietly differ from its siblings.
* **Cross-implementation fixtures.** gzip streams produced by the system `gzip`
  are decoded in tests, so the decoder is not only checked against this crate's own
  encoder.
* **Codec boundary.** The engine rejects any backend that reports consuming or
  producing more than it was given, before those counts are used to mark output
  initialized. See [IMPLEMENTATION.md](IMPLEMENTATION.md).
* **Coverage and mutation.** A 100% line-coverage gate runs over two feature
  configurations, and `cargo-mutants` runs on every pull request; the limit
  comparisons are mutation-covered, so a bound that stopped being enforced would
  fail rather than pass quietly.
* **Fuzzing.** `tests/bolero_compressors.rs` is a bounded campaign over the
  decompression surface, run on every pull request. It generates a format, a
  payload, a corruption (truncate, bit flip, append, concatenate), a span layout,
  an output chunk size and a bound placed at or beside the real output size, and
  asserts that decompression never panics and never fails to terminate, that an
  unmutated stream round-trips, and that a bound below the real output is refused.
  It covers what the deterministic tests cannot: the product of those dimensions,
  where a member boundary coincides with a span boundary, an exact limit and
  trailing bytes at once.

Compressor-side operation ordering -- where a flush falls, a repeated
`end_input` -- is not fuzzed. That ordering is chosen by trusted calling code
rather than by input bytes, so it is covered by the deterministic suite instead.

[`max_output_len`]: https://docs.rs/compressors/latest/compressors/struct.DecompressorLimits.html#method.max_output_len
[`max_ratio`]: https://docs.rs/compressors/latest/compressors/struct.DecompressorLimits.html#method.max_ratio
[`max_streams`]: https://docs.rs/compressors/latest/compressors/struct.DecompressorLimits.html#method.max_streams
[`DecompressorLimits::UNLIMITED`]: https://docs.rs/compressors/latest/compressors/struct.DecompressorLimits.html#associatedconstant.UNLIMITED
[`CompressionStream`]: https://docs.rs/compressors/latest/compressors/struct.CompressionStream.html
[`Resources`]: https://docs.rs/compressors/latest/compressors/struct.Resources.html
