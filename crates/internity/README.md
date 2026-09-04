<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Internity Logo" width="96">

# Internity

[![crate.io](https://img.shields.io/crates/v/internity.svg)](https://crates.io/crates/internity)
[![docs.rs](https://docs.rs/internity/badge.svg)](https://docs.rs/internity)
[![MSRV](https://img.shields.io/crates/msrv/internity)](https://crates.io/crates/internity)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Compact string interning with local and concurrent engines.

String interning is a common technique to reduce memory use and improve
performance when code handles the same strings over and over
(identifiers in a compiler, tags/labels in telemetry, keys in a parser).
The benefits of interning include:

* Strings are stored once and reused which saves memory and CPU cycles

* Strings are referenced with a 4-byte handle instead of an 8- or 16-byte reference.
  This can save considerable memory.

* Hashing and comparison of interned strings is faster since it doesn’t require
  hashing or comparing whole strings, merely their 4-byte handle.

To intern a string, you supply it to the interning engine and it hands back a handle.
No matter how many times you try to intern a given string, it gets deduplicated and
gets added only once to the data store, and you get back the same handle. Later, you can
use the handle to retrieve the actual string.

## Performance

See [`PERF.md`][__link0]
for wall-clock timings and memory footprint measured head-to-head against the
other main Rust interners, and
[`COMPARISON.md`][__link1]
for a design-level comparison of the Rust string-interning ecosystem.

## Handles

Interning yields a [`Sym`][__link2] — a 4-byte, `Copy` handle. It’s cheap to store and
pass, `Option<Sym>` is also 4 bytes, and within one interner
equal strings always produce equal handles, so `==` on handles is an O(1) stand
in for string equality and a `Sym` works directly as a `HashMap` key.

## Choosing an interner

`internity` supports two different string interners for different scenarios:

* [`LocalLexicon`][__link3]. This engine interns strings from one thread and avoids
  synchronization during the fill phase. Shared readers can resolve strings
  concurrently.

* [`ThreadedLexicon`][__link4]. This engine allows multiple threads to intern words
  concurrently and uses synchronization to coordinate inserts.

Both engines can be used through the [`Lexicon`][__link5] trait, allowing generic code
to intern strings without selecting a concrete engine.

## The intern → freeze → read pattern

Interning and resolving have different needs, so the typical lifecycle is to
intern during a build phase, then [`freeze`][__link6] into a
[`Reader`][__link7] for the read phase. A `Reader` is immutable, `Send + Sync`, and its
lookups are lock-free — ideal for sharing across threads.

Freezing returns a concrete, nameable type — [`LocalReader`][__link8] or
[`ThreadedReader`][__link9] — so a frozen table can be stored in a struct by value,
without boxing or virtual dispatch.

```rust
use internity::{LocalLexicon, Reader};

// Build phase.
let mut lexicon = LocalLexicon::new();
let hello = lexicon.intern("hello");
let world = lexicon.intern("world");
assert_eq!(lexicon.intern("hello"), hello); // deduplicated

// Read phase: freeze once, then resolve (here you could share `reader`
// across threads).
let reader = lexicon.freeze();
assert_eq!(reader.resolve(hello), "hello");
assert_eq!(reader.resolve(world), "world");
```

## Custom hashers

Both interners default to a fast, non-cryptographic hasher and are generic over
the [`BuildHasher`][__link10], like
`HashMap`. Use `with_hasher` to supply your own — for
example a DoS-resistant hasher when interning untrusted input.

The default is **not** collision-attack resistant, and this differs from
`lasso`, which defaults to a collision-resistant hasher. Migrating by swapping
the type therefore needs a deliberate hasher choice, or it silently loses that
protection. See [`LocalLexicon`][__link11] for the full warning.

## Dense handles

[`LocalLexicon`][__link12] assigns its handles consecutively in insertion order, and
[`freeze`][__link13] preserves that numbering, so every live string
has a distinct position in `0..len`. Use
[`index_of`][__link14] and [`sym_at`][__link15] to
move between a handle and its position, which makes a `Vec<T>` side table
indexed by symbol possible — cheaper than a hash map keyed by the handle.
The raw [`Sym::as_u32`][__link16] value is 1-based and must not be used as an index
directly.

[`ThreadedLexicon`][__link17] handles pack a shard index into their high bits and are
**not** consecutive, so it offers no such conversion.

## Production guidance

* A [`Sym`][__link18] is local to the interner that created it. A foreign handle is
  range-checked, but an in-range numeric value can resolve to an unrelated
  string. Persist or transmit handles together with the matching interner.
* The default Fx hasher is fast but not collision-attack resistant. Supply a
  defensive `BuildHasher` when strings can be selected by an attacker.
* Interners do not remove individual strings. Memory grows during the fill
  phase until the interner is dropped or frozen.
* Freezing drops the dedup hash map, which is where its memory saving comes
  from — so a frozen reader resolves handles but cannot look strings up by
  value. Keep the lexicon live if you need that, or rebuild a smaller index;
  see [`freeze`][__link19] for a worked example.
* A [`Sym`][__link20] does not implement [`serde::Serialize`][__link21]/`Deserialize` on its own:
  a bare handle is a meaningless integer without its interner. Serialize
  handles with the reader-aware [`se::SerializeIn`][__link22] derive (which resolves
  each handle to its string) and read them back with the [`de::DeserializeIn`][__link23]
  derive, so a value round-trips through a self-describing encoding. Serialize
  a whole corpus by freezing the interner and wrapping the [`Reader`][__link24] in
  [`se::SerializeReader`][__link25].
* Exceeding the documented byte or handle limits panics. Applications that
  accept untrusted strings should enforce count and byte quotas before
  interning.

## Capacity

A single [`LocalLexicon`][__link26] holds up to approximately 4 GiB of string bytes; a
[`ThreadedLexicon`][__link27] up to approximately 256 GiB (across its shards). Either way
the number of distinct strings is bounded by the 4-byte handle (approximately
4.29 billion). Exceeding these limits panics rather than corrupting data.

## Cargo features

* `std` *(default)* — enables the concurrent [`ThreadedLexicon`][__link28] and its frozen
  [`ThreadedReader`][__link29]. Without it the crate is `no_std` + `alloc`:
  [`LocalLexicon`][__link30], its frozen [`LocalReader`][__link31], [`Lexicon`][__link32], [`Sym`][__link33], and
  [`Reader`][__link34] still work.
* `serde` — reader-aware serialization: the [`se::SerializeIn`][__link35] /
  [`de::DeserializeIn`][__link36] derives, [`se::SerializeReader`][__link37] for a whole corpus,
  and `DeserializeIn` on the interners. [`ThreadedLexicon`][__link38] deserialization
  requires its default hasher so deserialization can reproduce identical
  handles.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/internity">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEG9dVcQv7gDzkG7VJ-FsdvgXwG4ndzbdWNuz6G6a5_GehYxcvYXKEG9vNV3arphdEG7I9TeizVDe4G-OG2mI4hylRGy5HBTEFC0bxYWSCgmlpbnRlcm5pdHllMC4yLjCCZXNlcmRlZzEuMC4yMjk
 [__link0]: https://github.com/microsoft/oxidizer/blob/main/crates/internity/docs/PERF.md
 [__link1]: https://github.com/microsoft/oxidizer/blob/main/crates/internity/docs/COMPARISON.md
 [__link10]: https://doc.rust-lang.org/stable/core/?search=hash::BuildHasher
 [__link11]: https://docs.rs/internity/0.2.0/internity/?search=LocalLexicon
 [__link12]: https://docs.rs/internity/0.2.0/internity/?search=LocalLexicon
 [__link13]: https://docs.rs/internity/0.2.0/internity/?search=LocalLexicon::freeze
 [__link14]: https://docs.rs/internity/0.2.0/internity/?search=LocalLexicon::index_of
 [__link15]: https://docs.rs/internity/0.2.0/internity/?search=LocalLexicon::sym_at
 [__link16]: https://docs.rs/internity/0.2.0/internity/?search=Sym::as_u32
 [__link17]: https://docs.rs/internity/0.2.0/internity/?search=ThreadedLexicon
 [__link18]: https://docs.rs/internity/0.2.0/internity/?search=Sym
 [__link19]: https://docs.rs/internity/0.2.0/internity/?search=LocalLexicon::freeze
 [__link2]: https://docs.rs/internity/0.2.0/internity/?search=Sym
 [__link20]: https://docs.rs/internity/0.2.0/internity/?search=Sym
 [__link21]: https://docs.rs/serde/1.0.229/serde/?search=Serialize
 [__link22]: https://docs.rs/internity/0.2.0/internity/?search=se::SerializeIn
 [__link23]: https://docs.rs/internity/0.2.0/internity/?search=de::DeserializeIn
 [__link24]: https://docs.rs/internity/0.2.0/internity/?search=Reader
 [__link25]: https://docs.rs/internity/0.2.0/internity/?search=se::SerializeReader
 [__link26]: https://docs.rs/internity/0.2.0/internity/?search=LocalLexicon
 [__link27]: https://docs.rs/internity/0.2.0/internity/?search=ThreadedLexicon
 [__link28]: https://docs.rs/internity/0.2.0/internity/?search=ThreadedLexicon
 [__link29]: https://docs.rs/internity/0.2.0/internity/?search=ThreadedReader
 [__link3]: https://docs.rs/internity/0.2.0/internity/?search=LocalLexicon
 [__link30]: https://docs.rs/internity/0.2.0/internity/?search=LocalLexicon
 [__link31]: https://docs.rs/internity/0.2.0/internity/?search=LocalReader
 [__link32]: https://docs.rs/internity/0.2.0/internity/?search=Lexicon
 [__link33]: https://docs.rs/internity/0.2.0/internity/?search=Sym
 [__link34]: https://docs.rs/internity/0.2.0/internity/?search=Reader
 [__link35]: https://docs.rs/internity/0.2.0/internity/?search=se::SerializeIn
 [__link36]: https://docs.rs/internity/0.2.0/internity/?search=de::DeserializeIn
 [__link37]: https://docs.rs/internity/0.2.0/internity/?search=se::SerializeReader
 [__link38]: https://docs.rs/internity/0.2.0/internity/?search=ThreadedLexicon
 [__link4]: https://docs.rs/internity/0.2.0/internity/?search=ThreadedLexicon
 [__link5]: https://docs.rs/internity/0.2.0/internity/?search=Lexicon
 [__link6]: https://docs.rs/internity/0.2.0/internity/?search=LocalLexicon::freeze
 [__link7]: https://docs.rs/internity/0.2.0/internity/?search=Reader
 [__link8]: https://docs.rs/internity/0.2.0/internity/?search=LocalReader
 [__link9]: https://docs.rs/internity/0.2.0/internity/?search=ThreadedReader
