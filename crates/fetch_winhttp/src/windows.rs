// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Windows coverage anchor for this re-export-only crate.
//!
//! Public items come from `fetch_winhttp_impl`. This module keeps one tested
//! item so the coverage result is not empty (AB#7790459). The `windows` file
//! name matches workspace tooling for platform-gated sources.

/// Reports whether the current target supports the WinHTTP transport.
///
/// This function compiles only on supported targets, so it always returns
/// `true`.
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
