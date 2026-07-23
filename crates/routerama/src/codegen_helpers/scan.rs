// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Splits a request `path` into its path body and an optional trailing custom
/// `:verb` (the text after the final `:` of the last segment).
///
/// The query string must already be stripped.
///
/// # Examples
///
/// ```
/// use routerama::codegen_helpers::split_verb;
///
/// assert_eq!(
///     split_verb("/shelves/7:archive"),
///     ("/shelves/7", Some("archive"))
/// );
/// assert_eq!(split_verb("/shelves/7:"), ("/shelves/7:", None));
/// // The verb is the suffix after the *final* colon, so a colon inside the
/// // last segment stays part of the path body.
/// assert_eq!(
///     split_verb("/shelves/a:b:archive"),
///     ("/shelves/a:b", Some("archive"))
/// );
/// ```
#[must_use]
#[inline]
pub fn split_verb(path: &str) -> (&str, Option<&str>) {
    match path.as_bytes().iter().rposition(|&byte| matches!(byte, b':' | b'/' | b'?' | b'#')) {
        Some(colon) => {
            if path.as_bytes()[colon] != b':' {
                return (path, None);
            }
            let verb = &path[colon + 1..];
            if verb.is_empty() {
                (path, None)
            } else {
                (&path[..colon], Some(verb))
            }
        }
        None => (path, None),
    }
}

/// Scans `body` into the byte offsets of its `/`-separated segments.
///
/// Writes up to `starts.len().min(ends.len())` segment `(start, end)` pairs and
/// returns the *total* segment count (which may exceed the buffers' capacity).
///
/// Segment `i` (for `i` below the written count) is `&body[starts[i]..ends[i]]`.
///
/// A root path (`""` or `"/"`) has zero segments, and a trailing `/` yields a
/// trailing empty segment.
///
/// The separator search uses baseline SSE2 on `x86_64`, baseline NEON on
/// `aarch64`, and a scalar implementation elsewhere.
///
/// # Examples
///
/// ```
/// use routerama::codegen_helpers::scan_segments;
///
/// let path = "/shelves/7/books";
/// let mut starts = [0_usize; 4];
/// let mut ends = [0_usize; 4];
///
/// let count = scan_segments(path, &mut starts, &mut ends);
/// assert_eq!(count, 3);
/// assert_eq!(&path[starts[0]..ends[0]], "shelves");
/// assert_eq!(&path[starts[1]..ends[1]], "7");
/// assert_eq!(&path[starts[2]..ends[2]], "books");
/// ```
#[must_use]
#[inline]
pub fn scan_segments(body: &str, starts: &mut [usize], ends: &mut [usize]) -> usize {
    scan_segments_checked(body, starts, ends).count
}

pub(super) struct ScanResult {
    pub(super) count: usize,
    pub(super) valid: bool,
}

#[inline]
pub(super) fn scan_segments_checked(body: &str, starts: &mut [usize], ends: &mut [usize]) -> ScanResult {
    #[cfg(target_arch = "x86_64")]
    {
        scan_segments_sse2(body, starts, ends)
    }
    #[cfg(target_arch = "aarch64")]
    {
        scan_segments_neon(body, starts, ends)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        scan_segments_scalar(body, starts, ends)
    }
}

/// Returns `body[start..end]` as raw bytes.
///
/// Generated routers use this to read a path segment for literal/affix matching.
/// It is `#[doc(hidden)]` — an internal codegen primitive, not a public API.
///
/// # Panics
///
/// Panics if `start..end` is not a valid range within `body`.
#[doc(hidden)]
#[inline]
#[must_use]
pub fn seg_bytes(body: &str, start: usize, end: usize) -> &[u8] {
    body.as_bytes()
        .get(start..end)
        .expect("segment start and end must delimit a byte range within the path")
}

/// Returns `body[start..end]`.
///
/// Generated routers use this to bind a captured path variable to a `&str`. It
/// is `#[doc(hidden)]` — an internal codegen primitive, not a public API.
///
/// # Panics
///
/// Panics if the range is out of bounds or either offset is not a UTF-8
/// character boundary.
#[doc(hidden)]
#[inline]
#[must_use]
pub fn substr(body: &str, start: usize, end: usize) -> &str {
    body.get(start..end)
        .expect("capture start and end must delimit UTF-8 character boundaries within the path")
}

/// Scalar reference implementation of [`scan_segments`].
#[cfg(any(all(not(target_arch = "x86_64"), not(target_arch = "aarch64")), test))]
#[cfg_attr(not(test), inline)]
fn scan_segments_scalar(body: &str, starts: &mut [usize], ends: &mut [usize]) -> ScanResult {
    let rest = body.strip_prefix('/').unwrap_or(body);
    if rest.is_empty() {
        return ScanResult { count: 0, valid: true };
    }

    let base = body.len() - rest.len();
    let cap = starts.len().min(ends.len());
    let mut count = 0_usize;
    let mut start = 0_usize;
    let mut valid = true;

    for (idx, byte) in rest.bytes().enumerate() {
        if byte == b'/' {
            if count < cap {
                starts[count] = base + start;
                ends[count] = base + idx;
            }
            count += 1;
            start = idx + 1;
        } else if matches!(byte, b'?' | b'#') {
            valid = false;
        }
    }
    if count < cap {
        starts[count] = base + start;
        ends[count] = base + rest.len();
    }
    ScanResult { count: count + 1, valid }
}

/// SSE2 implementation of [`scan_segments`]: finds `/` separators 16 bytes at a
/// time via `pcmpeqb` + `pmovmskb`, then handles the sub-16-byte tail with a
/// scalar loop.
#[cfg(target_arch = "x86_64")]
#[inline]
#[expect(
    clippy::multiple_unsafe_ops_per_block,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "the SSE2 separator search groups intrinsics that share one safety precondition and operates on i8 lanes by design"
)]
// SIMD correctness is covered differentially by `scan_segments_simd_matches_scalar`.
#[cfg_attr(test, mutants::skip)]
fn scan_segments_sse2(body: &str, starts: &mut [usize], ends: &mut [usize]) -> ScanResult {
    use core::arch::x86_64::{_mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_or_si128, _mm_set1_epi8};

    let rest = body.strip_prefix('/').unwrap_or(body);
    if rest.is_empty() {
        return ScanResult { count: 0, valid: true };
    }

    let base = body.len() - rest.len();
    let bytes = rest.as_bytes();
    let n = bytes.len();
    let cap = starts.len().min(ends.len());
    let mut count = 0_usize;
    let mut seg_start = 0_usize;
    let mut valid = true;

    let mut i = 0_usize;
    while i + 16 <= n {
        // SAFETY: SSE2 is part of the `x86_64` baseline, so these intrinsics are
        // always available; and `i + 16 <= n`, so the 16-byte load reads only
        // bytes that lie within `bytes`.
        let mut mask = unsafe {
            let chunk = _mm_loadu_si128(bytes.as_ptr().add(i).cast());
            let eq = _mm_cmpeq_epi8(chunk, _mm_set1_epi8(b'/' as i8));
            let forbidden = _mm_or_si128(
                _mm_cmpeq_epi8(chunk, _mm_set1_epi8(b'?' as i8)),
                _mm_cmpeq_epi8(chunk, _mm_set1_epi8(b'#' as i8)),
            );
            valid &= _mm_movemask_epi8(forbidden) == 0;
            _mm_movemask_epi8(eq) as u32
        };
        while mask != 0 {
            let idx = i + mask.trailing_zeros() as usize;
            if count < cap {
                starts[count] = base + seg_start;
                ends[count] = base + idx;
            }
            count += 1;
            seg_start = idx + 1;
            mask &= mask - 1;
        }
        i += 16;
    }
    // Scan the tail with an overlapping load, masking bytes already processed.
    // Inputs shorter than one vector use the scalar loop.
    if i < n {
        if n >= 16 {
            let load_at = n - 16;
            // SAFETY: `n >= 16`, so `load_at = n - 16` is in range and the 16-byte
            // load reads only bytes that lie within `bytes`.
            let raw = unsafe {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(load_at).cast());
                let eq = _mm_cmpeq_epi8(chunk, _mm_set1_epi8(b'/' as i8));
                let forbidden = _mm_or_si128(
                    _mm_cmpeq_epi8(chunk, _mm_set1_epi8(b'?' as i8)),
                    _mm_cmpeq_epi8(chunk, _mm_set1_epi8(b'#' as i8)),
                );
                valid &= _mm_movemask_epi8(forbidden) == 0;
                _mm_movemask_epi8(eq) as u32
            };
            // Clear mask bits for positions already processed by the main loop.
            let skip = i - load_at;
            let mut mask = raw & !((1_u32 << skip) - 1);
            while mask != 0 {
                let idx = load_at + mask.trailing_zeros() as usize;
                if count < cap {
                    starts[count] = base + seg_start;
                    ends[count] = base + idx;
                }
                count += 1;
                seg_start = idx + 1;
                mask &= mask - 1;
            }
        } else {
            while i < n {
                if bytes[i] == b'/' {
                    if count < cap {
                        starts[count] = base + seg_start;
                        ends[count] = base + i;
                    }
                    count += 1;
                    seg_start = i + 1;
                } else if matches!(bytes[i], b'?' | b'#') {
                    valid = false;
                }
                i += 1;
            }
        }
    }
    if count < cap {
        starts[count] = base + seg_start;
        ends[count] = base + n;
    }
    ScanResult { count: count + 1, valid }
}

/// NEON implementation of [`scan_segments`].
///
/// Each comparison lane is narrowed to a nibble, allowing matching byte
/// positions to be found with `trailing_zeros`.
#[cfg(target_arch = "aarch64")]
#[inline]
#[expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "the NEON separator search groups intrinsics that share one safety precondition"
)]
// SIMD correctness is covered differentially by `scan_segments_simd_matches_scalar`.
#[cfg_attr(test, mutants::skip)]
fn scan_segments_neon(body: &str, starts: &mut [usize], ends: &mut [usize]) -> ScanResult {
    use core::arch::aarch64::{
        vceqq_u8, vdupq_n_u8, vget_lane_u64, vld1q_u8, vorrq_u8, vreinterpret_u64_u8, vreinterpretq_u16_u8, vshrn_n_u16,
    };

    let rest = body.strip_prefix('/').unwrap_or(body);
    if rest.is_empty() {
        return ScanResult { count: 0, valid: true };
    }

    let base = body.len() - rest.len();
    let bytes = rest.as_bytes();
    let n = bytes.len();
    let cap = starts.len().min(ends.len());
    let mut count = 0_usize;
    let mut seg_start = 0_usize;
    let mut valid = true;

    let mut i = 0_usize;
    while i + 16 <= n {
        // SAFETY: NEON is part of the `aarch64` baseline, so these intrinsics are
        // always available; and `i + 16 <= n`, so the 16-byte load reads only
        // bytes that lie within `bytes`.
        let mut mask = unsafe {
            let chunk = vld1q_u8(bytes.as_ptr().add(i));
            let eq = vceqq_u8(chunk, vdupq_n_u8(b'/'));
            let forbidden = vorrq_u8(vceqq_u8(chunk, vdupq_n_u8(b'?')), vceqq_u8(chunk, vdupq_n_u8(b'#')));
            let forbidden_narrowed = vshrn_n_u16::<4>(vreinterpretq_u16_u8(forbidden));
            valid &= vget_lane_u64::<0>(vreinterpret_u64_u8(forbidden_narrowed)) == 0;
            // Narrow each 16-bit lane to a nibble-per-byte mask (`0xF` per match).
            let narrowed = vshrn_n_u16::<4>(vreinterpretq_u16_u8(eq));
            vget_lane_u64::<0>(vreinterpret_u64_u8(narrowed))
        };
        while mask != 0 {
            // Each matched byte occupies a nibble, so its index is the lowest set
            // bit divided by four; clearing that whole nibble advances to the next.
            let tz = mask.trailing_zeros();
            let idx = i + (tz >> 2) as usize;
            if count < cap {
                starts[count] = base + seg_start;
                ends[count] = base + idx;
            }
            count += 1;
            seg_start = idx + 1;
            mask &= !(0xF_u64 << tz);
        }
        i += 16;
    }
    // Use an overlapping final load, or the scalar loop for short inputs.
    if i < n {
        if n >= 16 {
            let load_at = n - 16;
            // SAFETY: `n >= 16`, so `load_at = n - 16` is in range and the 16-byte
            // load reads only bytes that lie within `bytes`.
            let raw = unsafe {
                let chunk = vld1q_u8(bytes.as_ptr().add(load_at));
                let eq = vceqq_u8(chunk, vdupq_n_u8(b'/'));
                let forbidden = vorrq_u8(vceqq_u8(chunk, vdupq_n_u8(b'?')), vceqq_u8(chunk, vdupq_n_u8(b'#')));
                let forbidden_narrowed = vshrn_n_u16::<4>(vreinterpretq_u16_u8(forbidden));
                valid &= vget_lane_u64::<0>(vreinterpret_u64_u8(forbidden_narrowed)) == 0;
                let narrowed = vshrn_n_u16::<4>(vreinterpretq_u16_u8(eq));
                vget_lane_u64::<0>(vreinterpret_u64_u8(narrowed))
            };
            // Clear nibbles for positions already processed by the main loop.
            let skip_nibbles = (i - load_at) * 4;
            let mut mask = raw & !((1_u64 << skip_nibbles) - 1);
            while mask != 0 {
                let tz = mask.trailing_zeros();
                let idx = load_at + (tz >> 2) as usize;
                if count < cap {
                    starts[count] = base + seg_start;
                    ends[count] = base + idx;
                }
                count += 1;
                seg_start = idx + 1;
                mask &= !(0xF_u64 << tz);
            }
        } else {
            while i < n {
                if bytes[i] == b'/' {
                    if count < cap {
                        starts[count] = base + seg_start;
                        ends[count] = base + i;
                    }
                    count += 1;
                    seg_start = i + 1;
                } else if matches!(bytes[i], b'?' | b'#') {
                    valid = false;
                }
                i += 1;
            }
        }
    }
    if count < cap {
        starts[count] = base + seg_start;
        ends[count] = base + n;
    }
    ScanResult { count: count + 1, valid }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use super::*;

    #[test]
    fn verb_is_the_suffix_after_the_final_colon() {
        assert_eq!(split_verb("/v1/shelves/a:b:archive"), ("/v1/shelves/a:b", Some("archive")));
        assert_eq!(split_verb("/v1/shelves/3:read"), ("/v1/shelves/3", Some("read")));
    }

    #[test]
    fn trailing_colon_is_not_a_verb() {
        assert_eq!(split_verb("/v1/x:"), ("/v1/x:", None));
        assert_eq!(split_verb("/v1/x"), ("/v1/x", None));
    }

    #[test]
    fn query_and_fragment_delimiters_are_not_hidden_in_verbs() {
        assert_eq!(split_verb("/jobs/7:cancel?force"), ("/jobs/7:cancel?force", None));
        assert_eq!(split_verb("/jobs/7:cancel#result"), ("/jobs/7:cancel#result", None));
    }

    #[test]
    fn scalar_scan_without_leading_slash_uses_a_zero_base() {
        let mut starts = [0_usize; 4];
        let mut ends = [0_usize; 4];
        let result = scan_segments_scalar("a/bb", &mut starts, &mut ends);
        assert_eq!(result.count, 2);
        assert_eq!(&"a/bb"[starts[0]..ends[0]], "a");
        assert_eq!(&"a/bb"[starts[1]..ends[1]], "bb");
    }

    #[test]
    fn scalar_scan_never_writes_past_capacity() {
        let mut starts = [0_usize; 2];
        let mut ends = [0_usize; 2];

        let result = scan_segments_scalar("/a/b/c", &mut starts, &mut ends);
        assert_eq!(result.count, 3);
        assert_eq!(&"/a/b/c"[starts[0]..ends[0]], "a");
        assert_eq!(&"/a/b/c"[starts[1]..ends[1]], "b");

        let result = scan_segments_scalar("/a/b/c/d", &mut starts, &mut ends);
        assert_eq!(result.count, 4);
        assert_eq!(&"/a/b/c/d"[starts[0]..ends[0]], "a");
        assert_eq!(&"/a/b/c/d"[starts[1]..ends[1]], "b");
    }

    #[test]
    fn scan_reports_total_beyond_capacity() {
        let mut starts = [0_usize; 2];
        let mut ends = [0_usize; 2];
        let count = scan_segments("/a/b/c/d", &mut starts, &mut ends);
        assert_eq!(count, 4);
        assert_eq!(&"/a/b/c/d"[starts[0]..ends[0]], "a");
        assert_eq!(&"/a/b/c/d"[starts[1]..ends[1]], "b");
    }

    #[test]
    fn root_and_empty_have_zero_segments() {
        let mut starts = [0_usize; 4];
        let mut ends = [0_usize; 4];
        assert_eq!(scan_segments("/", &mut starts, &mut ends), 0);
        assert_eq!(scan_segments("", &mut starts, &mut ends), 0);
    }

    #[test]
    fn trailing_slash_yields_a_trailing_empty_segment() {
        let mut starts = [0_usize; 4];
        let mut ends = [0_usize; 4];
        let count = scan_segments("/a/", &mut starts, &mut ends);
        assert_eq!(count, 2);
        assert_eq!(&"/a/"[starts[1]..ends[1]], "");
    }

    #[test]
    fn checked_scan_rejects_query_and_fragment_delimiters() {
        let mut starts = [0_usize; 4];
        let mut ends = [0_usize; 4];
        assert!(!scan_segments_checked("/a?query", &mut starts, &mut ends).valid);
        assert!(!scan_segments_checked("/a#fragment", &mut starts, &mut ends).valid);
        assert!(scan_segments_checked("/a%3Fquery", &mut starts, &mut ends).valid);
    }

    #[test]
    fn slicing_helpers_reject_invalid_ranges() {
        let _ = std::panic::catch_unwind(|| seg_bytes("abc", 2, 4)).expect_err("out-of-bounds byte range must panic");
        let _ = std::panic::catch_unwind(|| seg_bytes("abc", 2, 1)).expect_err("reversed byte range must panic");
        let _ = std::panic::catch_unwind(|| substr("\u{e9}", 1, 2)).expect_err("non-character boundary must panic");
        let _ = std::panic::catch_unwind(|| substr("abc", 2, 4)).expect_err("out-of-bounds string range must panic");
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn scan_segments_simd_matches_scalar() {
        let paths = [
            "/",
            "/v1",
            "/v1/users/octocat",
            "/v1/repos/rust-lang/cargo/issues/1347/comments/7",
            "/v1/repos/rust-lang/cargo/contents/a/b/c/d/e/f/g.rs",
            "/aaaaaaaaaaaaaaaa/b",
            "/a/bbbbbbbbbbbbbbbbbbbbbbbbbbbb/c/",
            "/with?query",
            "/with#fragment",
            "/aaaaaaaaaaaaaaaaaaaaaaaa?query",
            "/aaaaaaaaaaaaaaaaaaaaaaaa#fragment",
        ];
        for path in paths {
            let (mut ss, mut se) = ([0_usize; 24], [0_usize; 24]);
            let (mut vs, mut ve) = ([0_usize; 24], [0_usize; 24]);
            let scalar = scan_segments_scalar(path, &mut ss, &mut se);
            let simd = scan_segments_checked(path, &mut vs, &mut ve);
            assert_eq!(scalar.count, simd.count, "count mismatch for {path:?}");
            assert_eq!(scalar.valid, simd.valid, "validity mismatch for {path:?}");
            assert_eq!(ss, vs, "starts mismatch for {path:?}");
            assert_eq!(se, ve, "ends mismatch for {path:?}");
        }
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[cfg_attr(
        miri,
        ignore = "exhaustive SIMD sweep is too slow under Miri; scan_segments_simd_matches_scalar covers the intrinsics"
    )]
    #[test]
    fn scan_segments_simd_matches_scalar_exhaustive_tail() {
        use core::iter::{once, repeat_n};

        for len in 1..=40_usize {
            let no_sep: String = once('/').chain(repeat_n('a', len)).collect();
            assert_simd_eq(&no_sep);
            for pos in 0..len {
                let mut bytes = vec![b'a'; len];
                bytes[pos] = b'/';
                let path: String = once('/').chain(bytes.iter().map(|&b| b as char)).collect();
                assert_simd_eq(&path);
            }
            for pos in 0..len.saturating_sub(1) {
                let mut bytes = vec![b'a'; len];
                bytes[pos] = b'/';
                bytes[pos + 1] = b'/';
                let path: String = once('/').chain(bytes.iter().map(|&b| b as char)).collect();
                assert_simd_eq(&path);
            }
            let trailing: String = once('/').chain(repeat_n('a', len - 1)).chain(once('/')).collect();
            assert_simd_eq(&trailing);
        }
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    fn assert_simd_eq(path: &str) {
        let (mut ss, mut se) = ([0_usize; 48], [0_usize; 48]);
        let (mut vs, mut ve) = ([0_usize; 48], [0_usize; 48]);
        let scalar = scan_segments_scalar(path, &mut ss, &mut se);
        let simd = scan_segments_checked(path, &mut vs, &mut ve);
        assert_eq!(scalar.count, simd.count, "count mismatch for {path:?}");
        assert_eq!(scalar.valid, simd.valid, "validity mismatch for {path:?}");
        assert_eq!(ss, vs, "starts mismatch for {path:?}");
        assert_eq!(se, ve, "ends mismatch for {path:?}");
    }
}
