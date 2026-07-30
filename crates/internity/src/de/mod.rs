// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Lexicon-aware deserialization (behind the `serde` feature).
//!
//! Serde's [`serde::Deserialize`] trait carries no context for choosing the
//! interner that owns a [`Sym`]. This module provides the context-carrying
//! counterpart:
//! [`DeserializeIn`] receives a [`Lexicon`] and produces each [`Sym`] by
//! interning the decoded string into a [`LocalLexicon`] or [`ThreadedLexicon`]
//! of your choosing.
//!
//! Most users derive [`DeserializeIn`](derive@DeserializeIn) on their structs
//! and call [`LocalLexicon::deserialize_in`](crate::LocalLexicon::deserialize_in) or
//! [`ThreadedLexicon::deserialize_in`](crate::ThreadedLexicon::deserialize_in).
//!
//! ```
//! use internity::LocalLexicon;
//! use internity::de::DeserializeIn;
//!
//! #[derive(DeserializeIn)]
//! struct Record {
//!     name: internity::Sym,
//!     count: u64,
//! }
//!
//! let mut lexicon = LocalLexicon::new();
//! let json = r#"{"name":"widget","count":3}"#;
//! let record: Record = lexicon
//!     .deserialize_in(&mut serde_json::Deserializer::from_str(json))
//!     .unwrap();
//! assert_eq!(lexicon.resolve(record.name), "widget");
//! assert_eq!(record.count, 3);
//! ```
//!
//! # Ordinary Serde and `DeserializeIn`
//!
//! The two traits are independent. **Scalars** (integers, `bool`, `char`,
//! `String`, and similar leaf types) implement [`DeserializeIn`] by delegating to
//! their ordinary [`serde::Deserialize`] implementation. **Containers** (`Vec`,
//! `Box`, `Option`, tuples, `BTreeMap`/`BTreeSet`, and the `std` hash
//! collections) do not delegate; they recursively apply [`DeserializeIn`] to
//! their elements so nested [`Sym`] fields are interned into the target lexicon.
//! Mixed structs therefore work out of the box, and a field whose type implements
//! only [`serde::Deserialize`] can opt out with `#[internity(via_serde)]`.
//!
//! [`Sym`]: crate::Sym
//! [`Lexicon`]: crate::Lexicon
//! [`LocalLexicon`]: crate::LocalLexicon
//! [`ThreadedLexicon`]: crate::ThreadedLexicon

mod deserialize_in;
mod impls;
mod inherent;
mod seed;

pub use deserialize_in::DeserializeIn;
pub(crate) use inherent::cautious_capacity;
/// Derive interner-aware deserialization for a struct.
///
/// See the [module documentation](self) for the supported shapes and the
/// `#[internity(...)]` attributes.
pub use internity_macros::DeserializeIn;
pub use seed::DeserializeInSeed;
#[doc(hidden)]
pub use seed::DeserializeSeed;

#[doc(hidden)]
pub mod __private {
    pub use serde;

    pub use crate::Lexicon;
}
