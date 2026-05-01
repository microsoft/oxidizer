// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::Error;
use std::fmt::Debug;

/// Wraps an infallible closure taking a reference so it can be used where a fallible one is expected.
///
/// Use this for encoder closures that borrow their input.
///
/// # Examples
///
/// ```
/// use cachet::{infallible, TransformEncoder};
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
/// use cachet::{infallible_owned, TransformCodec, infallible};
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
/// Used for key encoding in the transform builder pipeline, where
/// only the forward direction is needed.
pub trait Encoder<From, To>: Send + Sync {
    /// Encodes a value from type `From` to type `To`.
    ///
    /// # Errors
    ///
    /// Returns an error if the encoding fails.
    fn encode(&self, value: &From) -> Result<To, Error>;
}

/// The result of a decode operation.
///
/// Used by [`Codec::decode`] to distinguish between a successful decode,
/// a soft failure that should be treated as a cache miss, and a hard error.
#[derive(Debug)]
pub enum DecodeOutcome<T> {
    /// The value was successfully decoded.
    Value(T),
    /// The stored data is undecodable and should be treated as a cache miss.
    ///
    /// The string describes the reason (e.g., "version mismatch", "empty payload").
    SoftFailure(&'static str),
}

/// A bidirectional codec that converts between types `A` and `B`.
///
/// Extends [`Encoder<A, B>`] with a `decode` method for the reverse direction.
/// Used for value encoding and decoding in the transform builder pipeline.
pub trait Codec<A, B>: Encoder<A, B> {
    /// Decodes a value from type `B` back to type `A`.
    ///
    /// # Returns
    ///
    /// - `Ok(DecodeOutcome::Value(v))` on success
    /// - `Ok(DecodeOutcome::SoftFailure(reason))` if the stored data is undecodable
    ///   and should be treated as a cache miss
    ///
    /// # Errors
    ///
    /// Returns `Err` for hard failures that should propagate to the caller.
    fn decode(&self, value: B) -> Result<DecodeOutcome<A>, Error>;
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
            encode_fn: Box::new(move |a| encode_fn(a).map_err(|e| Error::from_source(e))),
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

impl<A, B> Encoder<A, B> for TransformCodec<A, B> {
    fn encode(&self, value: &A) -> Result<B, Error> {
        (self.encode_fn)(value)
    }
}

impl<A, B> Codec<A, B> for TransformCodec<A, B> {
    fn decode(&self, value: B) -> Result<DecodeOutcome<A>, Error> {
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
