# Changelog

## [0.1.1] - 2026-08-13

### Added

- `LocalLexicon::with_capacity(strings, bytes)` and
  `LocalLexicon::with_capacity_and_hasher(strings, bytes, hasher)` — preallocate the
  dedup table, offset index, and string buffer to avoid reallocation while filling.

### Changed

- `LocalLexicon` live resolution (`resolve` / `try_resolve` / `iter`) is now ~1.6× faster
  (on par with the frozen reader and the fastest competitors): it reconstructs
  strings from the byte buffer without the UTF-8 char-boundary checks that slicing a
  `&str` incurs. All unchecked-UTF-8 conversion now lives in one `storage` module —
  every other module, including the frozen readers, is `#![forbid(unsafe_code)]`.

- ✨ Features

  - export frozen readers and guarantee dense handles ([#657](https://github.com/microsoft/oxidizer/657))

All notable changes to this crate are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0]

Initial release.

### Added

- `LocalLexicon` — a fast, single-threaded string interner (`&mut self` interning,
  live `&self` resolution).
- `ThreadedLexicon` — a concurrent string interner: `&self` interning from many
  threads, a cheap `Clone` `Arc`-backed handle, fill-then-freeze.
- `Sym` — a 4-byte, `Copy`, niche-optimized handle; `as_u32`/`from_u32` and
  `From<Sym> for u32` conversions; `iter()` over `(Sym, &str)`.
- `Reader` — a sealed trait for the frozen, `Send + Sync`, lock-free read form,
  produced by `LocalLexicon::freeze` / `ThreadedLexicon::freeze`; supports `iter()`.
- `SymMap` / `SymSet` / `SymBuildHasher` — fast `Sym`-keyed maps and sets via an
  identity-style hasher.
- `Extend` and `FromIterator` for both interners.
- Generic over the `BuildHasher`, defaulting to a fast non-cryptographic hasher.
- `no_std` support: the crate is `no_std` + `alloc` without the default `std`
  feature (which enables `ThreadedLexicon`).
- Optional `serde` feature: a string-based, interner-aware serialization model.
  `#[derive(SerializeIn)]` / `#[derive(DeserializeIn)]` resolve each `Sym`
  against a `Reader` / interner so values round-trip **as their strings**
  (serialize a `Sym` to its string, deserialize a string back into a `Sym`),
  making corpora portable across processes. `SerializeReader` snapshots a whole
  frozen corpus. `Sym` itself has no plain `Serialize`/`Deserialize` (a raw
  handle is meaningless outside its lexicon), and the live `ThreadedLexicon` is
  deserialize-only — serialize the frozen `Reader` instead.
