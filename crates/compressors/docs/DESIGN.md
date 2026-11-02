# `compressors` design

This document records user-visible decisions that apply to several APIs.
The rustdoc describes behavior for individual items.
[IMPLEMENTATION.md](IMPLEMENTATION.md) describes the mechanisms that enforce these decisions.

## 1. Purpose

`compressors` streams compression and decompression over [`bytesbuf`] byte
sequences. It gives `flate2`, the `brotli` crate, and `zstd-safe` one contract.
Code that uses this contract works with each supported format.

The crate provides two main capabilities:

- Input stays segmented. The crate reads existing `BytesView` segments and
  writes into the uninitialized spare capacity of a `BytesBuf`. It does not
  flatten input or copy output through an intermediate buffer.
- The crate recycles engine state. For a small message, engine setup costs about
  as much as compression. `Resources` retains this state between messages.

## 2. Selecting a format

A caller selects a format in one of two ways. Both choices use the same engines.

At compile time, use the `gzip`, `deflate`, `zlib`, `brotli`, or `zstd` module.
Each module requires a Cargo feature with the same name. No format feature is
enabled by default.

At run time, use the `format` module. It resolves a `Content-Encoding` token to a
`Format` and provides the same API shape as each format module. This module is
available when no format feature is enabled.

The `deflate` feature and the HTTP `deflate` content coding are different:
`Format::Deflate` is raw DEFLATE (RFC 1951) and has no content-coding token,
but the HTTP `deflate` token denotes a zlib-wrapped stream (RFC 1950).
Therefore, the token resolves to `Format::Zlib`.

## 3. What is uniform, and what is not

`compress`, `decompress`, `Compressor`, and `Decompressor` have the same shape
in every format module. A caller can change formats by changing the import.

The builders are not uniform. `brotli` and `zstd` expose format-specific
settings for quality, window size, mode, and native compression level.
A shared builder cannot honor these settings. Zstd compressor and decompressor
builds return a `Result` because the native library validates their settings.

## 4. Accepting input

Per-format whole-buffer entry points take `impl InputData`. This sealed trait is
implemented for `BytesView`, `&[u8]`, and `&[u8; N]`. An existing `BytesView`
passes through unchanged. The entry point builds a view for a slice from its
`Resources`. The crate-level functions that accept an engine require a
`BytesView`. The trait is sealed so that this crate controls the accepted input
types and can add implementations without allowing downstream implementations.

## 5. Bounded work, at any size

An engine is a state machine, not a one-shot transform. A stream of any length
can pass through it. Pending output stays bounded by the configured chunk size.
Pending input, the engine window, and engine tables use additional memory.
Their size depends on the format and its configuration.

The crate provides two consumption models:

- `CompressionStream`, behind the `futures-stream` feature, yields one bounded
  chunk at a time. A consumer stays bounded if it processes and drops each
  chunk. The underlying push/pull API is private.
- The whole-buffer functions accumulate the complete result. These functions
  require cumulative bounds for decompression.

## 6. Bounding untrusted decompression

[SECURITY.md](SECURITY.md) describes the complete threat model.

Every supported format can expand its input by orders of magnitude, so a
decompressor pointed at untrusted data is a memory-exhaustion vector.

Retained output is the boundary, not throughput. A cumulative output cap
protects a caller that accumulates output. The same cap rejects valid long
streams in a pipeline that does not retain output.

`DecompressorLimits` carries three independent bounds: a ratio, a total output
length, and a count of concatenated streams.

- A ratio bound is a coarse backstop, not protection. Only the deflate family has
  a structural expansion ceiling. Brotli and zstd do not, so for them a ratio
  that accepts valid data can also accept a decompression bomb. Brotli declares
  no ratio default. Zstd uses a loose 250,000x ratio as a coarse backstop.
- Whole-buffer functions add output and stream caps for bounds that the caller
  did not set.

The same decompressor can stream without cumulative bounds, but it cannot safely
buffer unbounded output. Each `pull` states whether the destination streams or
buffers output. For a buffering destination, the decompressor adds shared
fallbacks for unset bounds: 64 MiB of output and 1024 concatenated streams.
The tighter configured or fallback bound applies. For a streaming destination,
only configured bounds apply.

The buffering fallbacks travel together in one value. The output and stream
bounds cover different attacks. Many tiny members each require a full engine
setup but produce almost no output. An output ceiling does not stop this case.

The engine applies the bounds. It narrows each output slice to the remaining
budget. Caller-visible output stops at the bound. The engine can produce one
discarded probe byte to distinguish an exact-bound stream from an over-limit
stream. This design also keeps the implementation consistent for all callers.

## 7. Stream framing

Each format defines whether bytes after a complete stream start another stream.
Gzip and zstd define concatenation. The other formats do not.

When concatenation is disabled, `TrailingData` controls these bytes. Its default
is `Reject`. Silent discard can create a parser differential. A proxy can inspect
only the prefix while a downstream component reads the suffix. Use `Ignore` for
a container that places other data after the compressed stream.

Decompression can also yield bytes before a checksum or trailer has rejected the
stream, so output is provisional until the decompressor reports that it is done.

## 8. Resources and recycling

`Resources` supplies a memory provider and a pool of idle engine state.
A builder describes engine behavior. Keep one `Resources` value per application,
or per subsystem that needs separate memory accounting.

Recycling is enabled by default. Each engine-building API therefore takes
`Resources` instead of only a memory provider. `with_pool_capacity` sets the
number of retained idle engines. A value of zero disables recycling.

The public contract does not specify which engines are recycled. The pool keeps
state when reuse is worth its memory cost. It rebuilds other state.
[IMPLEMENTATION.md](IMPLEMENTATION.md) records the current choices.

`Resources` implements `ThreadAware`. It relocates the memory provider because a
NUMA-aware or per-thread provider can use memory near the destination thread.
It does not relocate the shared pool. Idle engine state has no thread affinity.
Relocating the pool discards the state that the pool retains.

## 9. Errors

Failures are separated by when they occur. `BuildError` reports a configuration
that an engine rejects during construction. `Error` reports failures during
streaming. After construction, an engine cannot return `BuildError`.
`BuildError` converts into `Error`.

Both implement `recoverable::Recovery`, so a caller can distinguish a failure
worth retrying from one that will recur.

## 10. The public surface is sealed

`Compression` names an engine. `impl Compression<Mode = Compress>` accepts any
compressor but no decompressor. The driving methods are not public API.

The driving methods live on a crate-private supertrait. `Compression` also has a
`Sized` bound, so callers cannot create `dyn Compression`. A trait object can
resolve supertrait methods without naming the supertrait. Without `Sized`, its
vtable exposes the private driving methods. Callers can still box a
concrete compressor.

## 11. Feature policy

Features are additive. Enabling one never removes or changes behavior available
without it, and enabling all of them at once is valid. No format is on by
default. A build that names no format still gets `Compression`, the builders,
`Resources` and the `format` types.

## Design tenets

- Buffers come from the caller's memory provider. The crate does not select a
  global allocator instead.
- Nothing is flattened or copied that the underlying buffers can express
  directly.
- What is bounded is what the caller retains.
- Defaults use the safe interpretation of ambiguous input.
- Recycling is transparent. Correct behavior does not depend on engine reuse.
- Format-specific builders provide format-specific settings. The shared builders
  contain only settings that apply to every format.

[`bytesbuf`]: https://docs.rs/bytesbuf
