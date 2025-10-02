// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Utility macros for exporting data from a bundle.
//!
//! Currently used, but not useful, due to Rust compiler bug #51445. Once
//! that is fixed, the `#[forward]` macro should work without parameters.

/// General info for exported data.
///
/// This is implemented for each bundle, e.g., `AppState`.
pub trait Exports {
    /// Number of exports for this bundle.
    const NUM_EXPORTS: usize;
}

/// Marker for the single N-th export.
///
/// This is implemented multiple times for each bundle,
/// once per contained field.
pub trait Export<const N: usize> {
    /// Type of the export.
    type T;
    /// Get the N-th export.
    fn get(&self) -> &Self::T;
}
