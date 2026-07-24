// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A blazingly fast string interning infrastructure.
//!
//! String interning is a common technique to reduce memory use and improve
//! performance when code handles the same strings over and over
//! (identifiers in a compiler, tags/labels in telemetry, keys in a parser).
//! The benefits of interning include:
//!
//! * Strings are stored once and reused which saves memory and CPU cycles
//!
//! * Strings are referenced with a 4 byte handle instead of an 8 or 16 byte reference.
//!   This can save considerable memory.
//!
//! * Hashing and comparison of interned strings is faster since it doesn't require
//!   hashing or comparing whole strings, merely their 4 byte handle.
//!
//! To intern a string, you supply it to the interning engine and it hands back a handle.
//! No matter how many times you try to intern a given string, it gets deduplicated and
//! gets added only once to the data store, and you get back the same handle. Later, you can
//! use the handle to retrieve the actual string.
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
//! * [`Lexicon`]. This is a single-threaded engine: only one thread can be interning strings,
//!   although any number of threads can access the interned strings. This is the faster of the two
//!   engines.
//!
//! * [`ThreadedLexicon`]. This engine allows multiple threads to be interning words concurrently.
//!   It's naturally a bit slower due to the need for synchronization.
//!
//! # The intern → freeze → read pattern
//!
//! Interning and resolving have different needs, so the typical lifecycle is to
//! intern during a build phase, then [`freeze`](Lexicon::freeze) into a
//! [`Reader`] for the read phase. A `Reader` is immutable, `Send + Sync`, and its
//! lookups are lock-free — ideal for sharing across threads.
//!
//! ```
//! use internity::{Lexicon, Reader};
//!
//! // Build phase.
//! let mut lexicon = Lexicon::new();
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
//! # Production guidance
//!
//! * A [`Sym`] is local to the interner that created it. A foreign handle is
//!   range-checked, but an in-range numeric value can resolve to an unrelated
//!   string. Persist or transmit handles together with the matching interner.
//! * The default Fx hasher is fast but not collision-attack resistant. Supply a
//!   defensive `BuildHasher` when strings can be selected by an attacker.
//! * Interners do not remove individual strings. Memory grows during the fill
//!   phase until the interner is dropped or frozen.
//! * A serialized `Sym` is only a raw integer. Deserialize it with the interner
//!   serialized from the same data, order, and compatible hasher.
//! * Exceeding the documented byte or handle limits panics. Applications that
//!   accept untrusted strings should enforce count and byte quotas before
//!   interning.
//!
//! # Capacity
//!
//! A single [`Lexicon`] holds up to approximately 4 GB of string bytes; a
//! [`ThreadedLexicon`] up to approximately 256 GB (across its shards). Either way
//! the number of distinct strings is bounded by the 4-byte handle (approximately
//! 4.29 billion). Exceeding these limits panics rather than corrupting data.
//!
//! # Cargo features
//!
//! * `std` *(default)* — enables the concurrent [`ThreadedLexicon`]. Without it the
//!   crate is `no_std` + `alloc`: [`Lexicon`], [`Sym`], and [`Reader`] still work.
//! * `serde` — `Serialize`/`Deserialize` for [`Sym`] and the interners.

#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/internity/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/internity/favicon.ico")]
#![warn(missing_docs)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

#[forbid(unsafe_code)]
mod lexicon;
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
mod sharded_reader;
#[cfg(feature = "std")]
#[forbid(unsafe_code)]
mod threaded_lexicon;

// Frozen-reader modules — now `unsafe`-free: all unchecked UTF-8 lives in
// `storage`, the crate's single deliberate exception to the no-`unsafe` rule.
#[forbid(unsafe_code)]
mod flat_reader;
#[cfg(feature = "std")]
#[forbid(unsafe_code)]
mod shard_reader;

mod storage;

#[cfg(feature = "serde")]
#[forbid(unsafe_code)]
mod serde_impls;

pub use lexicon::Lexicon;
pub use reader::Reader;
pub use sym::Sym;
pub use symbol_map::{SymBuildHasher, SymHasher};
#[cfg(feature = "std")]
pub use symbol_map::{SymMap, SymSet};
#[cfg(feature = "std")]
pub use threaded_lexicon::ThreadedLexicon;
