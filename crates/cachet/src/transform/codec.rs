// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::Error;

/// A one-directional encoder that converts values from type `From` to type `To`.
///
/// Used by [`TransformAdapter`](super::TransformAdapter) for key encoding where
/// only the forward direction is needed.
pub trait Encoder<From, To>: Send + Sync {
    /// Encodes a value from type `From` to type `To`.
    fn encode(&self, value: &From) -> Result<To, Error>;
}

/// A bidirectional codec that converts between types `A` and `B`.
///
/// Extends [`Encoder<A, B>`] with a `decode` method for the reverse direction.
/// Used by [`TransformAdapter`](super::TransformAdapter) for value encoding and
/// decoding.
pub trait Codec<A, B>: Encoder<A, B> {
    /// Decodes a value from type `B` back to type `A`.
    fn decode(&self, value: &B) -> Result<A, Error>;
}
