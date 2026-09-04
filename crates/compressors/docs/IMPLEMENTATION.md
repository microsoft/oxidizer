# `compressors` implementation

This document describes the internal mechanisms that enforce the policies in
[DESIGN.md](DESIGN.md): the pump, the codec safety boundary, backend lifecycle,
pooling, and the async driving rules. It covers the invariants that span modules;
anything explainable from one source file is documented there instead.

## Layers

```
compress / decompress / CompressionStream   public entry points
  Compressor / Decompressor                 per-format, macro-generated
    Pump                                    engine-agnostic state machine
      Codec                                 unsafe trait, one impl per direction
        flate2 / brotli / zstd-safe         third-party engines
```

`Pump` knows nothing about any format, and a `Codec` knows nothing about
buffering or limits. Everything format-specific is in the `Codec` impl and the
per-format module; everything policy-related is in `Pump` and the builders.

## The push/pull contract

The engine is driven by alternating `push` and `pull`. `pull` returns one of four
outcomes, and the whole state machine exists to make each of them unambiguous:

- `Data` — bytes are ready.
- `NeedInput` — more input, or `end_input`, is required before progress.
- `Progress` — work happened but produced nothing; call `pull` again without
  pushing.
- `Done` — the operation is complete.

`Progress` and `NeedInput` are deliberately distinct. "No bytes right now" and
"no bytes ever again without more input" require different responses, and
conflating them turns a missing check into an infinite loop. `Output` is
therefore an enum rather than `Option<BytesView>` plus a separate `is_finished`,
and it is deliberately not `#[non_exhaustive]`: the variants describe a complete
step, and forcing a wildcard arm would convert a missing-case bug from a compile
error into silent misbehaviour.

`Pump` tracks where it is with a `State`: `Open`, `Flushing`, `Finishing`,
`BetweenStreams`, `AwaitingEof`, `AtStreamLimit` and `Done`. The states after
`Finishing` are what implement the framing policy — concatenated streams, strict
trailing-data rejection, and the stream-count limit — without any of it reaching
the `Codec` impls.

## The codec safety boundary

`Codec` is an `unsafe` trait, and this is the crate's one soundness-critical
contract.

`Pump::pull` hands an engine the *uninitialized* spare capacity of a `BytesBuf`
and then declares exactly as many bytes initialized as the engine reports
writing. An implementation must therefore leave `output[..produced]` genuinely
initialized, never read from `output`, never write past `output.len()`, and never
touch memory outside the two slices it was given. Reporting a count it did not
write exposes uninitialized memory.

The three backend families split two to one on how they satisfy this:

| Family | How it writes into uninitialized memory |
|---|---|
| flate (`deflate`, `zlib`, `gzip`) | `flate2`'s `*_uninit` entry points |
| zstd | `zstd_safe::WriteBuf` |
| brotli | initializes the slice first, because its encoder takes `&mut [u8]` |

Brotli's zero-fill is a real cost that the other two do not pay, so it is done
with a bulk `fill` rather than per element. `UninitOutput::filled_until` clamps
the reported count against the backing slice, so an engine that over-reports is
rejected before `advance` is reached rather than trusted.

Each family implements the trait twice, once per direction, giving six adapters.

## Bounding one `pull`

Two guards keep a single `pull` from running away, and both are the kind of
invariant that is invisible until it is wrong:

- `yields_to_the_caller` caps the engine calls and the input consumed per `pull`,
  so one call cannot run until the stream ends.
- `made_no_progress` detects a step that moved neither input nor output, which
  means the engine is stuck; it is the only guard against an engine that can
  never finish.

Both are excluded from mutation testing, because a mutant of either produces a
program that hangs rather than fails, and the harness records a timeout instead
of a verdict. The exclusion is attached to the attribute rather than the doc
comment, so the item's documentation stays about behaviour.

## Backend lifecycle and pooling

Pooling is what makes engine reuse worth the complexity, and the constraint is
that a reused engine must be indistinguishable from a fresh one.

An engine is reset before it is handed out, not when it is returned, so an engine
dropped part-way through a stream cannot leak state into the next user. Reset is
also what decides whether an engine can be pooled at all:

| Engine | Pooled? | Why |
|---|---|---|
| flate compressors | yes | `reset` preserves container and level, so the pool keys on both |
| `deflate` / `zlib` decompressors | yes | `reset` restores the framing |
| `gzip` decompressor | **no** | `flate2`'s reset takes a boolean that cannot express gzip framing, so a recycled engine would silently decode as raw deflate |
| zstd compressor and decompressor | yes | `reset` keeps the context's allocations, which is where the cost is |
| brotli compressor and decompressor | **no** | no upstream reset; recycling its buffers through a custom allocator was measured and did not pay for itself |

The gzip decompressor is the one gap worth explaining, because gzip is the most
common encoding on the wire. Nothing about gzip prevents recycling — the obstacle
is only that the engine's reset cannot express its framing. Taking that framing
over here would mean owning header parsing and checksum validation permanently to
route around an upstream API gap. If `flate2` gains a reset that can express gzip
framing, gzip decompressors can start being pooled with no change to calling code.

The zstd compressor pool is **unkeyed**. Checkout resets with
`SessionAndParameters` and the compressor then applies its level unconditionally,
so any idle context serves any level. Keying by level would fragment reuse and,
worse, let a caller-chosen level grow the map without bound.

Each engine class has its own `Mutex` rather than one lock over everything, so a
compressor being returned never waits on a decompressor being taken. Poisoning is
contained to the one class whose critical section panicked: every checkout treats
a poisoned lock as "nothing to reuse" and builds a fresh engine.

## Async driving

`poll_compression` drives both directions and enforces two rules that the
`Stream` contract does not:

- The source is polled **only when the engine has nothing left to give**, so a
  slow consumer never causes unbounded buffering. This is the streaming half of
  the "what the caller retains" policy.
- A `finished` flag latches once the last item has been yielded. Without it, a
  failing engine would report the same error on every subsequent poll, and a
  caller collecting the stream would accumulate errors until it ran out of
  memory.

Immediately-ready work per poll is capped so the task yields to its executor
rather than starving its peers.

## Runtime format dispatch

`format::Compressor` and `format::Decompressor` hold an enum of the compiled-in
formats and forward through a `dispatch!` macro, so adding a format does not add
a match arm at every call site.

Two details make the always-compiled requirement work:

- **Brotli variants are boxed, the rest are inline.** Brotli's encoder state is
  around 6480 bytes against roughly 1024 for the next largest, so an entirely
  unboxed enum would make every runtime-format value — including a gzip one —
  carry the brotli footprint. Boxing only brotli keeps the common path
  allocation-free.
- **A zero-format build needs an uninhabited placeholder.** With no feature
  enabled the enum would have no variants, and matching a *reference* to a
  zero-variant enum is not exhaustive. An `Impossible(Infallible)` variant keeps
  the type well-formed; there is no way to construct one.

## Test build configuration

Feature-dependent code is gated `cfg(any(test, feature = "..."))` per
[docs/optional-deps-in-test-builds.md](../../../docs/optional-deps-in-test-builds.md),
and every optional dependency is mirrored as a non-optional dev-dependency. The
crate's own test build therefore compiles every format, engine, stream adapter
and pool without the test target enumerating features.

The consequence worth knowing before adding a test: the test build is a
*superset*, so a test cannot observe a format being absent. Assertions of the
form "this token is rejected when the feature is off" have no configuration in
which they hold and do not belong here. `cfg!` call sites need the same treatment
as the attributes, since they are resolved at run time rather than during
expansion.

Shared fixtures and helpers live in `src/testing.rs`, following the workspace's
`mod testing` convention, so production modules carry only production code.

## Verification

- The format contract suite drives every compiled format through the same
  behavioural checks, so a new format cannot be added without satisfying them,
  and a guard test fails if `Format::ALL` grows without the suite growing.
- Round-trip tests decode fixtures produced by the system `gzip` to check
  interoperability against a real encoder rather than only against this crate.
- Drain loops in tests are step-capped so a spinning implementation fails rather
  than hangs, which is also what lets mutation testing reach a verdict.
- Benchmarks in `benches/` cover throughput and allocation behaviour; see
  [benchmarks.md](../../../docs/benchmarks.md) for the conventions they follow.
