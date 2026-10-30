# `compressors` implementation

This document describes internal mechanisms that apply across modules.
It covers the pump, the codec safety boundary, backend lifecycle, pooling, and
the async driving rules. [DESIGN.md](DESIGN.md) describes the user-visible
policies.

## Layers

```
compress / decompress / CompressionStream   public entry points
  Compressor / Decompressor                 per-format, macro-generated
    Pump                                    engine-agnostic state machine
      Codec                                 unsafe trait, one impl per direction
        flate2 / brotli / zstd-safe         third-party engines
```

`Pump` does not depend on a format. A `Codec` does not manage buffering or
limits. Each `Codec` implementation and format module contain format-specific
behavior. `Pump` and the builders apply shared policy.

## The push/pull contract

The caller alternates `push` and `pull`. `pull` returns one of four outcomes:

- `Data`: Bytes are ready.
- `NeedInput`: More input or `end_input` is required before progress.
- `Progress`: Work occurred but produced no bytes. Call `pull` again without
  pushing.
- `Done`: The operation is complete.

`Progress` and `NeedInput` require different responses. `Progress` means that the
caller must pull again. `NeedInput` means that the caller must push input or end
the input. `Output` combines this state with the returned bytes. It is not
`#[non_exhaustive]` because every variant requires a specific response.

`Pump` tracks where it is with a `State`: `Open`, `Flushing`, `Finishing`,
`BetweenStreams`, `AwaitingEof`, `AtStreamLimit` and `Done`. The states after
`Finishing` implement the framing policy. This policy covers concatenated
streams, strict trailing-data rejection, and the stream-count limit. It does not
reach the `Codec` implementations.

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
with a bulk `fill` rather than per element.

`Pump::pull` enforces the contract for every engine. Before it advances state,
it rejects a step that reports `consumed > supplied` or
`produced > provided_output`. The pump then enters the failed state. This check
prevents an engine from marking uninitialized output as initialized.
`WriteBuf::filled_until` has a different purpose. It records how much of the
buffer zstd can read back.

Each family implements the trait twice, once per direction, giving six adapters.

## Bounding one `pull`

Two guards keep a single `pull` from running away, and both are the kind of
invariant that is invisible until it is wrong:

- `yields_to_the_caller` caps the engine calls and the input consumed per `pull`,
  so one call cannot run until the stream ends.
- `made_no_progress` detects a step that moved neither input nor output, which
  means that the engine is stuck. It is the only guard against an engine that can
  never finish.

Both are excluded from mutation testing, because a mutant of either produces a
program that hangs rather than fails, and the harness records a timeout instead
of a verdict. The exclusion is attached to the attribute rather than the doc
comment, so the item's documentation stays about behavior.

## Bounding cumulative output

Output bounds are cumulative. Whether they apply depends on what the caller does
with the result. Therefore, `pull` takes a `Destination`. `Pump` combines two
sources of budget:

- `Codec::remaining_output` and `Codec::check_limits`, which report whatever the
  decompressor was configured with. These apply to every destination.
- `Pump`'s own `buffered_ceiling`, set at build time from
  `DecompressorLimits::buffered_ceiling`. This applies only to
  `Destination::Buffer`, since a caller that consumes and drops each chunk
  retains nothing that cumulative output measures.

`Pump::remaining_output` narrows one budget by the other and `Pump::check_limits`
tests both, so the tighter bound decides and the two call sites cannot drift
apart on which bounds they test.

The step loop uses the budget to size the output slice. An expansion bomb stops
at the bound, except for one probe byte. `limit_budget.max(1)` always offers at
least one byte. This byte lets an engine finish a stream that ends exactly at
the bound. The caller never receives the probe byte.

## Backend lifecycle and pooling

A reused engine must behave like a fresh engine.

The pool resets an engine before checkout, not on return. This order prevents an
engine that was dropped during a stream from leaking state to the next user.
Reset support also determines whether an engine can enter the pool:

| Engine | Pooled? | Why |
|---|---|---|
| flate compressors | yes | `reset` preserves container and level, so the pool keys on both |
| `deflate` / `zlib` decompressors | yes | `reset` restores the framing |
| `gzip` decompressor | no | `flate2`'s reset takes a boolean that cannot express gzip framing, so a recycled engine decodes as raw deflate |
| zstd compressor and decompressor | yes | `reset` keeps the context's allocations, which is where the cost is |
| brotli compressor and decompressor | no | No upstream reset exists. Measurements did not justify recycling its buffers through a custom allocator. |

Gzip framing does not prevent recycling. The `flate2` reset API cannot express
gzip framing. This crate does not replace the upstream header parsing and
checksum validation. If `flate2` adds the required reset support, this crate can
pool gzip decompressors without an API change.

The zstd compressor pool is unkeyed. Checkout resets with
`SessionAndParameters` and the compressor then applies its level unconditionally,
so any idle context serves any level. A level key fragments reuse. It also lets
caller-selected levels grow the map without a bound.

Each engine class has its own `Mutex` rather than one lock over everything, so a
compressor being returned never waits on a decompressor being taken. Poisoning is
contained to the one class whose critical section panicked: every checkout treats
a poisoned lock as "nothing to reuse" and builds a fresh engine.

## Async driving

`poll_compression` drives both directions and enforces two rules that the
`Stream` contract does not:

- The source is polled only when the engine has no pending output. A
  slow consumer never causes unbounded buffering. This is the streaming half of
  the "what the caller retains" policy.
- A `finished` flag latches after the stream yields the last item. Without it, a
  failing engine can report the same error on every subsequent poll. A caller
  that collects the stream can then accumulate errors until it runs out of
  memory.

Immediately-ready work per poll is capped so the task yields to its executor
rather than starving its peers.

## Runtime format dispatch

`format::Compressor` and `format::Decompressor` hold an enum of the compiled-in
formats and forward through a `dispatch!` macro, so adding a format does not add
a match arm at every call site.

Two details keep runtime dispatch available with every feature combination:

- Brotli variants are boxed. The other variants are inline. Brotli's encoder
  state is around 6480 bytes against roughly 1024 for the next largest, so an entirely
  unboxed enum makes every runtime-format value, including a gzip value,
  carry the brotli footprint. Boxing only brotli keeps the common path
  allocation-free.
- A zero-format build needs an uninhabited placeholder. With no feature
  enabled, the enum has no format variants. Matching a reference to a
  zero-variant enum is not exhaustive. An `Impossible(Infallible)` variant keeps
  the type well-formed. No caller can construct this variant.

## Test build configuration

Feature-dependent code is gated `cfg(any(test, feature = "..."))` per
[docs/optional-deps-in-test-builds.md](../../../docs/optional-deps-in-test-builds.md),
and every optional dependency is mirrored as a non-optional dev-dependency. The
crate's own test build therefore compiles every format, engine, stream adapter
and pool without the test target enumerating features.

The test build is a superset, so a test cannot observe an absent format.
Do not add assertions that require a format feature to be disabled.
Apply the same rule to `cfg!` call sites because they resolve at run time.

Shared fixtures and helpers live in `src/testing.rs`, following the workspace's
`mod testing` convention, so production modules carry only production code.

## Verification

- The format contract suite drives every compiled format through the same
  behaviors, so a new format cannot be added without satisfying them,
  and a guard test fails if `Format::ALL` grows without the suite growing.
- Round-trip tests decode fixtures produced by the system `gzip`. This makes
  sure that decoding works with a real encoder, not only with this crate.
- Drain loops in tests are step-capped so a spinning implementation fails rather
  than hangs, which is also what lets mutation testing reach a verdict.
- Benchmarks in `benches/` cover throughput and allocation behavior. See
  [benchmarks.md](../../../docs/benchmarks.md) for the conventions they follow.
