// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serialization codecs for converting typed values to/from bytes via serde.
//!
//! These codecs serialize values into [`BytesView`] using pool-backed memory,
//! avoiding heap allocations. The serialized bytes can then flow through
//! compression, encryption, and network I/O without additional copies.

use std::borrow::Cow;

use bytesbuf::mem::GlobalPool;
use bytesbuf::{BytesBuf, BytesView};

use crate::telemetry::CacheTelemetry;
use crate::{Codec, Encoder, Error};

use serde::{Serialize, de::DeserializeOwned};

/// An encoder that serializes values to [`BytesView`] using postcard (one-directional).
///
/// Implements `Encoder<T, BytesView>` for any `T: Serialize + Send + Sync`.
///
/// For bidirectional serialization/deserialization, use [`PostcardCodec`].
#[derive(Debug, Clone)]
pub struct PostcardEncoder {}

impl PostcardEncoder {
    /// Creates a new `PostcardEncoder` with the given telemetry.
    pub fn new() -> Self {
        Self {}
    }
}

/// A bidirectional codec that serializes and deserializes values using postcard.
///
/// Implements `Codec<T, BytesView>` for any `T: Serialize + DeserializeOwned + Send + Sync`.
///
/// Serialization writes directly into pool-backed memory via [`BytesBufWriter`](bytesbuf::BytesBufWriter),
/// producing a [`BytesView`] with no intermediate heap allocation.
#[derive(Debug, Clone)]
pub struct PostcardCodec {}

impl PostcardCodec {
    /// Creates a new `PostcardCodec` with the given telemetry.
    pub fn new() -> Self {
        Self {}
    }
}

impl<T: Serialize + Send + Sync> Encoder<T, BytesView> for PostcardEncoder {
    fn encode(&self, value: &T) -> Result<BytesView, Error> {
        encode(value)
    }
}

impl<T: Serialize + Send + Sync> Encoder<T, BytesView> for PostcardCodec {
    fn encode(&self, value: &T) -> Result<BytesView, Error> {
        encode(value)
    }
}

fn encode<T: Serialize + Send + Sync>(value: &T) -> Result<BytesView, Error> {
    // TODO make Cache thread aware so we can simply store the pool in the encoder
    // Until then we need this to avoid creating a new pool for every encode call, which would be
    // very expensive
    thread_local! {
        static POOL: GlobalPool = GlobalPool::new();
    }
    let pool = POOL.with(GlobalPool::clone);

    let mut writer = BytesBuf::new().into_writer(pool);
    postcard::to_io(value, &mut writer).map_err(Error::from_source)?;
    Ok(writer.into_inner().peek())
}

/// Returns a contiguous byte slice from a [`BytesView`]. Zero-copy for single-span
/// views (the common case), collects into a Vec only for multi-span views.
fn to_contiguous(view: &BytesView) -> Cow<'_, [u8]> {
    let first = view.first_slice();
    if first.len() == view.len() {
        Cow::Borrowed(first)
    } else {
        let mut buf = Vec::with_capacity(view.len());
        for (slice, _) in view.slices() {
            buf.extend_from_slice(slice);
        }
        Cow::Owned(buf)
    }
}

impl<T: Serialize + DeserializeOwned + Send + Sync> Codec<T, BytesView> for PostcardCodec {
    fn decode(&self, value: BytesView) -> Result<T, Error> {
        let bytes = to_contiguous(&value);
        postcard::from_bytes(&bytes).map_err(Error::from_source)
    }
}
