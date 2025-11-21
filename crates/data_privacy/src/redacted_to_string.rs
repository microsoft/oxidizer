// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::RedactionEngine;

/// Trait for converting a value to a redacted string representation.
pub trait RedactedToString {
    /// Converts the value to a redacted string representation.
    fn to_string(&self, engine: &RedactionEngine) -> String;
}
