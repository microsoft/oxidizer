// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Placeholder contents for the platform that supports WinHTTP.
//!
//! Every item this crate exposes on Windows is defined in `fetch_winhttp_impl`
//! and merely re-exported here, and a re-export carries no code. That leaves a
//! library with no instrumented code at all, which the coverage tooling cannot
//! distinguish from a failed measurement (tracked as AB#7790459). This module
//! keeps a single trivially exercised item in the build so that the measurement
//! remains meaningful, and carries no behavior of its own.
//!
//! The file is named for the platform it is gated to, which is the naming the
//! mutation tooling matches on to skip platform-gated code that it cannot
//! build.

/// Reports whether the current target supports the WinHTTP transport.
///
/// Only compiled on targets where it does, so the answer is fixed.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "exists so the library carries instrumented code on targets with WinHTTP")
)]
pub(crate) const fn is_supported() -> bool {
    true
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::is_supported;

    #[test]
    fn winhttp_is_supported_on_this_target() {
        assert!(is_supported());
    }
}
