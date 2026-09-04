# `compressors` design

This document records the cross-cutting, user-visible decisions of the
`compressors` crate — the ones that span several APIs and cannot be read off any
single item. Per-item behaviour lives in the rustdoc and is not repeated here.
The mechanisms that enforce these decisions are described separately in
[IMPLEMENTATION.md](IMPLEMENTATION.md).

## 1. Purpose

`compressors` streams compression and decompression over [`bytesbuf`] byte
sequences. It sits in front of several third-party engines — `flate2`, the
`brotli` crate, `zstd-safe` — and gives them one contract, so that code written
against it works with whichever format it is handed.

Two properties motivate the crate's existence and constrain everything below:

- **Segmented buffers throughout.** Input is read from a `BytesView`'s existing
  segments and output is written into a `BytesBuf`'s uninitialized spare
  capacity. Nothing is flattened on the way in and nothing is copied out on the
  way back.
- **Recycled engine state.** Building an engine allocates and initializes
  working memory that, on a small message, costs about as much as the
  compression itself. `Resources` retains that state between messages.

## 2. Selecting a format

A format is chosen in one of two ways, and both reach the same engines.

**At compile time**, through a module: `gzip`, `deflate`, `zlib`, `brotli`,
`zstd`. Each is behind a cargo feature of the same name, and none is enabled by
default, so a build compiles only the engines it names.

**At run time**, through the `format` module, which resolves a `Content-Encoding`
token to a `Format` and offers the same shape the per-format modules do. This
module is always compiled, including with no features at all, so a crate that
only passes compressors around does not have to enable a format to name the type.

The `deflate` feature and the HTTP `deflate` content coding are deliberately not
the same thing, because the ecosystem's names disagree with the specifications:
`Format::Deflate` is raw DEFLATE (RFC 1951) and has no content-coding token,
while the HTTP `deflate` token denotes a zlib-wrapped stream (RFC 1950) and
therefore resolves to `Format::Zlib`.

## 3. What is uniform, and what is not

The conveniences are uniform: `compress`, `decompress`, `Compressor` and
`Decompressor` have the same shape in every format module, so moving a call site
between formats is a change of import.

The builders are deliberately not uniform. `brotli` and `zstd` expose settings
that only they have — quality, window size, mode, native compression level — and
a builder that might produce any format cannot honour them. Switching a builder
call site between formats can therefore take more than an import change, and
`zstd`'s compressor build returns a `Result` because its native library validates
what it is given.

## 4. Accepting input

Whole-buffer entry points take `impl InputData`, a sealed trait implemented for
`BytesView`, `&[u8]` and `&[u8; N]`. A caller who already holds a `BytesView`
passes it through unchanged; a caller who holds a slice does not have to
construct one, and does not have to decide which memory provider it should come
from. The trait is sealed because the set of things this crate can accept
without copying is a property of `bytesbuf`, not an extension point.

## 5. Bounded work, at any size

An engine is a state machine rather than a one-shot transform, so a stream of any
length passes through it while the output it has buffered but not yet handed back
stays bounded by the configured chunk size. Pending input and the engine's own
window and tables are additional, and their size depends on the format and its
configuration — the chunk size is not a total-memory ceiling.

Two consumption models are offered and they differ in what the *caller* retains:

- `CompressionStream`, behind the `futures-stream` feature, yields one bounded
  chunk at a time, so a consumer that processes and drops each chunk stays
  bounded however long the stream is. This is the incremental model available to
  callers: the push/pull mechanics underneath it are crate-private (see
  §10), so a downstream crate reaches them through this adapter rather than
  directly.
- The whole-buffer conveniences accumulate the entire result, which is what makes
  them convenient and also what makes them the APIs that need bounding.

## 6. Bounding untrusted decompression

Every supported format can expand its input by orders of magnitude, so a
decompressor pointed at untrusted data is a memory-exhaustion vector.

The decision that shapes the whole area: **retained output is the boundary, not
throughput.** A cumulative output cap protects a caller who accumulates; applying
one to a pipeline that retains nothing only rejects legitimately long streams.
The documentation is written to that condition rather than to "the input is
untrusted".

`DecompressorLimits` carries three independent bounds — a ratio, a total output
length, and a count of concatenated streams. Two policies follow:

- A ratio bound is a coarse backstop, not protection. Only the deflate family has
  a structural expansion ceiling; brotli and zstd do not, so for them a ratio
  loose enough to admit legitimate data is also loose enough to admit a bomb.
  Those formats declare no ratio default rather than a reassuring one.
- The conveniences that buffer a whole result add their own output and stream
  caps on top of whatever the caller did not set, because those are the APIs
  whose exposure the caller cannot otherwise bound.

## 7. Stream framing

Whether bytes after a complete compressed stream begin another one is a
per-format default, because the formats genuinely differ: gzip and zstd define
concatenation, the others do not.

When concatenation is off, `TrailingData` decides what happens to those bytes,
and it defaults to `Reject`. Silently discarding a suffix is a parser
differential: a proxy or scanner built on this crate would see only the benign
prefix of a body whose tail something downstream still reads. Accepting a suffix
is a legitimate need — a container whose framing puts other data after the
compressed stream — but it is an opt-in.

Decompression can also yield bytes before a checksum or trailer has rejected the
stream, so output is provisional until the decompressor reports that it is done.

## 8. Resources and recycling

`Resources` is what an engine is built *with*, as distinct from a builder, which
describes what it should *do*. It carries a memory provider and a pool of idle
engine state, is cheap to clone, and is expected to be held once per application
or per subsystem that wants its own memory accounting.

Recycling is on by default — which is why every API that builds an engine asks
for `Resources` rather than for a memory provider alone — and
`with_pool_capacity` sets how many idle engines are retained, with zero
disabling it.

Which engines are recycled is deliberately *not* part of the contract. The pool
retains the state that is expensive enough to be worth retaining and rebuilds the
rest, so calling code never has to know which is which and the set can change
without any change to calling code. [IMPLEMENTATION.md](IMPLEMENTATION.md)
records the current set and why each entry is where it is.

`Resources` implements `ThreadAware`, and the two halves are treated
differently. The memory provider is relocated, because a NUMA-aware or
per-thread provider will want to allocate from the destination's memory from
then on. The pool is not: every clone shares one, an idle engine is a window and
a set of hash tables with no affinity to where it was built, and re-homing or
draining it on a move would discard the very state the type exists to retain —
on every move.

## 9. Errors

Failures are split by when they can happen. `BuildError` reports a configuration
an engine rejected at construction; `Error` reports everything that can go wrong
while streaming. Keeping them apart means a failure to build is not something a
caller has to consider on every chunk: once an engine exists, `BuildError` can no
longer occur. `BuildError` converts into `Error` for code that handles both in
one place.

Both implement `recoverable::Recovery`, so a caller can distinguish a failure
worth retrying from one that will recur.

## 10. The public surface is sealed

`Compression` names an engine — `impl Compression<Mode = Compress>` accepts any
compressor and no decompressor — and nothing more. How this crate drives one is
not public API.

Sealing that claim takes two things, because a supertrait alone is not enough.
The mechanics live on a crate-private supertrait that no downstream crate can
name, *and* `Compression` carries a `Sized` bound so no `dyn Compression` can be
formed: a trait object resolves supertrait methods as inherent candidates,
needing neither an import nor a nameable supertrait, so a vtable would have
handed those mechanics to every downstream crate. Boxing a concrete compressor
remains available and is how to hold one without naming its type.

## 11. Feature policy

Features are additive: enabling one never removes or changes behaviour available
without it, and enabling all of them at once is valid. No format is on by
default. A build that names no format still gets `Compression`, the builders,
`Resources` and the `format` types.

## Design tenets

- The caller's memory provider is where buffers come from; the crate does not
  reach for a global allocator behind their back.
- Nothing is flattened or copied that the underlying buffers can express
  directly.
- What is bounded is what the caller retains.
- Defaults are the safe reading of an ambiguous input, not the permissive one.
- Recycling is transparent: a correct program cannot tell whether an engine was
  reused.
- The uniform surface is uniform; format-specific settings are reached through
  format-specific builders rather than by widening the shared ones.

[`bytesbuf`]: https://docs.rs/bytesbuf
