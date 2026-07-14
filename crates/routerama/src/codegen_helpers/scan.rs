// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Splits a request `path` into its path body and an optional trailing custom
/// `:verb` (the text after the final `:` of the last segment).
///
/// This is the allocation-free primitive that generated routers call before
/// scanning segments with [`scan_segments`]; the query string must already be
/// stripped by the caller.
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
// `mutants::skip`: the `+ 1` past the last `/` only moves where the `:` search
// starts; since `:` never matches `/`, it reaches the same colon either way.
#[cfg_attr(test, mutants::skip)]
pub fn split_verb(path: &str) -> (&str, Option<&str>) {
    let search_start = path.rfind('/').map_or(0, |i| i + 1);
    match path[search_start..].rfind(':') {
        Some(rel) => {
            let colon = search_start + rel;
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
/// This is the allocation-free core generated routers use to split a path into
/// fixed-size stack buffers sized to the deepest route known at build time.
/// Segment `i` (for `i` below the written count) is `&body[starts[i]..ends[i]]`.
///
/// A root path (`""` or `"/"`) has zero segments, and a trailing `/` yields a
/// trailing empty segment.
///
/// On `x86_64` the separator search uses SSE2 and on `aarch64` it uses NEON —
/// both are part of their respective 64-bit baselines, so no runtime feature
/// detection is needed; every other target uses the scalar implementation. All
/// produce identical output.
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

/// Returns `body[start..end]` as raw bytes, skipping the slice bounds check.
///
/// Generated routers use this to read a path segment for literal/affix matching.
/// It is `#[doc(hidden)]` — an internal codegen primitive, not a public API.
///
/// # Panics (debug only)
///
/// In debug builds a `debug_assert!` verifies `start <= end <= body.len()`.
#[doc(hidden)]
#[inline]
#[must_use]
pub fn seg_bytes(body: &str, start: usize, end: usize) -> &[u8] {
    debug_assert!(start <= end && end <= body.len(), "segment offsets out of range");
    // SAFETY: generated routers only pass offsets produced by `scan_segments`,
    // which guarantees `start <= end <= body.len()`, so the range is in bounds.
    unsafe { body.as_bytes().get_unchecked(start..end) }
}

/// Returns `body[start..end]`, skipping the slice bounds and UTF-8 char-boundary
/// checks.
///
/// Generated routers use this to bind a captured path variable to a `&str`. It
/// is `#[doc(hidden)]` — an internal codegen primitive, not a public API.
///
/// # Panics (debug only)
///
/// In debug builds `debug_assert!`s verify the range is in bounds and both ends
/// fall on UTF-8 char boundaries.
#[doc(hidden)]
#[inline]
#[must_use]
pub fn substr(body: &str, start: usize, end: usize) -> &str {
    debug_assert!(start <= end && end <= body.len(), "capture offsets out of range");
    debug_assert!(
        body.is_char_boundary(start) && body.is_char_boundary(end),
        "capture offsets are not char boundaries"
    );
    // SAFETY: generated routers only pass offsets produced by `scan_segments` /
    // `split_verb`. Segment boundaries fall on ASCII `/` (or the string end), so
    // they are valid UTF-8 char boundaries, and `start <= end <= body.len()`.
    unsafe { body.get_unchecked(start..end) }
}

/// Scalar reference implementation of [`scan_segments`].
#[cfg(any(all(not(target_arch = "x86_64"), not(target_arch = "aarch64")), test))]
#[cfg_attr(not(test), inline)]
fn scan_segments_scalar(body: &str, starts: &mut [usize], ends: &mut [usize]) -> usize {
    let rest = body.strip_prefix('/').unwrap_or(body);
    if rest.is_empty() {
        return 0;
    }

    let base = body.len() - rest.len();
    let cap = starts.len().min(ends.len());
    let mut count = 0_usize;
    let mut start = 0_usize;

    for (idx, byte) in rest.bytes().enumerate() {
        if byte == b'/' {
            if count < cap {
                starts[count] = base + start;
                ends[count] = base + idx;
            }
            count += 1;
            start = idx + 1;
        }
    }
    if count < cap {
        starts[count] = base + start;
        ends[count] = base + rest.len();
    }
    count + 1
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
// `mutants::skip`: SIMD index/mask mutants tend to infinite-loop rather than
// misbehave; correctness is covered differentially by `scan_segments_simd_matches_scalar`.
#[cfg_attr(test, mutants::skip)]
fn scan_segments_sse2(body: &str, starts: &mut [usize], ends: &mut [usize]) -> usize {
    use core::arch::x86_64::{_mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8};

    let rest = body.strip_prefix('/').unwrap_or(body);
    if rest.is_empty() {
        return 0;
    }

    let base = body.len() - rest.len();
    let bytes = rest.as_bytes();
    let n = bytes.len();
    let cap = starts.len().min(ends.len());
    let mut count = 0_usize;
    let mut seg_start = 0_usize;

    let mut i = 0_usize;
    while i + 16 <= n {
        // SAFETY: SSE2 is part of the `x86_64` baseline, so these intrinsics are
        // always available; and `i + 16 <= n`, so the 16-byte load reads only
        // bytes that lie within `bytes`.
        let mut mask = unsafe {
            let chunk = _mm_loadu_si128(bytes.as_ptr().add(i).cast());
            let eq = _mm_cmpeq_epi8(chunk, _mm_set1_epi8(b'/' as i8));
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
    // Tail: bytes in `[i, n)` remain when `n` is not a multiple of 16. When the
    // whole input is at least 16 bytes, one final *overlapping* 16-byte load at
    // `n - 16` covers them in a single vector op instead of a byte-by-byte scan;
    // separators at positions already processed by the main loop (`< i`) are
    // masked off. Only a sub-16-byte input — which cannot be loaded as a 16-byte
    // vector without over-reading — falls back to the scalar loop.
    if i < n {
        if n >= 16 {
            let load_at = n - 16;
            // SAFETY: `n >= 16`, so `load_at = n - 16` is in range and the 16-byte
            // load reads only bytes that lie within `bytes`.
            let raw = unsafe {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(load_at).cast());
                let eq = _mm_cmpeq_epi8(chunk, _mm_set1_epi8(b'/' as i8));
                _mm_movemask_epi8(eq) as u32
            };
            // The overlapping load re-covers positions `[load_at, i)`, already
            // handled by the main loop; clear their (low) mask bits so each
            // separator is recorded exactly once.
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
                }
                i += 1;
            }
        }
    }
    if count < cap {
        starts[count] = base + seg_start;
        ends[count] = base + n;
    }
    count + 1
}

/// NEON implementation of [`scan_segments`]: finds `/` separators 16 bytes at a
/// time, then handles any sub-16-byte tail with one overlapping final load (or a
/// scalar loop when the whole input is shorter than 16 bytes).
///
/// NEON has no direct `pmovmskb` equivalent, so the 16-lane `cmeq` result is
/// reduced to a 64-bit value with the standard "shift-right-narrow by 4" trick:
/// each input byte becomes a nibble (`0xF` when it matched `/`, `0x0`
/// otherwise), so a matched byte at index `k` sets bits `[4k, 4k+3]`. The lowest
/// match is then `trailing_zeros() >> 2`, and its nibble is cleared to advance.
#[cfg(target_arch = "aarch64")]
#[inline]
#[expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "the NEON separator search groups intrinsics that share one safety precondition"
)]
// `mutants::skip`: SIMD index/mask mutants tend to infinite-loop rather than
// misbehave; correctness is covered differentially by `scan_segments_simd_matches_scalar`.
#[cfg_attr(test, mutants::skip)]
fn scan_segments_neon(body: &str, starts: &mut [usize], ends: &mut [usize]) -> usize {
    use core::arch::aarch64::{vceqq_u8, vdupq_n_u8, vget_lane_u64, vld1q_u8, vreinterpret_u64_u8, vreinterpretq_u16_u8, vshrn_n_u16};

    let rest = body.strip_prefix('/').unwrap_or(body);
    if rest.is_empty() {
        return 0;
    }

    let base = body.len() - rest.len();
    let bytes = rest.as_bytes();
    let n = bytes.len();
    let cap = starts.len().min(ends.len());
    let mut count = 0_usize;
    let mut seg_start = 0_usize;

    let mut i = 0_usize;
    while i + 16 <= n {
        // SAFETY: NEON is part of the `aarch64` baseline, so these intrinsics are
        // always available; and `i + 16 <= n`, so the 16-byte load reads only
        // bytes that lie within `bytes`.
        let mut mask = unsafe {
            let chunk = vld1q_u8(bytes.as_ptr().add(i));
            let eq = vceqq_u8(chunk, vdupq_n_u8(b'/'));
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
    // Tail via one overlapping final load when the input is at least 16 bytes;
    // otherwise a scalar loop (see the SSE2 implementation for the rationale).
    if i < n {
        if n >= 16 {
            let load_at = n - 16;
            // SAFETY: `n >= 16`, so `load_at = n - 16` is in range and the 16-byte
            // load reads only bytes that lie within `bytes`.
            let raw = unsafe {
                let chunk = vld1q_u8(bytes.as_ptr().add(load_at));
                let eq = vceqq_u8(chunk, vdupq_n_u8(b'/'));
                let narrowed = vshrn_n_u16::<4>(vreinterpretq_u16_u8(eq));
                vget_lane_u64::<0>(vreinterpret_u64_u8(narrowed))
            };
            // Each matched byte occupies a nibble; clear the nibbles for positions
            // already processed by the main loop (`< i`, i.e. the low
            // `(i - load_at) * 4` bits) so each separator is recorded once.
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
                }
                i += 1;
            }
        }
    }
    if count < cap {
        starts[count] = base + seg_start;
        ends[count] = base + n;
    }
    count + 1
}

#[cfg(test)]
mod tests {
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
    fn scalar_scan_without_leading_slash_uses_a_zero_base() {
        // No leading `/`, so the base offset must be 0; a wrong base shifts every
        // segment slice.
        let mut starts = [0_usize; 4];
        let mut ends = [0_usize; 4];
        let count = scan_segments_scalar("a/bb", &mut starts, &mut ends);
        assert_eq!(count, 2);
        assert_eq!(&"a/bb"[starts[0]..ends[0]], "a");
        assert_eq!(&"a/bb"[starts[1]..ends[1]], "bb");
    }

    #[test]
    fn scalar_scan_never_writes_past_capacity() {
        // With fewer output slots than segments, the scanner still reports the
        // true total but records only the first `cap`. `/a/b/c` fills the array
        // exactly (post-loop boundary) and `/a/b/c/d` overflows it mid-loop.
        let mut starts = [0_usize; 2];
        let mut ends = [0_usize; 2];

        let count = scan_segments_scalar("/a/b/c", &mut starts, &mut ends);
        assert_eq!(count, 3);
        assert_eq!(&"/a/b/c"[starts[0]..ends[0]], "a");
        assert_eq!(&"/a/b/c"[starts[1]..ends[1]], "b");

        let count = scan_segments_scalar("/a/b/c/d", &mut starts, &mut ends);
        assert_eq!(count, 4);
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

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn scan_segments_simd_matches_scalar() {
        // Cover sub-16, exactly-16, and multi-chunk paths plus trailing slashes.
        let paths = [
            "/",
            "/v1",
            "/v1/users/octocat",
            "/v1/repos/rust-lang/cargo/issues/1347/comments/7",
            "/v1/repos/rust-lang/cargo/contents/a/b/c/d/e/f/g.rs",
            "/aaaaaaaaaaaaaaaa/b",
            "/a/bbbbbbbbbbbbbbbbbbbbbbbbbbbb/c/",
        ];
        for path in paths {
            let (mut ss, mut se) = ([0_usize; 24], [0_usize; 24]);
            let (mut vs, mut ve) = ([0_usize; 24], [0_usize; 24]);
            let scalar = scan_segments_scalar(path, &mut ss, &mut se);
            let simd = scan_segments(path, &mut vs, &mut ve);
            assert_eq!(scalar, simd, "count mismatch for {path:?}");
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

        // Exhaustively exercise the overlapping-final-load tail: for every total
        // length spanning the sub-16, one-chunk, and two-chunk regimes, place a
        // `/` at every possible position (and none), and check against the scalar
        // oracle.
        for len in 1..=40_usize {
            // No separators at all (single long segment).
            let no_sep: String = once('/').chain(repeat_n('a', len)).collect();
            assert_simd_eq(&no_sep);
            // A single separator at each interior position.
            for pos in 0..len {
                let mut bytes = vec![b'a'; len];
                bytes[pos] = b'/';
                let path: String = once('/').chain(bytes.iter().map(|&b| b as char)).collect();
                assert_simd_eq(&path);
            }
            // Two adjacent separators (empty middle segment) sweeping the tail.
            for pos in 0..len.saturating_sub(1) {
                let mut bytes = vec![b'a'; len];
                bytes[pos] = b'/';
                bytes[pos + 1] = b'/';
                let path: String = once('/').chain(bytes.iter().map(|&b| b as char)).collect();
                assert_simd_eq(&path);
            }
            // A trailing separator (empty final segment).
            let trailing: String = once('/').chain(repeat_n('a', len - 1)).chain(once('/')).collect();
            assert_simd_eq(&trailing);
        }
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    fn assert_simd_eq(path: &str) {
        let (mut ss, mut se) = ([0_usize; 48], [0_usize; 48]);
        let (mut vs, mut ve) = ([0_usize; 48], [0_usize; 48]);
        let scalar = scan_segments_scalar(path, &mut ss, &mut se);
        let simd = scan_segments(path, &mut vs, &mut ve);
        assert_eq!(scalar, simd, "count mismatch for {path:?}");
        assert_eq!(ss, vs, "starts mismatch for {path:?}");
        assert_eq!(se, ve, "ends mismatch for {path:?}");
    }
}
