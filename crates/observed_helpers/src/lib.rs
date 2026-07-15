// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Helper types and utilities that expand `observed` functionality.
//!
//! This crate is less stable than `observed` itself and may have breaking changes.

mod format_any_value;
mod sensitive_slice;

pub use format_any_value::format_any_value;
pub use sensitive_slice::SensitiveSlice;
