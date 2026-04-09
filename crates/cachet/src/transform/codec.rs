// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::Error;

/// Wraps an infallible closure so it can be used where a fallible one is expected.
///
/// This avoids the need to write `Ok::<_, std::convert::Infallible>(...)` in
/// encoder/codec closures that cannot fail.
///
/// # Examples
///
/// ```
/// use cachet::{infallible, TransformCodec};
///
/// // Mixed fallibility: encode is fallible, decode is infallible
/// let codec = TransformCodec::new(
///     |v: &String| v.parse::<i32>(),
///     infallible(|v: &i32| v.to_string()),
/// );
/// ```
pub fn infallible<A, B, F>(f: F) -> impl Fn(&A) -> Result<B, std::convert::Infallible> + Send + Sync + 'static
where
    F: Fn(&A) -> B + Send + Sync + 'static,
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

/// A bidirectional codec that converts between types `A` and `B`.
///
/// Extends [`Encoder<A, B>`] with a `decode` method for the reverse direction.
/// Used for value encoding and decoding in the transform builder pipeline.
pub trait Codec<A, B>: Encoder<A, B> {
    /// Decodes a value from type `B` back to type `A`.
    ///
    /// # Errors
    ///
    /// Returns an error if the decoding fails.
    fn decode(&self, value: &B) -> Result<A, Error>;
}
