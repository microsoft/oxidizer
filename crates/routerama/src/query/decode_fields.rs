// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::QueryDecoder;

/// Runtime contract implemented by `FromQuery` derive output.
#[doc(hidden)]
pub trait DecodeFields<'q>: Sized {
    /// Whether the outermost decoding boundary rejects unclaimed fields.
    const DENY_UNKNOWN_FIELDS: bool;

    /// Creates an opaque incremental decoder for this schema.
    fn decoder() -> impl QueryDecoder<'q, Output = Self>;
}
