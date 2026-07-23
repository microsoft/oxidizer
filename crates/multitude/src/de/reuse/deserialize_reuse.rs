// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use serde::de::Deserializer;

pub(crate) trait DeserializeReuse<'de> {
    /// Replace `self` from `deserializer` while retaining reusable capacity.
    ///
    /// # Errors
    ///
    /// Returns an error from the deserializer for invalid input or allocation
    /// failure.
    fn deserialize_reusing<D>(&mut self, deserializer: D) -> Result<(), D::Error>
    where
        D: Deserializer<'de>;
}
