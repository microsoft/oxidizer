// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementation details for the [`fundle`](https://docs.rs/fundle) crate.
//! See `fundle` for more information.

#![expect(clippy::missing_panics_doc, clippy::missing_errors_doc, reason = "This is a macro")]

mod bundle;
mod deps;
mod newtype;

pub use bundle::bundle;
pub use deps::deps;
pub use newtype::newtype;
