// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error as StdError;

/// The core error information - either a transparent error, a wrapped error, or none.
#[derive(Debug)]
pub enum Source {
    /// No source error (used when display provides the message)
    None,
    /// A transparent error that acts as the main error
    Transparent(Box<dyn StdError + Send + Sync>),
    /// A wrapped source error
    Error(Box<dyn StdError + Send + Sync>),
}
