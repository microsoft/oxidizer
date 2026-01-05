// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Macros for the [`fundle`](https://docs.rs/fundle) crate.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fundle_macros_impl/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fundle_macros_impl/favicon.ico"
)]
#![expect(clippy::missing_panics_doc, clippy::missing_errors_doc, reason = "This is a macro")]

mod bundle;
mod deps;
mod newtype;

pub use bundle::bundle;
pub use deps::deps;
pub use newtype::newtype;
