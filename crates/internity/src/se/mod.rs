// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reader-aware serialization (behind the `serde` feature).
//!
//! Serde's [`serde::Serialize`] trait carries no context for resolving a [`Sym`]
//! against its interner. This module provides the context-carrying counterpart:
//! [`SerializeIn`] receives a [`Reader`] and emits each [`Sym`] as the string it
//! resolves to. Paired with [`DeserializeIn`](crate::de::DeserializeIn), a value
//! round-trips through a self-describing encoding without shipping the interner
//! separately.
//!
//! Most users derive [`SerializeIn`](derive@SerializeIn) on their structs and
//! wrap the value in [`SerializeInWith`] to hand it to a Serde entry point.
//!
//! ```
//! use internity::de::DeserializeIn;
//! use internity::se::{SerializeIn, SerializeInWith};
//! use internity::{LocalLexicon, Reader};
//!
//! #[derive(SerializeIn, DeserializeIn)]
//! struct Record {
//!     name: internity::Sym,
//!     count: u64,
//! }
//!
//! // Serialize with a reader: the `Sym` becomes its string.
//! let mut lexicon = LocalLexicon::new();
//! let record = Record {
//!     name: lexicon.intern("widget"),
//!     count: 3,
//! };
//! let reader = lexicon.freeze();
//! let json = serde_json::to_string(&SerializeInWith::new(&record, &reader)).unwrap();
//! assert_eq!(json, r#"{"name":"widget","count":3}"#);
//!
//! // Deserialize into a fresh interner: the same handle comes back.
//! let mut restored = LocalLexicon::new();
//! let back: Record = restored
//!     .deserialize_in(&mut serde_json::Deserializer::from_str(&json))
//!     .unwrap();
//! assert_eq!(restored.resolve(back.name), "widget");
//! ```
//!
//! # Ordinary Serde and `SerializeIn`
//!
//! The two traits are independent. **Scalars** (integers, `bool`, `char`,
//! `String`, and similar leaf types) implement [`SerializeIn`] by delegating to
//! their ordinary [`serde::Serialize`] implementation. **Containers** (`Vec`,
//! `Box`, `Option`, tuples, `BTreeMap`/`BTreeSet`, and the `std` hash
//! collections) do not delegate; they recursively apply [`SerializeIn`] to their
//! elements so nested [`Sym`] fields are resolved against the reader. Mixed
//! structs therefore work out of the box, and a field whose type implements only
//! [`serde::Serialize`] can opt out with `#[internity(via_serde)]`.
//!
//! # Serializing the corpus
//!
//! To serialize the strings of a whole interner, freeze it into a [`Reader`] and wrap
//! it in [`SerializeReader`]. Serializing a [`Reader`] (rather than a live
//! [`ThreadedLexicon`](crate::ThreadedLexicon)) makes the point-in-time snapshot
//! explicit and avoids serializing an interner while other threads mutate it.
//!
//! [`Sym`]: crate::Sym
//! [`Reader`]: crate::Reader

mod impls;
mod seed;
mod serialize_in;
mod serialize_reader;

/// Derive reader-aware serialization for a struct.
///
/// See the [module documentation](self) for the supported shapes and the
/// `#[internity(...)]` attributes.
pub use internity_macros::SerializeIn;

pub use self::seed::SerializeInWith;
pub use self::serialize_in::SerializeIn;
pub use self::serialize_reader::SerializeReader;

#[doc(hidden)]
pub mod __private {
    pub use serde;

    pub use super::{SerializeIn, SerializeInWith};
    pub use crate::Reader;
}
