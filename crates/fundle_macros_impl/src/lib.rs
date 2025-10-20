// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! # Fundle Macros Implementation
//!
//! See the `fundle_macros` crate for more information.

#![expect(clippy::missing_panics_doc, clippy::missing_errors_doc, reason = "This is a macro")]

mod bundle;
mod deps;
mod newtype;

pub use bundle::bundle;
pub use deps::deps;
pub use newtype::newtype;
