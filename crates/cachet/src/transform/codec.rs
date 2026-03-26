// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::Error;

/// A single-direction transform codec.
///
/// Converts values from type `T1` to type `T2`. Used by [`TransformAdapter`](super::TransformAdapter)
/// for key encoding, value encoding, and value decoding.
pub trait Codec<T1, T2>: Send + Sync {
    fn apply(&self, value: &T1) -> Result<T2, Error>;
}
