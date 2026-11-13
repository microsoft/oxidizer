// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Direct reference-backend access for differential benchmarks.
//!
//! Scalar functions provide benchmark references. Architecture-specific URI
//! functions return `None` when unavailable and bypass application dispatch.
//!
//! # Examples
//!
//! ```
//! # #[cfg(feature = "benchmarking")]
//! # {
//! use http_headers_simd::benchmarking;
//!
//! assert_eq!(
//!     benchmarking::is_token_scalar(b"content-type"),
//!     http_headers_simd::is_token(b"content-type"),
//! );
//! # }
//! ```

#[cfg(all(feature = "std", any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
use std::arch;

use crate::{EmptyMembers, TokenListScan, base64, list, range, scalar, uri};

/// Runs the production scalar token validator.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "benchmarking")]
/// assert!(http_headers_simd::benchmarking::is_token_scalar(
///     b"content-type"
/// ));
/// ```
#[must_use]
pub fn is_token_scalar(bytes: &[u8]) -> bool {
    scalar::is_token(bytes)
}

/// Runs the production scalar `token68` validator.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "benchmarking")]
/// assert!(http_headers_simd::benchmarking::is_token68_scalar(
///     b"YWJjZA=="
/// ));
/// ```
#[must_use]
pub fn is_token68_scalar(bytes: &[u8]) -> bool {
    scalar::is_token68(bytes)
}

/// Runs the production scalar field-value validator.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "benchmarking")]
/// assert!(http_headers_simd::benchmarking::is_field_value_scalar(
///     b"text/plain"
/// ));
/// ```
#[must_use]
pub fn is_field_value_scalar(bytes: &[u8]) -> bool {
    scalar::is_field_value(bytes)
}

/// Runs the production scalar interesting-byte scanner.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "benchmarking")]
/// assert_eq!(
///     http_headers_simd::benchmarking::find_interesting_scalar(b"gzip, br"),
///     Some(4),
/// );
/// ```
#[must_use]
pub fn find_interesting_scalar(bytes: &[u8]) -> Option<usize> {
    scalar::find_interesting(bytes)
}

/// Runs the production scalar base64-alphabet scanner.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "benchmarking")]
/// assert!(http_headers_simd::benchmarking::all_base64_alphabet_scalar(
///     b"AZaz09+/"
/// ));
/// ```
#[must_use]
pub fn all_base64_alphabet_scalar(bytes: &[u8]) -> bool {
    base64::all_base64_alphabet(bytes)
}

/// Runs the unaccelerated origin-relative reference check.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "benchmarking")]
/// assert!(http_headers_simd::benchmarking::is_simple_uri_path_scalar(
///     b"/docs?page=2",
/// ));
/// ```
#[must_use]
pub fn is_simple_uri_path_scalar(bytes: &[u8]) -> bool {
    uri::is_simple_uri_path(bytes)
}

/// Runs the unaccelerated byte-range-set scan.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "benchmarking")]
/// assert!(http_headers_simd::benchmarking::scan_byte_range_set_scalar(
///     b"bytes=0-499",
///     6,
/// ));
/// ```
#[must_use]
pub fn scan_byte_range_set_scalar(bytes: &[u8], start: usize) -> bool {
    range::scan_byte_range_set(bytes, start)
}

/// Runs the unaccelerated comma-separated token list scan.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "benchmarking")]
/// # {
/// use http_headers_simd::{EmptyMembers, TokenListScan, benchmarking};
///
/// assert_eq!(
///     benchmarking::scan_token_list_scalar(b"gzip, br", EmptyMembers::Skip),
///     TokenListScan::Members,
/// );
/// # }
/// ```
#[must_use]
pub fn scan_token_list_scalar(bytes: &[u8], empty: EmptyMembers) -> TokenListScan {
    list::scan_token_list(bytes, empty)
}

/// Runs the SSE2 URI scanner directly when the current host supports it.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "benchmarking")]
/// # {
/// let result = http_headers_simd::benchmarking::is_simple_uri_path_sse2(b"/long/simple/path");
/// assert!(result.is_none() || result == Some(true));
/// # }
/// ```
#[must_use]
pub fn is_simple_uri_path_sse2(bytes: &[u8]) -> Option<bool> {
    #[cfg(all(feature = "std", any(target_arch = "x86", target_arch = "x86_64")))]
    if uri_backend_eligible(bytes, || arch::is_x86_feature_detected!("sse2")) {
        // SAFETY: runtime detection establishes SSE2 and the length check
        // satisfies the scanner's trailing-block precondition.
        return Some(unsafe { crate::x86::is_simple_uri_tail_sse2(bytes) });
    }
    let _ = bytes;
    None
}

/// Runs the SSSE3 URI scanner directly when the current host supports it.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "benchmarking")]
/// # {
/// let result = http_headers_simd::benchmarking::is_simple_uri_path_ssse3(b"/long/simple/path");
/// assert!(result.is_none() || result == Some(true));
/// # }
/// ```
#[must_use]
pub fn is_simple_uri_path_ssse3(bytes: &[u8]) -> Option<bool> {
    #[cfg(all(feature = "std", any(target_arch = "x86", target_arch = "x86_64")))]
    if uri_backend_eligible(bytes, || arch::is_x86_feature_detected!("ssse3")) {
        // SAFETY: runtime detection establishes SSSE3 and the length check
        // satisfies the scanner's trailing-block precondition.
        return Some(unsafe { crate::x86::is_simple_uri_tail_ssse3(bytes) });
    }
    let _ = bytes;
    None
}

/// Runs the SSE4.2 URI scanner directly when the current host supports it.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "benchmarking")]
/// # {
/// let result = http_headers_simd::benchmarking::is_simple_uri_path_sse42(b"/long/simple/path");
/// assert!(result.is_none() || result == Some(true));
/// # }
/// ```
#[must_use]
pub fn is_simple_uri_path_sse42(bytes: &[u8]) -> Option<bool> {
    #[cfg(all(feature = "std", any(target_arch = "x86", target_arch = "x86_64")))]
    if uri_backend_eligible(bytes, || arch::is_x86_feature_detected!("sse4.2")) {
        // SAFETY: runtime detection establishes SSE4.2 and the length
        // check satisfies the scanner's trailing-block precondition.
        return Some(unsafe { crate::x86::is_simple_uri_tail_sse42(bytes) });
    }
    let _ = bytes;
    None
}

/// Runs the NEON URI scanner directly when the current host supports it.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "benchmarking")]
/// # {
/// let result = http_headers_simd::benchmarking::is_simple_uri_path_neon(b"/long/simple/path");
/// assert!(result.is_none() || result == Some(true));
/// # }
/// ```
#[must_use]
pub fn is_simple_uri_path_neon(bytes: &[u8]) -> Option<bool> {
    if bytes.len() < 16 || !uri::has_origin_relative_prefix(bytes) {
        return None;
    }
    let scan = neon_uri_scanner()?;
    // SAFETY: the provider establishes CPU support and the checks above provide a whole path vector.
    Some(unsafe { scan(bytes) })
}

type UriScanner = unsafe fn(&[u8]) -> bool;

#[inline]
fn neon_uri_scanner() -> Option<UriScanner> {
    #[cfg(all(test, feature = "std"))]
    if let Some((available, scan)) = tests::NEON_URI_OVERRIDE.get() {
        return available.then_some(scan as UriScanner);
    }
    #[cfg(all(feature = "std", target_arch = "aarch64"))]
    {
        arch::is_aarch64_feature_detected!("neon").then_some(crate::arm::is_simple_uri_tail as UriScanner)
    }
    #[cfg(not(all(feature = "std", target_arch = "aarch64")))]
    {
        None
    }
}

#[cfg(all(
    feature = "std",
    any(target_arch = "x86", target_arch = "x86_64", all(test, target_arch = "aarch64"))
))]
#[inline]
fn uri_backend_eligible(bytes: &[u8], available: impl FnOnce() -> bool) -> bool {
    bytes.len() >= 16 && uri::has_origin_relative_prefix(bytes) && available()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[cfg(feature = "std")]
    type NeonUriHook = (bool, fn(&[u8]) -> bool);

    #[cfg(feature = "std")]
    std::thread_local! {
        pub(super) static NEON_URI_OVERRIDE: core::cell::Cell<Option<NeonUriHook>> = const { core::cell::Cell::new(None) };
    }

    #[cfg(feature = "std")]
    #[test]
    fn gamma_nov6_neon_wrapper_forwards_the_exact_eligible_scanner_answer() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                NEON_URI_OVERRIDE.set(None);
            }
        }
        assert!(NEON_URI_OVERRIDE.get().is_none());
        let _reset = Reset;
        for answer in [false, true] {
            let scan: fn(&[u8]) -> bool = if answer {
                |bytes| {
                    assert_eq!(bytes, b"/0123456789abcdef");
                    true
                }
            } else {
                |bytes| {
                    assert_eq!(bytes, b"/0123456789abcdef");
                    false
                }
            };
            NEON_URI_OVERRIDE.set(Some((true, scan)));
            assert_eq!(super::is_simple_uri_path_neon(b"/0123456789abcdef"), Some(answer));
        }
        NEON_URI_OVERRIDE.set(Some((false, |_| panic!("unavailable backend must not run"))));
        assert_eq!(super::is_simple_uri_path_neon(b"/0123456789abcdef"), None);
        NEON_URI_OVERRIDE.set(Some((true, |_| true)));
        assert_eq!(super::is_simple_uri_path_neon(b"/0123456789abcd"), None);
        assert_eq!(super::is_simple_uri_path_neon(b"/0123456789abcde"), Some(true));
        assert_eq!(super::is_simple_uri_path_neon(b"/0123456789abcdef"), Some(true));
        NEON_URI_OVERRIDE.set(Some((true, |_| panic!("ineligible input must not reach the scanner"))));
        for bytes in [b"/short".as_slice(), b"relative/long/path", b"//long/authority"] {
            assert_eq!(super::is_simple_uri_path_neon(bytes), None, "{bytes:?}");
        }
    }

    #[cfg(all(feature = "std", any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
    #[test]
    fn gamma_afternoon_uri_backend_policy_requires_an_available_isa_and_preserves_lazy_detection() {
        for bytes in [
            b"/short".as_slice(),
            b"relative/long/path",
            b"//long/authority",
            b"/long/simple/path",
        ] {
            for available in [false, true] {
                let queried = core::cell::Cell::new(false);
                let eligible = bytes.len() >= 16 && bytes.starts_with(b"/") && !bytes.starts_with(b"//");
                assert_eq!(
                    super::uri_backend_eligible(bytes, || {
                        queried.set(true);
                        available
                    }),
                    eligible && available,
                );
                assert_eq!(queried.get(), eligible);
            }
        }
    }

    #[cfg(all(not(feature = "std"), any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
    use std::arch;
    #[cfg(not(feature = "std"))]
    use std::vec;

    use super::*;

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn assert_direct_uri_backends(path: &[u8], expected: bool) {
        assert_eq!(is_simple_uri_path_sse2(path), Some(expected));
        assert_eq!(
            is_simple_uri_path_ssse3(path),
            arch::is_x86_feature_detected!("ssse3").then_some(expected)
        );
        assert_eq!(
            is_simple_uri_path_sse42(path),
            arch::is_x86_feature_detected!("sse4.2").then_some(expected)
        );
        assert_eq!(is_simple_uri_path_neon(path), None);
    }

    #[cfg(target_arch = "aarch64")]
    fn assert_direct_uri_backends(path: &[u8], expected: bool) {
        assert_eq!(is_simple_uri_path_sse2(path), None);
        assert_eq!(is_simple_uri_path_ssse3(path), None);
        assert_eq!(is_simple_uri_path_sse42(path), None);
        assert_eq!(
            is_simple_uri_path_neon(path),
            arch::is_aarch64_feature_detected!("neon").then_some(expected)
        );
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
    fn assert_direct_uri_backends(path: &[u8], _expected: bool) {
        assert_eq!(is_simple_uri_path_sse2(path), None);
        assert_eq!(is_simple_uri_path_ssse3(path), None);
        assert_eq!(is_simple_uri_path_sse42(path), None);
        assert_eq!(is_simple_uri_path_neon(path), None);
    }

    #[test]
    fn scalar_entry_points_expose_the_reference_backends() {
        assert!(is_token_scalar(b"token"));
        assert!(!is_token_scalar(b"not a token"));
        assert!(is_token68_scalar(b"abc+/=="));
        assert!(!is_token68_scalar(b"=abc"));
        assert!(is_field_value_scalar(&[b'\t', b' ', 0x80]));
        assert!(!is_field_value_scalar(b"line\nbreak"));
        assert_eq!(find_interesting_scalar(b"plain,value"), Some(5));
        assert!(all_base64_alphabet_scalar(b"AZaz09+/"));
        assert!(!all_base64_alphabet_scalar(b"padding="));
        assert!(is_simple_uri_path_scalar(b"/path?query#fragment"));
        assert!(!is_simple_uri_path_scalar(b"//authority/path"));
        assert!(scan_byte_range_set_scalar(b"bytes=0-499", 6));
        assert!(!scan_byte_range_set_scalar(b"bytes=500-100", 6));
        assert_eq!(scan_token_list_scalar(b"gzip, deflate", EmptyMembers::Skip), TokenListScan::Members);
    }

    #[test]
    fn direct_uri_backends_validate_preconditions_and_match_scalar() {
        let valid = vec![b'a'; 48];
        let mut path = valid;
        path[0] = b'/';
        let expected = is_simple_uri_path_scalar(&path);

        assert_direct_uri_backends(&path, expected);

        assert_eq!(is_simple_uri_path_sse2(b"/short"), None);
        assert_eq!(is_simple_uri_path_ssse3(b"relative-path-that-is-long"), None);
        assert_eq!(is_simple_uri_path_sse42(b"//authority/path-that-is-long"), None);
        assert_eq!(is_simple_uri_path_neon(b"/short"), None);
    }
}
