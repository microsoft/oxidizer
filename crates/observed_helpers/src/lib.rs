// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Helper types and utilities that expand `observed` functionality.
//!
//! This crate is less stable than `observed` itself and may have breaking changes.

mod format_any_value;
mod sensitive_slice;

pub use format_any_value::format_any_value;
pub use sensitive_slice::SensitiveSlice;
