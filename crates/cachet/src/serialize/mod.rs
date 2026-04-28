// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serialization codecs for converting typed values to/from bytes.
//!
//! Values are serialized using [postcard] with a format version byte prefix,
//! producing [`BytesView`] outputs backed by thread-local memory pools.
//!
//! When a cached value cannot be decoded (e.g., format version mismatch or
//! corrupt bytes), the codec returns `Ok(None)` to treat it as a cache miss
//! rather than a hard error.
//!
//! [postcard]: https://docs.rs/postcard
//! [`BytesView`]: bytesbuf::BytesView

pub(crate) mod codec;
