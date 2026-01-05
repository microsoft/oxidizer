// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Macros for the [`data_privacy`](https://docs.rs/data_privacy) crate.

#![expect(
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::unwrap_used,
    reason = "This is macro code"
)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/data_privacy_macros/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/data_privacy_macros/favicon.ico"
)]

pub mod classified;
pub mod derive;
pub mod taxonomy;
