// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Windows coverage anchor for the re-export-only facade.
//!
//! Public items live in `fetch_winhttp_impl` and are re-exported here, so the
//! facade would otherwise have no instrumented code. This module keeps one
//! exercised item so coverage measurement is not empty (AB#7790459). The
//! `windows` file name matches workspace tooling that skips platform-gated
//! sources it cannot build.

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
