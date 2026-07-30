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

## Handles

Interning yields a [`Sym`][__link0] — a 4-byte, `Copy` handle. It’s cheap to store and
pass, `Option<Sym>` is also 4 bytes, and within one interner
equal strings always produce equal handles, so `==` on handles is an O(1) stand
in for string equality and a `Sym` works directly as a `HashMap` key.

## Choosing an interner

`internity` supports two different string interners for different scenarios:

* [`LocalLexicon`][__link1]. This engine interns strings from one thread and avoids
  synchronization during the fill phase. Shared readers can resolve strings
  concurrently.

* [`ThreadedLexicon`][__link2]. This engine allows multiple threads to intern words
  concurrently and uses synchronization to coordinate inserts.

Both engines can be used through the [`Lexicon`][__link3] trait, allowing generic code
to intern strings without selecting a concrete engine.

## The intern → freeze → read pattern

Interning and resolving have different needs, so the typical lifecycle is to
intern during a build phase, then [`freeze`][__link4] into a
[`Reader`][__link5] for the read phase. A `Reader` is immutable, `Send + Sync`, and its
lookups are lock-free — ideal for sharing across threads.

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
the [`BuildHasher`][__link6], like
`HashMap`. Use `with_hasher` to supply your own — for
example a DoS-resistant hasher when interning untrusted input.

## Production guidance

* A [`Sym`][__link7] is local to the interner that created it. A foreign handle is
  range-checked, but an in-range numeric value can resolve to an unrelated
  string. Persist or transmit handles together with the matching interner.
* The default Fx hasher is fast but not collision-attack resistant. Supply a
  defensive `BuildHasher` when strings can be selected by an attacker.
* Interners do not remove individual strings. Memory grows during the fill
  phase until the interner is dropped or frozen.
* A [`Sym`][__link8] does not implement [`serde::Serialize`][__link9]/`Deserialize` on its own:
  a bare handle is a meaningless integer without its interner. Serialize
  handles with the reader-aware [`se::SerializeIn`][__link10] derive (which resolves
  each handle to its string) and read them back with the [`de::DeserializeIn`][__link11]
  derive, so a value round-trips through a self-describing encoding. Serialize
  a whole corpus by freezing the interner and wrapping the [`Reader`][__link12] in
  [`se::SerializeReader`][__link13].
* Exceeding the documented byte or handle limits panics. Applications that
  accept untrusted strings should enforce count and byte quotas before
  interning.

## Capacity

A single [`LocalLexicon`][__link14] holds up to approximately 4 GiB of string bytes; a
[`ThreadedLexicon`][__link15] up to approximately 256 GiB (across its shards). Either way
the number of distinct strings is bounded by the 4-byte handle (approximately
4.29 billion). Exceeding these limits panics rather than corrupting data.

## Cargo features

* `std` *(default)* — enables the concurrent [`ThreadedLexicon`][__link16]. Without it the
  crate is `no_std` + `alloc`: [`LocalLexicon`][__link17], [`Lexicon`][__link18], [`Sym`][__link19], and
  [`Reader`][__link20] still work.
* `serde` — reader-aware serialization: the [`se::SerializeIn`][__link21] /
  [`de::DeserializeIn`][__link22] derives, [`se::SerializeReader`][__link23] for a whole corpus,
  and `DeserializeIn` on the interners. [`ThreadedLexicon`][__link24] deserialization
  requires its default hasher so deserialization can reproduce identical
  handles.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/internity">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQb2G7EdP2hZtcblfzsjF5fRG4bjHbu2fkr7Ysb3M2rIQy6E39hZIKCaWludGVybml0eWUwLjEuMIJlc2VyZGVnMS4wLjIyOA
 [__link0]: https://docs.rs/internity/0.1.0/internity/?search=Sym
 [__link1]: https://docs.rs/internity/0.1.0/internity/?search=LocalLexicon
 [__link10]: https://docs.rs/internity/0.1.0/internity/?search=se::SerializeIn
 [__link11]: https://docs.rs/internity/0.1.0/internity/?search=de::DeserializeIn
 [__link12]: https://docs.rs/internity/0.1.0/internity/?search=Reader
 [__link13]: https://docs.rs/internity/0.1.0/internity/?search=se::SerializeReader
 [__link14]: https://docs.rs/internity/0.1.0/internity/?search=LocalLexicon
 [__link15]: https://docs.rs/internity/0.1.0/internity/?search=ThreadedLexicon
 [__link16]: https://docs.rs/internity/0.1.0/internity/?search=ThreadedLexicon
 [__link17]: https://docs.rs/internity/0.1.0/internity/?search=LocalLexicon
 [__link18]: https://docs.rs/internity/0.1.0/internity/?search=Lexicon
 [__link19]: https://docs.rs/internity/0.1.0/internity/?search=Sym
 [__link2]: https://docs.rs/internity/0.1.0/internity/?search=ThreadedLexicon
 [__link20]: https://docs.rs/internity/0.1.0/internity/?search=Reader
 [__link21]: https://docs.rs/internity/0.1.0/internity/?search=se::SerializeIn
 [__link22]: https://docs.rs/internity/0.1.0/internity/?search=de::DeserializeIn
 [__link23]: https://docs.rs/internity/0.1.0/internity/?search=se::SerializeReader
 [__link24]: https://docs.rs/internity/0.1.0/internity/?search=ThreadedLexicon
 [__link3]: https://docs.rs/internity/0.1.0/internity/?search=Lexicon
 [__link4]: https://docs.rs/internity/0.1.0/internity/?search=LocalLexicon::freeze
 [__link5]: https://docs.rs/internity/0.1.0/internity/?search=Reader
 [__link6]: https://doc.rust-lang.org/stable/core/?search=hash::BuildHasher
 [__link7]: https://docs.rs/internity/0.1.0/internity/?search=Sym
 [__link8]: https://docs.rs/internity/0.1.0/internity/?search=Sym
 [__link9]: https://docs.rs/serde/1.0.228/serde/?search=Serialize
