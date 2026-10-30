# `compressors` security model

This document defines the trust model and resource budgets for this crate.
It also describes how tests support these claims. [DESIGN.md](DESIGN.md)
describes the API design.

## 1. Trust model

A compressed byte sequence is untrusted when an untrusted source selects its
contents, length, or framing. An HTTP
request or response body, an uploaded file, and a message off a queue are all
untrusted. Data your own process compressed a moment ago is not.

Decompression does not make content trusted. A stream that decodes cleanly,
whose CRC-32 or XXH64 matches, and whose trailer is well formed, is exactly as
untrusted afterwards as it was before. A checksum says the bytes survived the
wire. It does not identify who selected them. These formats do not carry a
signature or a MAC. Therefore, a checksum match does not prove authenticity.

Decompression changes resource cost. The attacker controls the input size, but
the application pays for the output and codec memory.

### In scope

Resource exhaustion driven by the shape of the compressed input: unbounded
expansion, unbounded stream counts, unbounded codec working memory, and engines
that consume input without making progress.

### Not in scope

- Authenticity and confidentiality. Sign or encrypt separately. If the sender is
  untrusted, do this before decompression.
- Compression side channels. Compressing attacker-influenced data with a secret
  can leak the secret through output length. CRIME and BREACH use this class of
  attack. Do not compress secrets with untrusted input.
- Backend vulnerabilities. Memory-safety defects in `flate2`/`zlib-rs`,
  `brotli` or `zstd` are upstream. See
  [IMPLEMENTATION.md](IMPLEMENTATION.md) for the codec safety boundary this crate
  maintains on top of them.
- Process-wide limits. The application controls total concurrency, per-request
  deadlines, and cancellation. See section 5.

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

Two limits do not substitute for each other:

- Ratio does not bound output. A ratio that accepts valid
  highly-compressible data is loose enough to admit a bomb, in any format without
  a structural expansion ceiling. Only deflate has one (~1032x). For brotli and
  zstd the ratio is a coarse backstop, not protection.
- Output does not bound stream count. Many tiny members each require a full engine
  setup while producing almost no output, so an output ceiling never trips on
  them. That is what the stream cap is for.

## 3. What applies where

The bound that matters depends on what the caller *keeps*, not on how much the
engine produces. A consumer that hands every chunk straight on retains only one
chunk for any stream length. A consumer that accumulates retains everything.

| Consumption mode | Cumulative bounds | Why |
|---|---|---|
| Driving `Decompressor` directly | Only what you configured | You choose what to keep. A cumulative default can cap valid long streams. |
| [`CompressionStream`] | Only what you configured | The adapter yields one bounded chunk at a time. The caller decides what to retain. |
| `compressors::decompress` | Yours, else 64 MiB output and 1024 streams | Buffers the whole result |
| `<format>::decompress` and `decompress_with_limits`, and the same pair on `Format` | Yours, else 64 MiB output and 1024 streams | Buffers the whole result |

The defaults are fallbacks, not overrides. They fill only bounds left unset.
Each limit setter accepts only a non-zero value. The matching
[`unbounded_ratio`], [`unbounded_output_len`], or [`unbounded_streams`] method
removes that bound, including its buffering fallback.
[`DecompressorLimits::UNLIMITED`] removes every bound. Removing a bound is a
decision too, not a request to restore its default.

The engine applies the bounds. Every `pull` states whether the destination
streams or accumulates output. The pump narrows the output slice to the tighter
bound. Caller-visible output stops at the bound. The engine can produce one
discarded probe byte to distinguish an exact-bound stream from an over-limit
stream.

## 4. Format defaults

| Format | Ratio | Output | Streams | Concatenated members | Window |
|---|---|---|---|---|---|
| `deflate` | 1100x | none | none | not decoded | 32 KiB, fixed |
| `zlib` | 1100x | none | none | not decoded | 32 KiB, fixed |
| `gzip` | 1100x | none | none | decoded by default | 32 KiB, fixed |
| `brotli` | none | none | none | not decoded | up to 16 MiB (window bits 24) |
| `zstd` | 250 000x | none | none | decoded by default | 128 MiB default, `max_window_log` to lower |

For untrusted input, use these rules:

- Gzip and zstd decode concatenated members by default because their ecosystems
  produce them. This is what makes the stream cap load-bearing for
  them. Turn it off with `multi_stream(false)` if you expect exactly one.
- The zstd window is the largest single allocation an attacker controls. A frame
  header can ask for up to 128 MiB by default, before any output is produced.
  Lower `max_window_log` when the producer is untrusted and you know your data
  does not need a large window.
- Trailing data is rejected by default. Bytes after a complete stream are an
  error rather than being silently discarded, because silent discard is a request
  smuggling primitive when a peer and this decoder disagree about where a message
  ends.
- Output is provisional until `Done`. A checksum or trailer can reject a stream
  after earlier chunks were handed back. Do not act on decompressed bytes until the
  decompressor reports completion.

## 5. What the caller still owns

These bounds are per operation. They say nothing about how many operations run at
once, and none of them is a substitute for the following:

- Concurrency. 64 MiB is a per-operation limit. One thousand concurrent
  decompressions of 64 MiB is 64 GiB. Bound how many bodies you decompress at once.
- Deadlines and cancellation. This crate performs no I/O and spawns nothing, so
  it never blocks on the network, but it also cannot time an operation out. One
  `pull` limits engine steps and input per call. The caller regains control after
  each call and decides when to stop.
- Input length. Nothing here bounds how much compressed input you accept. Cap
  it where you read it.
- Memory provisioning. Buffers come from the [`Resources`] memory provider the
  caller supplies, so a caller that wants a hard ceiling can impose one there.

## 6. How the model is verified

The following tests support these claims:

- Boundary tests. Every cumulative bound is exercised at the bound and one past
  it, so an off-by-one in either direction fails. Tests cover the four fallback
  states: unset, raised, removed, and lowered. They cover the output and stream bounds
  through the crate-level `decompress`.
- Engine tests. Pump-level tests cover the
  tighter-of-two-bounds rule, that a streaming destination is unbounded, and that
  an exactly-at-the-bound stream is admitted.
- State transitions. Concatenated members, trailing data, truncation and
  mid-stream corruption are covered per format through a shared contract suite, so
  a format cannot quietly differ from its siblings.
- Cross-implementation fixtures. Gzip streams produced by the system `gzip`
  are decoded in tests, so the decoder is not only checked against this crate's own
  encoder.
- Codec boundary. The engine rejects any backend that reports consuming or
  producing more than it was given, before those counts are used to mark output
  initialized. See [IMPLEMENTATION.md](IMPLEMENTATION.md).
- Coverage and mutation. A 100% line-coverage gate runs over two feature
  configurations. `cargo-mutants` runs on every pull request. The limit
  comparisons are mutation-covered, so a bound that stops being enforced will
  fail rather than pass quietly.
- Fuzzing. `tests/bolero_compressors.rs` is a bounded campaign over the
  decompression surface, run on every pull request. It generates a format, a
  payload, a corruption (truncate, bit flip, append, concatenate), a span layout,
  an output chunk size and a bound placed at or beside the real output size, and
  asserts that decompression never panics and never fails to terminate, that an
  unmutated stream round-trips, and that a bound below the real output is refused.
  It covers what the deterministic tests cannot: the product of those dimensions,
  where a member boundary coincides with a span boundary, an exact limit and
  trailing bytes at once.

Compressor-side operation ordering, such as flush placement and repeated
`end_input`, is not fuzzed. Trusted calling code, not input bytes, selects that
ordering. The deterministic suite covers it.

[`max_output_len`]: https://docs.rs/compressors/latest/compressors/struct.DecompressorLimits.html#method.max_output_len
[`max_ratio`]: https://docs.rs/compressors/latest/compressors/struct.DecompressorLimits.html#method.max_ratio
[`max_streams`]: https://docs.rs/compressors/latest/compressors/struct.DecompressorLimits.html#method.max_streams
[`unbounded_output_len`]: https://docs.rs/compressors/latest/compressors/struct.DecompressorLimits.html#method.unbounded_output_len
[`unbounded_ratio`]: https://docs.rs/compressors/latest/compressors/struct.DecompressorLimits.html#method.unbounded_ratio
[`unbounded_streams`]: https://docs.rs/compressors/latest/compressors/struct.DecompressorLimits.html#method.unbounded_streams
[`DecompressorLimits::UNLIMITED`]: https://docs.rs/compressors/latest/compressors/struct.DecompressorLimits.html#associatedconstant.UNLIMITED
[`CompressionStream`]: https://docs.rs/compressors/latest/compressors/struct.CompressionStream.html
[`Resources`]: https://docs.rs/compressors/latest/compressors/struct.Resources.html
