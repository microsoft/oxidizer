// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Key and value transforms for cache tiers.
//!
//! Transforms allow a cache to store data in a different representation than
//! the types exposed to callers. A common example is serialization: the caller
//! works with typed values while the backing tier stores raw bytes.
//!
//! # Core Traits
//!
//! | Trait | Direction | Use |
//! |-------|-----------|-----|
//! | [`Encoder<A, B>`] | `A → B` | One-way key encoding |
//! | [`Codec<A, B>`] | `A ↔ B` | Bidirectional value encoding/decoding |
//!
//! # Decode Outcomes
//!
//! [`Codec::decode`] returns [`DecodeOutcome<T>`] rather than a plain value:
//!
//! - [`DecodeOutcome::Value(v)`](DecodeOutcome::Value) - decoded successfully.
//! - [`DecodeOutcome::SoftFailure(reason)`](DecodeOutcome::SoftFailure) - the stored data is
//!   undecodable (e.g., format version mismatch, corrupt bytes) and should be treated as a
//!   cache miss rather than a hard error.
//!
//! # Implementations
//!
//! | Type | Description |
//! |------|-------------|
//! | [`TransformEncoder`] | Wraps a closure as an [`Encoder`]. |
//! | [`TransformCodec`] | Wraps a pair of closures as a [`Codec`]. Decode always returns [`DecodeOutcome::Value`] — closures that need soft-failure semantics should implement [`Codec`] directly. |
//!
//! # Helpers
//!
//! [`infallible`] and [`infallible_owned`] wrap closures that cannot fail so
//! they can be used where a fallible closure is expected.

mod codec;
#[cfg(any(feature = "test-util", test))]
pub mod testing;
mod tier;

pub use codec::{Codec, DecodeOutcome, Encoder, TransformCodec, TransformEncoder, infallible, infallible_owned};
pub(crate) use tier::TransformAdapter;
