// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::{Decoded, Error, QueryLimits};

/// Incremental decoder returned by generated query types.
#[doc(hidden)]
pub trait QueryDecoder<'q> {
    /// Value constructed after all query pairs have been decoded.
    type Output;

    /// Returns whether this schema recognizes a key without consuming its value.
    fn claims_field(&self, key: &str) -> bool;

    /// Consumes one decoded pair, returning whether this schema recognized it.
    fn decode_field(&mut self, key: &str, value: Decoded<'q>, pair_offset: usize, limits: QueryLimits) -> Result<bool, Error>;

    /// Validates accumulated fields and constructs the value.
    fn finish(self, end_offset: usize) -> Result<Self::Output, Error>;
}
