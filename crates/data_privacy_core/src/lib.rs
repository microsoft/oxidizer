// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Core data classification types and traits.
//!
//! The `data_privacy_core` crate contains the trait definitions and the [`DataClass`] type
//! with no support for `#[derive()]` or attribute macros.
//!
//! In crates that use `#[taxonomy]`, `#[classified]`, `#[derive(RedactedDebug)]`, or
//! `#[derive(RedactedDisplay)]`, you must depend on the **[`data_privacy`](https://docs.rs/data_privacy)**
//! crate, not `data_privacy_core`.
//!
//! In crates that hand-write implementations of data privacy traits, or only use them as trait
//! bounds, depending on `data_privacy_core` is permitted. But `data_privacy` re-exports all of
//! these traits and can be used for this use case too. **If in doubt, disregard `data_privacy_core`
//! and always use `data_privacy`.**
//!
//! # Contents
//!
//! - [`DataClass`] - identifies a data class within a taxonomy
//! - [`Classified`] - trait for types that hold classified data
//! - [`Redactor`] - trait for types that can apply redaction
//! - [`RedactedDebug`] / [`RedactedDisplay`] / [`RedactedToString`] - redaction-aware formatting traits

mod classified;
mod data_class;
mod redacted;
mod redactor;

pub use classified::Classified;
pub use data_class::{DataClass, IntoDataClass};
pub use redacted::{RedactedDebug, RedactedDisplay, RedactedToString};
pub use redactor::Redactor;
