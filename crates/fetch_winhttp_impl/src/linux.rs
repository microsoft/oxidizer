// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Placeholder contents for targets that do not support WinHTTP.
//!
//! The transport is implemented against WinHTTP and therefore exists only on
//! Windows. Configuring the implementation out would otherwise leave a library
//! with no instrumented code at all, which the coverage tooling cannot
//! distinguish from a failed measurement. This module keeps a single trivially
//! exercised item in the build so that the measurement remains meaningful, and
//! carries no behavior of its own.
//!
//! The file is named for the platform the workspace supports besides Windows,
//! which is the naming the tooling matches on to skip platform-gated code that
//! it cannot build. The module is gated on the absence of Windows rather than
//! on Linux specifically, so that the library keeps its instrumented item on
//! any other target as well.

/// Reports whether the current target supports the WinHTTP transport.
///
/// Only compiled on targets where it does not, so the answer is fixed.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "exists so the library carries instrumented code on targets without WinHTTP")
)]
pub(crate) const fn is_supported() -> bool {
    false
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::is_supported;

    #[test]
    fn winhttp_is_unsupported_on_this_target() {
        assert!(!is_supported());
    }
}
