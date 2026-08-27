// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Non-Windows coverage anchor for a Windows-only implementation crate.
//!
//! Without this module the library would compile with no instrumented code on
//! non-Windows targets, which coverage tooling treats as a failed measurement
//! (AB#7790459). The `linux` file name matches workspace tooling that skips
//! platform-gated sources it cannot build; the module itself is gated on
//! `not(windows)`.

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
