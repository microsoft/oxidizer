// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compact string interning with local and concurrent engines.
//!
//! String interning is a common technique to reduce memory use and improve
//! performance when code handles the same strings over and over
//! (identifiers in a compiler, tags/labels in telemetry, keys in a parser).
//! The benefits of interning include:
//!
//! * Strings are stored once and reused which saves memory and CPU cycles
//!
//! * Strings are referenced with a 4-byte handle instead of an 8- or 16-byte reference.
//!   This can save considerable memory.
//!
//! * Hashing and comparison of interned strings is faster since it doesn't require
//!   hashing or comparing whole strings, merely their 4-byte handle.
//!
//! To intern a string, you supply it to the interning engine and it hands back a handle.
//! No matter how many times you try to intern a given string, it gets deduplicated and
//! gets added only once to the data store, and you get back the same handle. Later, you can
//! use the handle to retrieve the actual string.
//!
//! # Performance
//!
//! See [`PERF.md`](https://github.com/microsoft/oxidizer/blob/main/crates/internity/docs/PERF.md)
//! for wall-clock timings and memory footprint measured head-to-head against the
//! other main Rust interners, and
//! [`COMPARISON.md`](https://github.com/microsoft/oxidizer/blob/main/crates/internity/docs/COMPARISON.md)
//! for a design-level comparison of the Rust string-interning ecosystem.
//!
//! # Handles
//!
//! Interning yields a [`Sym`] — a 4-byte, `Copy` handle. It's cheap to store and
//! pass, `Option<Sym>` is also 4 bytes, and within one interner
//! equal strings always produce equal handles, so `==` on handles is an O(1) stand
//! in for string equality and a `Sym` works directly as a `HashMap` key.
//!
//! # Choosing an interner
//!
//! `internity` supports two different string interners for different scenarios:
//!
//! * [`LocalLexicon`]. This engine interns strings from one thread and avoids
//!   synchronization during the fill phase. Shared readers can resolve strings
//!   concurrently.
//!
//! * [`ThreadedLexicon`]. This engine allows multiple threads to intern words
//!   concurrently and uses synchronization to coordinate inserts.
//!
//! Both engines can be used through the [`Lexicon`] trait, allowing generic code
//! to intern strings without selecting a concrete engine.
//!
//! # The intern → freeze → read pattern
//!
//! Interning and resolving have different needs, so the typical lifecycle is to
//! intern during a build phase, then [`freeze`](LocalLexicon::freeze) into a
//! [`Reader`] for the read phase. A `Reader` is immutable, `Send + Sync`, and its
//! lookups are lock-free — ideal for sharing across threads.
//!
//! Freezing returns a concrete, nameable type — [`LocalReader`] or
//! [`ThreadedReader`] — so a frozen table can be stored in a struct by value,
//! without boxing or virtual dispatch.
//!
//! ```
//! use internity::{LocalLexicon, Reader};
//!
//! // Build phase.
//! let mut lexicon = LocalLexicon::new();
//! let hello = lexicon.intern("hello");
//! let world = lexicon.intern("world");
//! assert_eq!(lexicon.intern("hello"), hello); // deduplicated
//!
//! // Read phase: freeze once, then resolve (here you could share `reader`
//! // across threads).
//! let reader = lexicon.freeze();
//! assert_eq!(reader.resolve(hello), "hello");
//! assert_eq!(reader.resolve(world), "world");
//! ```
//!
//! # Custom hashers
//!
//! Both interners default to a fast, non-cryptographic hasher and are generic over
//! the [`BuildHasher`](core::hash::BuildHasher), like
//! `HashMap`. Use `with_hasher` to supply your own — for
//! example a DoS-resistant hasher when interning untrusted input.
//!
//! The default is **not** collision-attack resistant, and this differs from
//! `lasso`, which defaults to a collision-resistant hasher. Migrating by swapping
//! the type therefore needs a deliberate hasher choice, or it silently loses that
//! protection. See [`LocalLexicon`] for the full warning.
//!
//! # Dense handles
//!
//! [`LocalLexicon`] assigns its handles consecutively in insertion order, and
//! [`freeze`](LocalLexicon::freeze) preserves that numbering, so every live string
//! has a distinct position in `0..len`. Use
//! [`index_of`](LocalLexicon::index_of) and [`sym_at`](LocalLexicon::sym_at) to
//! move between a handle and its position, which makes a `Vec<T>` side table
//! indexed by symbol possible — cheaper than a hash map keyed by the handle.
//! The raw [`Sym::as_u32`] value is 1-based and must not be used as an index
//! directly.
//!
//! [`ThreadedLexicon`] handles pack a shard index into their high bits and are
//! **not** consecutive, so it offers no such conversion.
//!
//! # Production guidance
//!
//! * A [`Sym`] is local to the interner that created it. A foreign handle is
//!   range-checked, but an in-range numeric value can resolve to an unrelated
//!   string. Persist or transmit handles together with the matching interner.
//! * The default Fx hasher is fast but not collision-attack resistant. Supply a
//!   defensive `BuildHasher` when strings can be selected by an attacker.
//! * Interners do not remove individual strings. Memory grows during the fill
//!   phase until the interner is dropped or frozen.
//! * Freezing drops the dedup hash map, which is where its memory saving comes
//!   from — so a frozen reader resolves handles but cannot look strings up by
//!   value. Keep the lexicon live if you need that, or rebuild a smaller index;
//!   see [`freeze`](LocalLexicon::freeze) for a worked example.
//! * A [`Sym`] does not implement [`serde::Serialize`]/`Deserialize` on its own:
//!   a bare handle is a meaningless integer without its interner. Serialize
//!   handles with the reader-aware [`se::SerializeIn`] derive (which resolves
//!   each handle to its string) and read them back with the [`de::DeserializeIn`]
//!   derive, so a value round-trips through a self-describing encoding. Serialize
//!   a whole corpus by freezing the interner and wrapping the [`Reader`] in
//!   [`se::SerializeReader`].
//! * Exceeding the documented byte or handle limits panics. Applications that
//!   accept untrusted strings should enforce count and byte quotas before
//!   interning.
//!
//! # Capacity
//!
//! A single [`LocalLexicon`] holds up to approximately 4 GiB of string bytes; a
//! [`ThreadedLexicon`] up to approximately 256 GiB (across its shards). Either way
//! the number of distinct strings is bounded by the 4-byte handle (approximately
//! 4.29 billion). Exceeding these limits panics rather than corrupting data.
//!
//! # Cargo features
//!
//! * `std` *(default)* — enables the concurrent [`ThreadedLexicon`] and its frozen
//!   [`ThreadedReader`]. Without it the crate is `no_std` + `alloc`:
//!   [`LocalLexicon`], its frozen [`LocalReader`], [`Lexicon`], [`Sym`], and
//!   [`Reader`] still work.
//! * `serde` — reader-aware serialization: the [`se::SerializeIn`] /
//!   [`de::DeserializeIn`] derives, [`se::SerializeReader`] for a whole corpus,
//!   and `DeserializeIn` on the interners. [`ThreadedLexicon`] deserialization
//!   requires its default hasher so deserialization can reproduce identical
//!   handles.

#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(
    not(feature = "std"),
    expect(rustdoc::broken_intra_doc_links, reason = "documentation links to the std-only ThreadedLexicon")
)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/internity/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/internity/favicon.ico")]
#![warn(missing_docs)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

#[forbid(unsafe_code)]
mod lexicon;
#[forbid(unsafe_code)]
mod local_lexicon;
#[forbid(unsafe_code)]
mod reader;
#[forbid(unsafe_code)]
mod sym;
#[forbid(unsafe_code)]
mod symbol_map;

#[cfg(feature = "std")]
#[forbid(unsafe_code)]
mod shard;
#[cfg(feature = "std")]
#[forbid(unsafe_code)]
mod shard_write;
#[cfg(feature = "std")]
#[forbid(unsafe_code)]
mod threaded_lexicon;
#[cfg(feature = "std")]
#[forbid(unsafe_code)]
mod threaded_reader;

// Unchecked UTF-8 reconstruction is isolated in `storage`; reader modules forbid
// unsafe code.
#[forbid(unsafe_code)]
mod local_reader;
#[cfg(feature = "std")]
#[forbid(unsafe_code)]
mod shard_reader;

mod storage;

#[cfg(feature = "serde")]
#[forbid(unsafe_code)]
mod serde_impls;

#[cfg(feature = "serde")]
#[forbid(unsafe_code)]
pub mod de;

#[cfg(feature = "serde")]
#[forbid(unsafe_code)]
pub mod se;

pub use lexicon::Lexicon;
pub use local_lexicon::LocalLexicon;
pub use local_reader::LocalReader;
pub use reader::Reader;
pub use sym::Sym;
pub use symbol_map::{SymBuildHasher, SymHasher};
#[cfg(feature = "std")]
pub use symbol_map::{SymMap, SymSet};
#[cfg(feature = "std")]
pub use threaded_lexicon::ThreadedLexicon;
#[cfg(feature = "std")]
pub use threaded_reader::ThreadedReader;
