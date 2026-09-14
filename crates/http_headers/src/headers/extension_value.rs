// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Distinguishes a flag-style extension from one carrying a value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExtensionValue<'a> {
    /// Emits only the extension name.
    Flag,
    /// Emits the extension name followed by `=` and this value.
    Value(&'a str),
}
