// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;

use super::{Encoder, Error};

/// Runtime contract implemented by `ToQuery` derive output.
#[doc(hidden)]
pub trait EncodeFields {
    /// Writes the fields of this schema to an encoder.
    fn encode_fields<W: fmt::Write>(&self, encoder: &mut Encoder<'_, W>) -> Result<(), Error>;
}
