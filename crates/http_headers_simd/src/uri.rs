// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Scalar URI-reference subset classification used as the SIMD oracle.

/// Bytes needing no percent-decoding in a path, query, or fragment.
///
/// This is `unreserved`, `sub-delims`, and `:`, `@`, `/`, `?`, `#`; `%` is
/// omitted so escapes fall back to full parsing.
///
/// [RFC 3986 section 2]: https://www.rfc-editor.org/rfc/rfc3986#section-2
const SIMPLE_URI_BITMAP: [u64; 4] = [0xafff_ffda_0000_0000, 0x47ff_fffe_87ff_ffff, 0, 0];

/// Returns whether one byte belongs to the separator-free URI subset.
pub(super) fn is_simple_uri_byte(byte: u8) -> bool {
    let word = SIMPLE_URI_BITMAP[usize::from(byte >> 6)];
    word & (1_u64 << (byte & 63)) != 0
}

/// Returns whether `bytes` may open an origin-relative reference.
///
/// Rejects `//authority` because its host follows a different grammar.
pub(super) fn has_origin_relative_prefix(bytes: &[u8]) -> bool {
    bytes.first() == Some(&b'/') && bytes.get(1) != Some(&b'/')
}

/// Checks the remaining bytes of a reference whose prefix already matched.
///
/// `hashes` counts accepted `#` bytes; at most one may appear overall.
pub(super) fn simple_uri_tail(bytes: &[u8], hashes: u32) -> bool {
    let mut seen = hashes;
    for &byte in bytes {
        if !is_simple_uri_byte(byte) {
            return false;
        }
        if byte == b'#' {
            if seen != 0 {
                return false;
            }
            seen = 1;
        }
    }
    true
}

/// Bytes that may appear in a `reg-name` host.
///
/// This is `unreserved` plus `sub-delims`; `%`, `:`, `@`, and `[` are omitted
/// so escapes, ports, userinfo, and IP literals are recognized positionally.
///
/// [RFC 3986 section 2]: https://www.rfc-editor.org/rfc/rfc3986#section-2
const HOST_BITMAP: [u64; 4] = [0x2bff_7fd2_0000_0000, 0x47ff_fffe_87ff_fffe, 0, 0];

/// Returns whether one byte belongs to the escape-free `reg-name` subset.
fn is_host_byte(byte: u8) -> bool {
    let word = HOST_BITMAP[usize::from(byte >> 6)];
    word & (1_u64 << (byte & 63)) != 0
}

/// Returns the offset of the path in a `scheme "://" host [":" port]` prefix.
///
/// `None` leaves userinfo, IP literals, percent escapes, and empty hosts to the
/// full parser. Bytes after the returned offset use origin-relative rules.
pub(super) fn simple_authority_end(bytes: &[u8]) -> Option<usize> {
    if !bytes.first()?.is_ascii_alphabetic() {
        return None;
    }
    let mut index = 1;
    while let Some(&byte) = bytes.get(index) {
        if byte == b':' {
            break;
        }
        if !byte.is_ascii_alphanumeric() && !matches!(byte, b'+' | b'-' | b'.') {
            return None;
        }
        index += 1;
    }
    if bytes.get(index..index.checked_add(3)?) != Some(b"://".as_slice()) {
        return None;
    }
    index += 3;

    let host_start = index;
    while bytes.get(index).copied().is_some_and(is_host_byte) {
        index += 1;
    }
    if index == host_start {
        return None;
    }
    if bytes.get(index) == Some(&b':') {
        index += 1;
        while bytes.get(index).copied().is_some_and(|byte| byte.is_ascii_digit()) {
            index += 1;
        }
    }
    match bytes.get(index) {
        None | Some(b'/' | b'?' | b'#') => Some(index),
        Some(_) => None,
    }
}

/// Runs the whole check without any architecture-specific acceleration.
#[cfg(any(feature = "benchmarking", test))]
pub(super) fn is_simple_uri_path(bytes: &[u8]) -> bool {
    has_origin_relative_prefix(bytes) && simple_uri_tail(bytes, 0)
}

/// Runs the whole reference check without architecture-specific acceleration.
#[cfg(test)]
pub(super) fn is_simple_uri_reference(bytes: &[u8]) -> bool {
    if has_origin_relative_prefix(bytes) {
        return simple_uri_tail(bytes, 0);
    }
    match simple_authority_end(bytes) {
        Some(end) => simple_uri_tail(&bytes[end..], 0),
        None => false,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn origin_relative_prefix_excludes_authorities_and_relative_paths() {
        assert!(has_origin_relative_prefix(b"/"));
        assert!(has_origin_relative_prefix(b"/path"));
        assert!(!has_origin_relative_prefix(b""));
        assert!(!has_origin_relative_prefix(b"path"));
        assert!(!has_origin_relative_prefix(b"//example.com/path"));
    }

    #[test]
    fn tail_rejects_escapes_controls_and_multiple_fragments() {
        assert!(simple_uri_tail(b"/a/b?x=1#top", 0));
        assert!(simple_uri_tail(b"continuation", 1));
        assert!(!simple_uri_tail(b"#second", 1));
        assert!(!simple_uri_tail(b"/percent%20escape", 0));
        assert!(!simple_uri_tail(b"/control\n", 0));
        assert!(is_simple_uri_path(b"/docs/index.html?x=1#top"));
        assert!(!is_simple_uri_path(b"/docs#one#two"));
    }
}
