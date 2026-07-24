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

A blazingly fast string interning infrastructure.

String interning is a common technique to reduce memory use and improve
performance when code handles the same strings over and over
(identifiers in a compiler, tags/labels in telemetry, keys in a parser).
The benefits of interning include:

* Strings are stored once and reused which saves memory and CPU cycles

* Strings are referenced with a 4 byte handle instead of an 8 or 16 byte reference.
  This can save considerable memory.

* Hashing and comparison of interned strings is faster since it doesn’t require
  hashing or comparing whole strings, merely their 4 byte handle.

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

* [`Lexicon`][__link1]. This is a single-threaded engine: only one thread can be interning strings,
  although any number of threads can access the interned strings. This is the faster of the two
  engines.

* [`ThreadedLexicon`][__link2]. This engine allows multiple threads to be interning words concurrently.
  It’s naturally a bit slower due to the need for synchronization.

## The intern → freeze → read pattern

Interning and resolving have different needs, so the typical lifecycle is to
intern during a build phase, then [`freeze`][__link3] into a
[`Reader`][__link4] for the read phase. A `Reader` is immutable, `Send + Sync`, and its
lookups are lock-free — ideal for sharing across threads.

```rust
use internity::{Lexicon, Reader};

// Build phase.
let mut lexicon = Lexicon::new();
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
the [`BuildHasher`][__link5], like
`HashMap`. Use `with_hasher` to supply your own — for
example a DoS-resistant hasher when interning untrusted input.

## Production guidance

* A [`Sym`][__link6] is local to the interner that created it. A foreign handle is
  range-checked, but an in-range numeric value can resolve to an unrelated
  string. Persist or transmit handles together with the matching interner.
* The default Fx hasher is fast but not collision-attack resistant. Supply a
  defensive `BuildHasher` when strings can be selected by an attacker.
* Interners do not remove individual strings. Memory grows during the fill
  phase until the interner is dropped or frozen.
* A serialized `Sym` is only a raw integer. Deserialize it with the interner
  serialized from the same data, order, and compatible hasher.
* Exceeding the documented byte or handle limits panics. Applications that
  accept untrusted strings should enforce count and byte quotas before
  interning.

## Capacity

A single [`Lexicon`][__link7] holds up to approximately 4 GB of string bytes; a
[`ThreadedLexicon`][__link8] up to approximately 256 GB (across its shards). Either way
the number of distinct strings is bounded by the 4-byte handle (approximately
4.29 billion). Exceeding these limits panics rather than corrupting data.

## Cargo features

* `std` *(default)* — enables the concurrent [`ThreadedLexicon`][__link9]. Without it the
  crate is `no_std` + `alloc`: [`Lexicon`][__link10], [`Sym`][__link11], and [`Reader`][__link12] still work.
* `serde` — `Serialize`/`Deserialize` for [`Sym`][__link13] and the interners.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/internity">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbHYnwGoESLh0bRQ_YakYq70kbB2_Vm8ozl8MbCu2DdbUDq9lhZIGCaWludGVybml0eWUwLjEuMA
 [__link0]: https://docs.rs/internity/0.1.0/internity/?search=Sym
 [__link1]: https://docs.rs/internity/0.1.0/internity/?search=Lexicon
 [__link10]: https://docs.rs/internity/0.1.0/internity/?search=Lexicon
 [__link11]: https://docs.rs/internity/0.1.0/internity/?search=Sym
 [__link12]: https://docs.rs/internity/0.1.0/internity/?search=Reader
 [__link13]: https://docs.rs/internity/0.1.0/internity/?search=Sym
 [__link2]: https://docs.rs/internity/0.1.0/internity/?search=ThreadedLexicon
 [__link3]: https://docs.rs/internity/0.1.0/internity/?search=Lexicon::freeze
 [__link4]: https://docs.rs/internity/0.1.0/internity/?search=Reader
 [__link5]: https://doc.rust-lang.org/stable/core/?search=hash::BuildHasher
 [__link6]: https://docs.rs/internity/0.1.0/internity/?search=Sym
 [__link7]: https://docs.rs/internity/0.1.0/internity/?search=Lexicon
 [__link8]: https://docs.rs/internity/0.1.0/internity/?search=ThreadedLexicon
 [__link9]: https://docs.rs/internity/0.1.0/internity/?search=ThreadedLexicon
