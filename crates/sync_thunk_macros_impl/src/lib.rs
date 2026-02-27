// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Macros for the [`sync_thunk`](https://docs.rs/sync_thunk) crate.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/sync_thunk_macros/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/sync_thunk_macros/favicon.ico"
)]

/// Implementation of the `thunk` attribute macro.
mod thunk;

pub use thunk::thunk_impl;
