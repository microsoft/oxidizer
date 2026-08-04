// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::fmt::Debug;

use crate::Error;

/// Wraps an infallible closure taking a reference so it can be used where a fallible one is expected.
///
/// Use this for encoder closures that borrow their input.
///
/// # Examples
///
/// ```
/// use cachet::{TransformEncoder, infallible};
///
/// let encoder = TransformEncoder::new(infallible(|v: &i32| v.to_string()));
/// ```
pub fn infallible<A, B, F>(f: F) -> impl Fn(&A) -> Result<B, std::convert::Infallible> + Send + Sync + 'static
where
    F: Fn(&A) -> B + Send + Sync + 'static,
{
    move |a| Ok(f(a))
}

/// Wraps an infallible closure taking an owned value so it can be used where a fallible one is expected.
///
/// Use this for decoder closures that consume their input.
///
/// # Examples
///
/// ```
/// use cachet::{TransformCodec, infallible, infallible_owned};
///
/// let codec = TransformCodec::new(
///     |v: &String| v.parse::<i32>(),
///     infallible_owned(|v: i32| v.to_string()),
/// );
/// ```
pub fn infallible_owned<A, B, F>(f: F) -> impl Fn(A) -> Result<B, std::convert::Infallible> + Send + Sync + 'static
where
    F: Fn(A) -> B + Send + Sync + 'static,
{
    move |a| Ok(f(a))
}

/// A one-directional encoder that converts values from type `From` to type `To`.
///
/// This is the **key** side of the pipeline: keys are only ever encoded — never decoded
/// back, since every operation re-encodes the key it already has — and must encode
/// deterministically so lookups stay stable. A key takes no [`CodecContext`] because it
/// *is* the context. Contrast [`Codec`], the bidirectional, context-bound **value** side.
pub trait Encoder<From, To>: Send + Sync {
    /// Encodes a value from type `From` to type `To`.
    ///
    /// # Errors
    ///
    /// Returns an error if the encoding fails.
    fn encode(&self, value: &From) -> Result<To, Error>;
}

/// Per-operation metadata threaded through [`Codec`] calls.
///
/// Currently just the storage key, which an authenticated codec binds as associated data
/// so a value can't be relocated to another key. It is a struct precisely so more
/// per-operation fields can be added later without changing how codecs receive it — a
/// codec always takes `&CodecContext<'_>`, and a context *producer* sets only the fields
/// it cares about. The key is a [`Cow`] so a producer can hand over either a borrowed
/// slice (the common single-span case) or a gathered buffer (a multi-span key) while
/// still yielding one self-contained context value.
#[derive(Debug, Clone)]
pub struct CodecContext<'a> {
    key: Cow<'a, [u8]>,
}

impl<'a> CodecContext<'a> {
    /// Creates a context bound to `key`.
    #[must_use]
    pub fn new(key: &'a [u8]) -> Self {
        Self { key: Cow::Borrowed(key) }
    }

    /// Creates a context with no key, for codecs that do not bind one (e.g. serialization
    /// or a plain type mapping).
    #[must_use]
    pub fn keyless() -> CodecContext<'static> {
        CodecContext { key: Cow::Borrowed(&[]) }
    }

    /// Creates a context bound to a possibly-owned key, for callers that must gather a
    /// multi-span key into a contiguous buffer before binding it.
    #[cfg(any(feature = "serialize", test))]
    #[must_use]
    pub(crate) fn from_key(key: Cow<'a, [u8]>) -> Self {
        Self { key }
    }

    /// The storage key this value is bound to, or an empty slice when none was supplied.
    #[must_use]
    pub fn key(&self) -> &[u8] {
        &self.key
    }
}

/// The result of a decode operation.
///
/// Used by [`Codec::decode`] to distinguish between a successful decode,
/// a soft failure that should be treated as a cache miss, and a hard error.
#[derive(Debug)]
pub enum DecodeOutcome<T> {
    /// The value was successfully decoded.
    Value(T),
    /// The stored data is undecodable and should be treated as a cache miss (as opposed
    /// to a hard [`Error`], which propagates). Why it was undecodable is not part of the
    /// general codec vocabulary — a codec that needs to react to a specific cause (e.g.
    /// an authentication failure) categorizes it internally before returning this.
    SoftFailure,
}

/// A bidirectional codec that converts between types `A` and `B`.
///
/// A codec is a stage in the value pipeline: it converts a value to its stored form
/// ([`encode`](Self::encode)) and back ([`decode`](Self::decode)), given a
/// [`CodecContext`]. Serialization ignores the context; an authenticated (protection)
/// codec binds the context's key so a value cannot be relocated to a different key.
///
/// Unlike [`Encoder`], a codec is *not* used for keys — it always receives the context,
/// which already carries the key.
pub trait Codec<A, B>: Send + Sync {
    /// Encodes `value` into its stored representation, given `ctx`.
    ///
    /// # Errors
    ///
    /// Returns an error if the encoding fails.
    fn encode(&self, ctx: &CodecContext<'_>, value: &A) -> Result<B, Error>;

    /// Decodes a value from type `B` back to type `A`, given `ctx`.
    ///
    /// # Returns
    ///
    /// - `Ok(DecodeOutcome::Value(v))` on success
    /// - `Ok(DecodeOutcome::SoftFailure)` if the stored data is undecodable
    ///   and should be treated as a cache miss
    ///
    /// # Errors
    ///
    /// Returns `Err` for hard failures that should propagate to the caller.
    fn decode(&self, ctx: &CodecContext<'_>, value: B) -> Result<DecodeOutcome<A>, Error>;
}

type EncodeFn<A, B> = Box<dyn Fn(&A) -> Result<B, Error> + Send + Sync>;
type DecodeFn<A, B> = Box<dyn Fn(A) -> Result<DecodeOutcome<B>, Error> + Send + Sync>;

/// A boxed-closure encoder for custom one-directional transforms (keys).
pub struct TransformEncoder<A, B> {
    encode_fn: EncodeFn<A, B>,
}

impl<A, B> TransformEncoder<A, B> {
    /// Creates a new `TransformEncoder` from a fallible closure.
    pub fn new<EncodeError>(encode_fn: impl Fn(&A) -> Result<B, EncodeError> + Send + Sync + 'static) -> Self
    where
        EncodeError: std::error::Error + Send + Sync + 'static,
    {
        Self {
            encode_fn: Box::new(move |a| encode_fn(a).map_err(Error::from_source)),
        }
    }

    /// Creates a new `TransformEncoder` from an infallible closure.
    pub fn infallible(encode_fn: impl Fn(&A) -> B + Send + Sync + 'static) -> Self {
        Self {
            encode_fn: Box::new(move |a| Ok(encode_fn(a))),
        }
    }
}

impl<A, B> Encoder<A, B> for TransformEncoder<A, B> {
    fn encode(&self, value: &A) -> Result<B, Error> {
        (self.encode_fn)(value)
    }
}

impl<A, B> Debug for TransformEncoder<A, B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformEncoder")
            .field("A", &std::any::type_name::<A>())
            .field("B", &std::any::type_name::<B>())
            .finish()
    }
}

/// A boxed-closure codec for custom bidirectional transforms (values).
pub struct TransformCodec<A, B> {
    encode_fn: EncodeFn<A, B>,
    decode_fn: DecodeFn<B, A>,
}

impl<A, B> TransformCodec<A, B> {
    /// Creates a new `TransformCodec` from a pair of fallible closures.
    pub fn new<EncodeError, DecodeError>(
        encode_fn: impl Fn(&A) -> Result<B, EncodeError> + Send + Sync + 'static,
        decode_fn: impl Fn(B) -> Result<A, DecodeError> + Send + Sync + 'static,
    ) -> Self
    where
        EncodeError: std::error::Error + Send + Sync + 'static,
        DecodeError: std::error::Error + Send + Sync + 'static,
    {
        Self {
            encode_fn: Box::new(move |a| encode_fn(a).map_err(|e| Error::from_source(e))),
            decode_fn: Box::new(move |b| decode_fn(b).map(DecodeOutcome::Value).map_err(|e| Error::from_source(e))),
        }
    }
}

impl<A, B> Codec<A, B> for TransformCodec<A, B> {
    fn encode(&self, _ctx: &CodecContext<'_>, value: &A) -> Result<B, Error> {
        (self.encode_fn)(value)
    }

    fn decode(&self, _ctx: &CodecContext<'_>, value: B) -> Result<DecodeOutcome<A>, Error> {
        (self.decode_fn)(value)
    }
}

impl<A, B> Debug for TransformCodec<A, B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformCodec")
            .field("A", &std::any::type_name::<A>())
            .field("B", &std::any::type_name::<B>())
            .finish()
    }
}

/// A codec formed by running one codec through another: `A <-> M <-> B`.
///
/// On encode, `first` runs then `second`; on decode the order reverses, and a
/// [`SoftFailure`](DecodeOutcome::SoftFailure) from either stage propagates as a miss.
/// Used to layer an authenticated-protection stage over a serialization stage while
/// presenting a single `Codec<A, B>` to the tier.
#[cfg(feature = "encrypt")]
pub(crate) struct ChainedCodec<A, M, B> {
    first: Box<dyn Codec<A, M>>,
    second: Box<dyn Codec<M, B>>,
}

#[cfg(feature = "encrypt")]
impl<A, M, B> ChainedCodec<A, M, B> {
    pub(crate) fn new(first: Box<dyn Codec<A, M>>, second: Box<dyn Codec<M, B>>) -> Self {
        Self { first, second }
    }
}

#[cfg(feature = "encrypt")]
impl<A, M, B> Codec<A, B> for ChainedCodec<A, M, B>
where
    A: Send + Sync,
    M: Send + Sync,
    B: Send + Sync,
{
    fn encode(&self, ctx: &CodecContext<'_>, value: &A) -> Result<B, Error> {
        let middle = self.first.encode(ctx, value)?;
        self.second.encode(ctx, &middle)
    }

    fn decode(&self, ctx: &CodecContext<'_>, value: B) -> Result<DecodeOutcome<A>, Error> {
        match self.second.decode(ctx, value)? {
            DecodeOutcome::Value(middle) => self.first.decode(ctx, middle),
            DecodeOutcome::SoftFailure => Ok(DecodeOutcome::SoftFailure),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn codec_context_exposes_bound_and_keyless_keys() {
        let bound = CodecContext::new(b"storage-key");
        assert_eq!(bound.key(), b"storage-key");
        assert!(CodecContext::keyless().key().is_empty());
    }
}
