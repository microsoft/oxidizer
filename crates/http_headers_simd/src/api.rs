// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public byte-scanning API and backend identification.

use core::str;

use crate::dispatch;
use crate::list::{EmptyMembers, TokenListScan};

/// Returns `bytes` as text when it holds nothing but ASCII.
///
/// The check reduces the high bits of the whole slice with OR, which has no
/// data-dependent branch and vectorizes, so a short header value settles in a
/// handful of instructions. The general UTF-8 validator instead pays for
/// pointer alignment and for the multi-byte sequence state machine even when
/// the input turns out to be plain ASCII.
///
/// `None` means the value holds bytes above `0x7f`. Callers that must accept
/// those need [`core::str::from_utf8`], which this function never replaces for
/// correctness — only for speed on the ASCII shape that headers almost always
/// take.
///
/// # Examples
///
/// ```
/// assert_eq!(
///     http_headers_simd::ascii_str(b"Content-Type"),
///     Some("Content-Type")
/// );
/// assert_eq!(http_headers_simd::ascii_str(&[0xff]), None);
/// ```
#[must_use]
#[inline]
pub fn ascii_str(bytes: &[u8]) -> Option<&str> {
    all_ascii(bytes).then(|| {
        // SAFETY: every byte is below `0x80`, so each one is a single-byte
        // UTF-8 sequence encoding the code point of the same value.
        unsafe { str::from_utf8_unchecked(bytes) }
    })
}

/// Returns whether every byte is below `0x80`.
///
/// Chunking keeps the reduction inside a fixed-width accumulator so the
/// compiler emits one vector OR per block instead of a per-byte compare and
/// branch.
fn all_ascii(bytes: &[u8]) -> bool {
    const BLOCK: usize = 16;

    let mut chunks = bytes.chunks_exact(BLOCK);
    let mut high = 0_u8;
    for chunk in &mut chunks {
        let mut block = 0_u8;
        for &byte in chunk {
            block |= byte;
        }
        high |= block;
    }
    for &byte in chunks.remainder() {
        high |= byte;
    }
    high < 0x80
}

/// Returns whether `bytes` is a non-empty RFC 9110 HTTP token.
///
/// # Examples
///
/// ```
/// assert!(http_headers_simd::is_token(b"content-type"));
/// assert!(!http_headers_simd::is_token(b"content type"));
/// ```
#[must_use]
#[inline]
pub fn is_token(bytes: &[u8]) -> bool {
    dispatch::is_token(bytes)
}

/// Returns whether `bytes` is an RFC 9110 `token68` value.
///
/// A value contains one or more alphanumeric or `-._~+/` bytes followed only
/// by optional `=` padding.
///
/// # Examples
///
/// ```
/// assert!(http_headers_simd::is_token68(
///     b"QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
/// ));
/// assert!(!http_headers_simd::is_token68(b"=padding-first"));
/// ```
#[must_use]
#[inline]
pub fn is_token68(bytes: &[u8]) -> bool {
    dispatch::is_token68(bytes)
}

/// Returns whether every byte is permitted in an HTTP field value.
///
/// This accepts SP, HTAB, visible ASCII, and `obs-text` (`0x80..=0xff`).
/// Empty values are valid.
///
/// # Examples
///
/// ```
/// assert!(http_headers_simd::is_field_value(
///     b"text/plain; charset=utf-8"
/// ));
/// assert!(!http_headers_simd::is_field_value(b"line\nbreak"));
/// ```
#[must_use]
#[inline]
pub fn is_field_value(bytes: &[u8]) -> bool {
    dispatch::is_field_value(bytes)
}

/// Compares two byte strings using ASCII case-insensitive equality.
///
/// Non-ASCII bytes compare exactly and are never case-folded.
///
/// # Examples
///
/// ```
/// assert!(http_headers_simd::eq_ignore_ascii_case(b"gzip", b"GZIP"));
/// assert!(!http_headers_simd::eq_ignore_ascii_case(b"gzip", b"br"));
/// ```
#[must_use]
#[inline]
pub fn eq_ignore_ascii_case(left: &[u8], right: &[u8]) -> bool {
    dispatch::eq_ignore_ascii_case(left, right)
}

/// Recognizes a simple origin-relative URI-reference.
///
/// The accepted subset is a `path-absolute` optionally followed by a query and
/// a fragment, spelled entirely with bytes that carry no escaping or structure:
/// `unreserved`, `sub-delims`, `:`, `@`, `/`, `?`, and at most one `#`. A
/// leading `//` is rejected because it introduces an authority.
///
/// A `false` result means "not known to be valid", not "invalid": percent
/// escapes, schemes, and authorities all leave the subset. Callers must run a
/// full parse in that case.
///
/// # Examples
///
/// ```
/// assert!(http_headers_simd::is_simple_uri_path(
///     b"/docs?page=2#results"
/// ));
/// assert!(!http_headers_simd::is_simple_uri_path(
///     b"//example.com/docs"
/// ));
/// ```
#[must_use]
#[inline]
pub fn is_simple_uri_path(bytes: &[u8]) -> bool {
    dispatch::is_simple_uri_path(bytes)
}

/// Returns a recognized simple URI-reference as text.
///
/// The origin-relative subset is exactly the one [`is_simple_uri_path`]
/// accepts. The absolute subset adds a `scheme "://" host [":" port]` prefix
/// whose host is spelled only with `unreserved` and `sub-delims` bytes,
/// followed by a path, query, and fragment obeying the same rules as the
/// relative shape. Userinfo, IP-literals, an empty host, and every percent
/// escape leave the subset.
///
/// The subset holds nothing but ASCII, so recognizing it settles UTF-8
/// validity too and the caller needs no second pass over the bytes.
///
/// `None` means "not known to be valid", not "invalid". Callers must run a
/// full parse in that case.
///
/// # Examples
///
/// ```
/// assert_eq!(
///     http_headers_simd::as_simple_uri_reference(b"https://example.com/docs"),
///     Some("https://example.com/docs"),
/// );
/// assert_eq!(
///     http_headers_simd::as_simple_uri_reference(b"https://[::1]/"),
///     None
/// );
/// ```
#[must_use]
#[inline]
pub fn as_simple_uri_reference(bytes: &[u8]) -> Option<&str> {
    dispatch::is_simple_uri_reference(bytes).then(|| {
        debug_assert!(bytes.is_ascii());
        // SAFETY: every byte the subset admits is drawn from `unreserved`,
        // `sub-delims`, and a handful of ASCII delimiters, so the slice is
        // ASCII and therefore already valid UTF-8.
        unsafe { str::from_utf8_unchecked(bytes) }
    })
}

/// Scans one field line for a comma-separated list of bare HTTP tokens.
///
/// Members are separated by commas and may be surrounded by optional
/// whitespace, and `empty` decides whether a zero-length member is ignored or
/// rejects the line. Anything else — a quoted string, a parameter, a control
/// byte, or two members separated by whitespace alone — leaves the subset and
/// reports [`TokenListScan::Rejected`], which means "not a simple token list"
/// rather than "malformed": callers that need a diagnostic re-scan the line
/// with their own parser.
///
/// # Examples
///
/// ```
/// use http_headers_simd::{EmptyMembers, TokenListScan, scan_token_list};
///
/// assert_eq!(
///     scan_token_list(b"gzip, br", EmptyMembers::Skip),
///     TokenListScan::Members,
/// );
/// assert_eq!(
///     scan_token_list(b"gzip,,br", EmptyMembers::Reject),
///     TokenListScan::Rejected,
/// );
/// ```
#[must_use]
#[inline]
pub fn scan_token_list(bytes: &[u8], empty: EmptyMembers) -> TokenListScan {
    dispatch::scan_token_list(bytes, empty)
}

/// Scans a `bytes=` field line for a byte-range-set that needs no parsing.
///
/// `start` is the offset of the payload, that is, the byte after the `=`. The
/// answer is `true` only when the payload is a comma separated list of well
/// formed `first-last`, `first-`, and `-suffix` specifications whose bounds
/// are ordered and carry no leading zeros, so callers may accept the line
/// outright. Empty members are accepted because the grammar's readers skip
/// them. Everything else — a payload longer than one sixteen byte window, a
/// leading zero, whitespace anywhere but directly after a comma, or any other
/// byte — reports `false`, which means "not provably valid" rather than
/// "malformed": callers fall back to the parser that produces the diagnostic.
///
/// # Examples
///
/// ```
/// assert!(http_headers_simd::scan_byte_range_set(
///     b"bytes=0-499,-200",
///     6
/// ));
/// assert!(!http_headers_simd::scan_byte_range_set(b"bytes=500-100", 6));
/// ```
#[must_use]
#[inline]
pub fn scan_byte_range_set(bytes: &[u8], start: usize) -> bool {
    dispatch::scan_byte_range_set(bytes, start)
}

/// Returns whether every byte is a standard base64 alphabet character.
///
/// Padding is excluded, so callers must check `=` positionally themselves.
///
/// # Examples
///
/// ```
/// assert!(http_headers_simd::all_base64_alphabet(b"AZaz09+/"));
/// assert!(!http_headers_simd::all_base64_alphabet(b"YWJj="));
/// ```
#[must_use]
#[inline]
pub fn all_base64_alphabet(bytes: &[u8]) -> bool {
    dispatch::all_base64_alphabet(bytes)
}

/// Finds the first comma, semicolon, quote, backslash, SP, or HTAB.
///
/// # Examples
///
/// ```
/// assert_eq!(http_headers_simd::find_interesting(b"gzip, br"), Some(4));
/// assert_eq!(http_headers_simd::find_interesting(b"gzip"), None);
/// ```
#[must_use]
#[inline]
pub fn find_interesting(bytes: &[u8]) -> Option<usize> {
    dispatch::find_interesting(bytes)
}

/// Finds the first occurrence of either requested byte.
///
/// # Examples
///
/// ```
/// assert_eq!(
///     http_headers_simd::find_either(b"gzip, br", b',', b'"'),
///     Some(4)
/// );
/// assert_eq!(http_headers_simd::find_either(b"gzip", b',', b'"'), None);
/// ```
#[must_use]
#[inline]
pub fn find_either(bytes: &[u8], first: u8, second: u8) -> Option<usize> {
    dispatch::find_either(bytes, first, second)
}

/// Reports the available backend tier at [`simd_threshold`].
///
/// Equivalent to [`backend_for`] at that length, not an operation-specific
/// kernel selection.
///
/// # Examples
///
/// ```
/// # #[cfg(any(feature = "benchmarking", feature = "test-util"))]
/// # {
/// use http_headers_simd::{Backend, backend};
///
/// assert!(matches!(
///     backend(),
///     Backend::Scalar | Backend::Sse2 | Backend::Sse42 | Backend::Neon
/// ));
/// # }
/// ```
#[cfg(any(feature = "benchmarking", feature = "test-util"))]
#[must_use]
pub fn backend() -> Backend {
    dispatch::backend()
}

/// Reports an available backend tier using the shared byte-scanner cutoff.
///
/// Returns [`Backend::Scalar`] below [`simd_threshold`]. At or above that
/// cutoff, the reported tier describes available instruction sets, not the
/// exact kernel every scanner selects: for example, x86 token validation uses
/// SSE2 even when this helper reports [`Backend::Sse42`].
///
/// Equality, URI-tail, base64, and token-list scanning have separate cutoffs
/// documented by [`simd_threshold`], so this helper does not predict their
/// scalar/SIMD choice.
///
/// # Examples
///
/// ```
/// # #[cfg(any(feature = "benchmarking", feature = "test-util"))]
/// # {
/// use http_headers_simd::{Backend, backend_for};
///
/// assert_eq!(backend_for(0), Backend::Scalar);
/// # }
/// ```
#[cfg(any(feature = "benchmarking", feature = "test-util"))]
#[must_use]
pub fn backend_for(len: usize) -> Backend {
    dispatch::backend_for(len)
}

/// Returns the shared byte-scanner cutoff, not a crate-wide SIMD minimum.
///
/// This cutoff applies to [`is_token`], [`is_token68`], [`is_field_value`],
/// [`find_interesting`], and [`find_either`]: 16 bytes on x86/x86-64 and 32 bytes
/// on other architectures, subject to instruction-set availability.
///
/// [`eq_ignore_ascii_case`] uses a separate 32-byte cutoff. URI-tail scanning,
/// [`all_base64_alphabet`], and [`scan_token_list`] use 16-byte cutoffs.
/// For [`as_simple_uri_reference`], the URI cutoff applies to the tail after
/// any absolute authority prefix, not to the length of the whole reference.
/// Range-window classification in [`scan_byte_range_set`] and ASCII conversion
/// in [`ascii_str`] do not use this shared cutoff.
///
/// # Examples
///
/// ```
/// # #[cfg(any(feature = "benchmarking", feature = "test-util"))]
/// # {
/// use http_headers_simd::{Backend, backend_for, simd_threshold};
///
/// assert_eq!(backend_for(simd_threshold() - 1), Backend::Scalar);
/// # }
/// ```
#[cfg(any(feature = "benchmarking", feature = "test-util"))]
#[must_use]
pub const fn simd_threshold() -> usize {
    dispatch::SIMD_THRESHOLD
}

/// A byte-scanning implementation.
///
/// # Examples
///
/// ```
/// # #[cfg(any(feature = "benchmarking", feature = "test-util"))]
/// # {
/// use http_headers_simd::Backend;
///
/// assert_eq!(format!("{:?}", Backend::Scalar), "Scalar");
/// # }
/// ```
#[cfg(any(feature = "benchmarking", feature = "test-util"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    /// Generic safe Rust.
    Scalar,
    /// SSE2 on x86 or x86-64.
    Sse2,
    /// SSE4.2 on x86 or x86-64.
    Sse42,
    /// NEON on `AArch64`.
    Neon,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[cfg(all(
        any(feature = "benchmarking", feature = "test-util"),
        any(target_arch = "x86", target_arch = "x86_64")
    ))]
    use std::arch;
    #[cfg(not(feature = "std"))]
    use std::format;
    use std::time::Duration;
    #[cfg(not(feature = "std"))]
    use std::vec;
    #[cfg(not(feature = "std"))]
    use std::vec::Vec;

    use super::*;
    use crate::{base64, list, range, scalar, uri};

    fn lane_bytes(length: usize) -> impl Iterator<Item = u8> {
        // Miri keeps every byte in every lane across two blocks and a tail.
        // Other lengths retain the boundaries of each byte class.
        (u8::MIN..=u8::MAX).filter(move |byte| {
            !cfg!(miri)
                || length == 33
                || matches!(
                    *byte,
                    0 | b'\t' | b'\n' | b'\r' | 0x1f..=b'0' | b'9'..=b'A' | b'Z'..=b'a' | b'z'..=0x80 | 0xff
                )
        })
    }

    #[test]
    fn token_byte_class_is_exhaustive() {
        for byte in u8::MIN..=u8::MAX {
            let expected = byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte);
            assert_eq!(is_token(&[byte]), expected, "byte {byte:#04x}");
        }
        assert!(!is_token(b""));
    }

    #[test]
    fn token68_byte_class_is_exhaustive() {
        for byte in u8::MIN..=u8::MAX {
            let expected = byte.is_ascii_alphanumeric() || b"-._~+/".contains(&byte);
            assert_eq!(is_token68(&[byte]), expected, "byte {byte:#04x}");
        }
        assert!(!is_token68(b""));
        assert!(!is_token68(b"="));
    }

    #[test]
    fn field_value_byte_class_is_exhaustive() {
        for byte in u8::MIN..=u8::MAX {
            let expected = byte == b'\t' || byte >= b' ' && byte != 0x7f;
            assert_eq!(is_field_value(&[byte]), expected, "byte {byte:#04x}");
        }
        assert!(is_field_value(b""));
    }

    #[test]
    fn interesting_byte_class_is_exhaustive() {
        for byte in u8::MIN..=u8::MAX {
            let expected = b",;\"\\ \t".contains(&byte).then_some(0);
            assert_eq!(find_interesting(&[byte]), expected, "byte {byte:#04x}");
        }
    }

    #[test]
    fn either_byte_search_matches_scalar_across_boundaries() {
        for length in 0..=80 {
            let mut bytes = vec![b'a'; length];
            assert_eq!(find_either(&bytes, b',', b'"'), None);
            for position in 0..length {
                bytes[position] = if position % 2 == 0 { b',' } else { b'"' };
                assert_eq!(
                    find_either(&bytes, b',', b'"'),
                    Some(position),
                    "length {length}, position {position}"
                );
                bytes[position] = b'a';
            }
        }
    }

    #[test]
    fn base64_byte_class_is_exhaustive() {
        for byte in u8::MIN..=u8::MAX {
            let expected = byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/';
            assert_eq!(base64::is_base64_alphabet_byte(byte), expected, "{byte:#04x}");
            let padded = [byte; 40];
            assert_eq!(all_base64_alphabet(&padded), expected, "{byte:#04x}");
        }
    }

    #[test]
    fn base64_boundary_lengths_match_scalar() {
        for length in 0..=65 {
            let mut bytes = vec![b'a'; length];
            if length != 0 {
                bytes[length - 1] = b'=';
            }
            assert_eq!(all_base64_alphabet(&bytes), base64::all_base64_alphabet(&bytes), "length {length}");
        }
    }

    #[test]
    fn simple_uri_byte_class_is_exhaustive() {
        for byte in u8::MIN..=u8::MAX {
            let expected = byte.is_ascii_alphanumeric() || b"!#$&'()*+,-./:;=?@_~".contains(&byte);
            let reference = [b'/', b'a', byte];
            assert_eq!(is_simple_uri_path(&reference), expected, "byte {byte:#04x}");
        }
        assert!(is_simple_uri_path(b"/"));
        assert!(!is_simple_uri_path(b""));
        assert!(!is_simple_uri_path(b"//host/path"));
        assert!(!is_simple_uri_path(b"path"));
        assert!(is_simple_uri_path(b"/docs/index.html?x=1#top"));
        assert!(!is_simple_uri_path(b"/docs#one#two"));
        assert!(!is_simple_uri_path(b"/caf%C3%A9"));
    }

    #[test]
    fn simple_uri_boundaries_and_lanes() {
        for length in [16, 17, 31, 32, 33, 47, 48, 63, 64, 65] {
            let mut bytes = vec![b'a'; length];
            bytes[0] = b'/';
            assert!(is_simple_uri_path(&bytes), "valid length {length}");

            for lane in 1..length {
                let mut invalid = bytes.clone();
                invalid[lane] = b'%';
                assert!(!is_simple_uri_path(&invalid), "escape lane {lane}, length {length}");

                let mut fragment = bytes.clone();
                fragment[lane] = b'#';
                assert!(is_simple_uri_path(&fragment), "one hash at {lane}, length {length}");

                for other in 1..length {
                    if other == lane {
                        continue;
                    }
                    let mut twice = fragment.clone();
                    twice[other] = b'#';
                    assert!(!is_simple_uri_path(&twice), "hashes at {lane} and {other}, length {length}");
                }
            }
        }
    }

    #[test]
    fn simd_lanes_match_scalar_for_every_byte() {
        for length in [16, 17, 31, 32, 33, 48] {
            for lane in 0..length {
                for byte in lane_bytes(length) {
                    let mut bytes = vec![b'a'; length];
                    bytes[lane] = byte;
                    let context = format!("byte {byte:#04x} at lane {lane}, length {length}");
                    assert_eq!(is_token(&bytes), scalar::is_token(&bytes), "{context}");
                    assert_eq!(all_base64_alphabet(&bytes), base64::all_base64_alphabet(&bytes), "{context}");
                    assert_eq!(is_token68(&bytes), scalar::is_token68(&bytes), "{context}");
                    assert_eq!(is_field_value(&bytes), scalar::is_field_value(&bytes), "{context}");
                    assert_eq!(find_interesting(&bytes), scalar::find_interesting(&bytes), "{context}");
                }
            }
        }
    }

    #[test]
    fn simple_uri_lanes_match_scalar_for_every_byte() {
        for length in [16, 17, 23, 31, 32, 33, 48, 49] {
            for lane in 1..length {
                for byte in lane_bytes(length) {
                    let mut bytes = vec![b'a'; length];
                    bytes[0] = b'/';
                    bytes[lane] = byte;
                    assert_eq!(
                        is_simple_uri_path(&bytes),
                        uri::is_simple_uri_path(&bytes),
                        "byte {byte:#04x} at lane {lane}, length {length}"
                    );
                }
            }
        }
    }

    #[test]
    fn ascii_text_conversion_matches_the_general_validator() {
        for length in 0..=40 {
            let bytes = vec![b'a'; length];
            assert_eq!(ascii_str(&bytes), Some(str::from_utf8(&bytes).expect("the probe is ASCII")),);

            for lane in 0..length {
                for byte in [0x00_u8, 0x7f, 0x80, 0xc3, 0xff] {
                    let mut probe = bytes.clone();
                    probe[lane] = byte;
                    assert_eq!(
                        ascii_str(&probe).is_some(),
                        byte.is_ascii(),
                        "byte {byte:#04x} at lane {lane}, length {length}"
                    );
                }
            }
        }
        assert_eq!(ascii_str(b""), Some(""));
        assert_eq!(ascii_str("caf\u{e9}".as_bytes()), None);
    }

    #[test]
    fn simple_uri_reference_accepts_the_absolute_shape() {
        for reference in [
            "https://example.com",
            "https://example.com/",
            "https://example.com/a/b?c=d&e=f#g",
            "http://example.com:8080/a",
            "http://example.com:/a",
            "ws+tls-1.0://h0st.example/a",
            "https://example.com//double",
            "/origin/relative?x=1#top",
        ] {
            assert!(
                as_simple_uri_reference(reference.as_bytes()).is_some(),
                "{reference} must be recognized"
            );
        }

        for reference in [
            "",
            "relative",
            "//example.com/a",
            "https://user@example.com/a",
            "https://[::1]/a",
            "https:///a",
            "https://example.com:80x/a",
            "https://exa%6dple.com/a",
            "https://example.com/a%20b",
            "https://example.com/a#b#c",
            "1https://example.com/a",
            "https:/example.com/a",
            "https//example.com/a",
            "mailto:user@example.com",
        ] {
            assert!(
                as_simple_uri_reference(reference.as_bytes()).is_none(),
                "{reference} must fall back"
            );
        }
    }

    #[test]
    fn simple_uri_reference_lanes_match_scalar_for_every_byte() {
        let prefix = b"https://example.com";
        for length in [24, 32, 33, 48, 49] {
            for lane in 0..length {
                for byte in lane_bytes(length) {
                    let mut bytes = vec![b'a'; length];
                    bytes[..prefix.len()].copy_from_slice(prefix);
                    bytes[prefix.len()] = b'/';
                    bytes[lane] = byte;
                    assert_eq!(
                        as_simple_uri_reference(&bytes).is_some(),
                        uri::is_simple_uri_reference(&bytes),
                        "byte {byte:#04x} at lane {lane}, length {length}"
                    );
                }
            }
        }
    }

    #[test]
    fn ascii_case_equality_is_exhaustive() {
        for left in u8::MIN..=u8::MAX {
            for right in u8::MIN..=u8::MAX {
                assert_eq!(
                    eq_ignore_ascii_case(&[left], &[right]),
                    left.eq_ignore_ascii_case(&right),
                    "{left:#04x} versus {right:#04x}"
                );
            }
        }
        assert!(!eq_ignore_ascii_case(b"a", b"aa"));
    }

    #[test]
    fn boundary_lengths_match_scalar() {
        for length in 0..=65 {
            let mut bytes = [b'a'; 65];
            if length != 0 {
                bytes[length - 1] = b';';
            }
            let bytes = &bytes[..length];
            assert_eq!(is_token(bytes), scalar::is_token(bytes));
            assert_eq!(is_field_value(bytes), scalar::is_field_value(bytes));
            assert_eq!(eq_ignore_ascii_case(bytes, bytes), scalar::eq_ignore_ascii_case(bytes, bytes));
            assert_eq!(find_interesting(bytes), scalar::find_interesting(bytes));
            assert_eq!(is_simple_uri_path(bytes), uri::is_simple_uri_path(bytes));
        }
    }

    #[test]
    fn valid_tokens_at_vector_boundaries() {
        for length in [1, 15, 16, 17, 31, 32, 33, 47, 48, 63, 64, 65, 80] {
            let bytes = vec![b'a'; length];
            assert!(is_token(&bytes), "valid token length {length}");
        }
    }

    #[test]
    fn every_lane_is_checked_at_vector_boundaries() {
        for length in [16, 17, 31, 32, 33, 47, 48, 63, 64, 65] {
            for lane in 0..length {
                let mut bytes = vec![b'a'; length];
                bytes[lane] = b'(';
                assert!(!is_token(&bytes), "invalid lane {lane}, length {length}");

                bytes[lane] = b';';
                assert_eq!(find_interesting(&bytes), Some(lane), "interesting lane {lane}, length {length}");
            }
        }
    }

    #[test]
    fn token68_boundaries_and_lanes() {
        for length in [1, 15, 16, 17, 31, 32, 33, 47, 48, 63, 64, 65, 127, 128] {
            let valid = vec![b'a'; length];
            assert!(is_token68(&valid), "all-data length {length}");

            for lane in 0..length {
                let mut invalid = valid.clone();
                invalid[lane] = b':';
                assert!(!is_token68(&invalid), "invalid lane {lane}, length {length}");

                let mut padded = valid.clone();
                padded[lane..].fill(b'=');
                assert_eq!(is_token68(&padded), lane != 0, "padding lane {lane}, length {length}");

                if lane + 1 < length {
                    let next = lane + 1;
                    padded[next] = b'a';
                    assert!(!is_token68(&padded), "data after padding at lane {next}, length {length}");
                }
            }
        }
    }

    #[test]
    fn equal_length_ascii_case_folding_crosses_vector_boundaries() {
        let lower = b"abcdefghijklmnopqrstuvwxyz012345abcdefghijklmnopqrstuvwxyz012345";
        let upper = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ012345ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
        assert_eq!(lower.len(), 64);
        assert!(eq_ignore_ascii_case(lower, upper));

        for lane in 0..lower.len() {
            let mut different = *upper;
            different[lane] = if lower[lane].is_ascii_alphabetic() { b'0' } else { b'X' };
            assert!(!eq_ignore_ascii_case(lower, &different), "different lane {lane}");
        }

        let mut non_ascii_left = [b'a'; 32];
        let mut non_ascii_right = [b'A'; 32];
        non_ascii_left[16] = 0x80;
        non_ascii_right[16] = 0x80;
        assert!(eq_ignore_ascii_case(&non_ascii_left, &non_ascii_right));
        non_ascii_right[16] = 0x81;
        assert!(!eq_ignore_ascii_case(&non_ascii_left, &non_ascii_right));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Bolero corpus replay requires filesystem access unavailable under Miri isolation")]
    fn differential_properties() {
        bolero::check!()
            .with_iterations(4_096)
            .with_test_time(Duration::from_millis(400))
            .with_type::<(Vec<u8>, Vec<u8>)>()
            .for_each(|(left, right)| {
                assert_eq!(is_token(left), scalar::is_token(left));
                assert_eq!(is_token68(left), scalar::is_token68(left));
                assert_eq!(is_field_value(left), scalar::is_field_value(left));
                assert_eq!(eq_ignore_ascii_case(left, right), scalar::eq_ignore_ascii_case(left, right));
                assert_eq!(find_interesting(left), scalar::find_interesting(left));
                assert_eq!(is_simple_uri_path(left), uri::is_simple_uri_path(left));
                for empty in [EmptyMembers::Skip, EmptyMembers::Reject] {
                    let expected = list::oracle(left, empty);
                    assert_eq!(scan_token_list(left, empty), expected);
                    assert_eq!(list::scan_token_list(left, empty), expected);
                }
                for start in 0..=left.len() {
                    let scanned = scan_byte_range_set(left, start);
                    assert_eq!(scanned, range::scan_byte_range_set(left, start));
                    assert!(!scanned || range::oracle(&left[start..]));
                }
            });
    }

    #[cfg(any(feature = "benchmarking", feature = "test-util"))]
    #[test]
    fn backend_helpers_report_threshold_dispatch() {
        let expected_threshold = if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
            16
        } else {
            32
        };
        assert_eq!(simd_threshold(), expected_threshold);
        assert_eq!(backend_for(simd_threshold() - 1), Backend::Scalar);
        assert_eq!(backend(), backend_for(simd_threshold()));

        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            let selected = backend();
            assert!([Backend::Sse2, Backend::Sse42].contains(&selected));
            assert_eq!(selected == Backend::Sse42, arch::is_x86_feature_detected!("sse4.2"));
        }
    }

    /// Checks the accelerated range scanner against the scalar one.
    ///
    /// Every payload the kernels classify is compared against both the scalar
    /// classification and the parser-shaped oracle, at every payload offset a
    /// field line can hand over, and with the payload placed both inside a
    /// borrowed window and inside a padded one.
    #[test]
    fn range_scan_matches_scalar_for_every_short_payload() {
        let alphabet = b"0-, 1\t9";
        let mut line = Vec::new();
        for prefix in [&b"bytes="[..], b"b=", b"aaaaaaaaaaaaaaaabytes="] {
            for length in 0..=4_usize {
                for index in 0..alphabet.len().pow(u32::try_from(length).unwrap_or(0)) {
                    line.clear();
                    line.extend_from_slice(prefix);
                    let mut rest = index;
                    for _step in 0..length {
                        line.push(alphabet[rest % alphabet.len()]);
                        rest /= alphabet.len();
                    }
                    let start = prefix.len();
                    let scanned = scan_byte_range_set(&line, start);
                    assert_eq!(scanned, range::scan_byte_range_set(&line, start), "{line:?} at {start}");
                    if scanned {
                        assert!(range::oracle(&line[start..]), "{line:?} at {start}");
                    }
                }
            }
        }
    }

    /// Slides every short byte pattern across a block boundary.
    ///
    /// The scanners carry token/whitespace state and whether token or empty
    /// members have occurred across blocks. Patterns straddling a block
    /// exercise those transitions at every nearby offset.
    #[test]
    fn token_list_scan_matches_scalar_across_block_boundaries() {
        let alphabet = b"a,\t %";
        let mut pattern = Vec::new();
        slide_patterns(alphabet, 3, &mut pattern);
    }

    /// Puts every byte in every position of every length that leaves a tail.
    ///
    /// A line whose length is not a multiple of the block width is finished by
    /// reloading the last whole block and dropping the lanes already folded, so
    /// an off-by-one in that shift shows up as one byte in one position of one
    /// length disagreeing with the scalar state machine.
    #[test]
    fn token_list_tails_match_scalar_for_every_length_and_byte() {
        for length in 16..=33_usize {
            for lane in 0..length {
                for byte in lane_bytes(length) {
                    let mut input = vec![b'a'; length];
                    input[lane] = byte;
                    for empty in [EmptyMembers::Skip, EmptyMembers::Reject] {
                        let expected = list::oracle(&input, empty);
                        assert_eq!(
                            scan_token_list(&input, empty),
                            expected,
                            "length {length} lane {lane} byte {byte:#04x}"
                        );
                    }
                }
            }
        }
    }

    /// Walks every tail a well-formed list can end with, across two blocks.
    ///
    /// The tail carries the state the earlier blocks left, so the interesting
    /// cases are the ones where a member, a comma, or a whitespace run spans
    /// the boundary between the last whole block and the reloaded one.
    #[test]
    fn token_list_tails_match_scalar_for_every_short_ending() {
        let alphabet = b"a,\t x";
        for prefix in [&b"abc, def, ghij"[..], &b"a,b,c,d,e,f,g,h,i,j,k,l"[..], &b"  a  ,  b  ,  c  "[..]] {
            let mut ending = Vec::new();
            walk_endings(alphabet, 4, prefix, &mut ending);
        }
    }

    fn walk_endings(alphabet: &[u8], length: usize, prefix: &[u8], ending: &mut Vec<u8>) {
        if length == 0 {
            let mut input = prefix.to_vec();
            input.extend_from_slice(ending);
            for empty in [EmptyMembers::Skip, EmptyMembers::Reject] {
                let expected = list::oracle(&input, empty);
                assert_eq!(scan_token_list(&input, empty), expected, "{input:?}");
            }
            return;
        }
        for &byte in alphabet {
            ending.push(byte);
            walk_endings(alphabet, length - 1, prefix, ending);
            ending.pop();
        }
        walk_endings(alphabet, 0, prefix, ending);
    }

    fn slide_patterns(alphabet: &[u8], length: usize, pattern: &mut Vec<u8>) {
        if length == 0 {
            for &fill in b"a, \t" {
                for lead in 0..=17 {
                    let mut input = vec![fill; lead];
                    input.extend_from_slice(pattern);
                    input.resize(input.len() + 19, fill);
                    for empty in [EmptyMembers::Skip, EmptyMembers::Reject] {
                        let expected = list::oracle(&input, empty);
                        assert_eq!(scan_token_list(&input, empty), expected, "{input:?}");
                        assert_eq!(list::scan_token_list(&input, empty), expected, "{input:?}");
                    }
                }
            }
            return;
        }
        for &byte in alphabet {
            pattern.push(byte);
            slide_patterns(alphabet, length - 1, pattern);
            pattern.pop();
        }
        slide_patterns(alphabet, 0, pattern);
    }
}
