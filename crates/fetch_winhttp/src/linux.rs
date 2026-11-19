// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Non-Windows coverage anchor for this Windows-only crate.
//!
//! WinHTTP is not available on these targets, so the crate has no public API.
//! This module keeps one tested item so the coverage result is not empty
//! (AB#7790459). The `linux` file name matches workspace tooling for
//! platform-gated sources.

/// Reports whether the current target supports the WinHTTP transport.
///
/// This function compiles only on unsupported targets, so it always returns
/// `false`.
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
