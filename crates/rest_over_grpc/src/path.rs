// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Request path, verb, segment-scan, and query parsing helpers.

use crate::segments::Segments;

/// Splits a request `path` into its `/`-separated [`Segments`] and an optional
/// trailing custom `:verb`.
///
/// The verb, when present, is the text after the final `:` of the last path
/// segment. The query string (if any) must already be stripped by the caller.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::split_path;
///
/// let (segments, verb) = split_path("/shelves/7/books:list");
/// assert_eq!(segments.len(), 3);
/// assert_eq!(segments.get(1), Some("7"));
/// assert_eq!(verb, Some("list"));
/// ```
#[must_use]
pub fn split_path(path: &str) -> (Segments<'_>, Option<&str>) {
    let (body, verb) = split_verb(path);
    (Segments::new(body), verb)
}

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
/// use rest_over_grpc::split_verb;
///
/// assert_eq!(
///     split_verb("/shelves/7:archive"),
///     ("/shelves/7", Some("archive"))
/// );
/// assert_eq!(split_verb("/shelves/7:"), ("/shelves/7:", None));
/// ```
#[must_use]
pub fn split_verb(path: &str) -> (&str, Option<&str>) {
    let search_start = path.rfind('/').map_or(0, |i| i + 1);
    match path[search_start..].find(':') {
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
/// Semantics match [`Segments`]: a root path (`""` or `"/"`) has zero segments,
/// and a trailing `/` yields a trailing empty segment.
///
/// On `x86_64` the separator search uses SSE2 (part of the x86-64 baseline, so
/// no runtime feature detection is needed); every other target uses the scalar
/// implementation. Both produce identical output.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::scan_segments;
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
pub fn scan_segments(body: &str, starts: &mut [usize], ends: &mut [usize]) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        scan_segments_sse2(body, starts, ends)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        scan_segments_scalar(body, starts, ends)
    }
}

/// Scalar reference implementation of [`scan_segments`].
#[cfg(any(not(target_arch = "x86_64"), test))]
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
#[expect(
    clippy::multiple_unsafe_ops_per_block,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "the SSE2 separator search groups intrinsics that share one safety precondition and operates on i8 lanes by design"
)]
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
    if count < cap {
        starts[count] = base + seg_start;
        ends[count] = base + n;
    }
    count + 1
}

/// Splits a request path-and-query string into the path and the raw query
/// string (the part after the first `?`), if any.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::split_query;
///
/// assert_eq!(
///     split_query("/shelves/7?theme=history"),
///     ("/shelves/7", Some("theme=history"))
/// );
/// assert_eq!(split_query("/shelves/7"), ("/shelves/7", None));
/// ```
#[must_use]
pub fn split_query(target: &str) -> (&str, Option<&str>) {
    match target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (target, None),
    }
}

/// Parses a raw query string into `(key, value)` pairs.
///
/// Keys without `=` are treated as having an empty value. No percent-decoding
/// is performed; callers needing it should decode beforehand.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::parse_query;
///
/// let pairs = parse_query("theme=history&show_deleted&limit=10");
/// assert_eq!(
///     pairs,
///     vec![("theme", "history"), ("show_deleted", ""), ("limit", "10")]
/// );
/// ```
#[must_use]
pub fn parse_query(query: &str) -> Vec<(&str, &str)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (key, value),
            None => (pair, ""),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_root() {
        let (segs, verb) = split_path("/");
        assert_eq!(segs.len(), 0);
        assert!(segs.is_empty());
        assert!(verb.is_none());
    }

    #[test]
    fn splits_segments() {
        let (segs, verb) = split_path("/v1/shelves/3");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs.get(0), Some("v1"));
        assert_eq!(segs.get(1), Some("shelves"));
        assert_eq!(segs.get(2), Some("3"));
        assert_eq!(segs.get(3), None);
        assert!(verb.is_none());
    }

    #[test]
    fn splits_verb() {
        let (segs, verb) = split_path("/v1/shelves/3:read");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs.get(2), Some("3"));
        assert_eq!(verb, Some("read"));
    }

    #[test]
    fn trailing_colon_is_not_a_verb() {
        let (segs, verb) = split_path("/v1/x:");
        assert_eq!(verb, None);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs.get(1), Some("x:"));
    }

    #[test]
    fn splits_query() {
        assert_eq!(split_query("/v1/x?a=1&b=2"), ("/v1/x", Some("a=1&b=2")));
        assert_eq!(split_query("/v1/x"), ("/v1/x", None));
    }

    #[test]
    fn parses_query_pairs() {
        assert_eq!(parse_query("a=1&b=2&flag"), vec![("a", "1"), ("b", "2"), ("flag", "")]);
        assert_eq!(parse_query(""), Vec::<(&str, &str)>::new());
    }

    #[test]
    fn scan_segments_matches_segments_semantics() {
        for path in ["/", "", "/v1", "/v1/shelves/3", "/v1/shelves/3/", "/a/b/c/d/e"] {
            let (segs, _) = split_path(path);
            let mut starts = [0_usize; 16];
            let mut ends = [0_usize; 16];
            let count = scan_segments(path, &mut starts, &mut ends);
            assert_eq!(count, segs.len(), "count mismatch for {path:?}");
            for i in 0..count {
                assert_eq!(
                    &path[starts[i]..ends[i]],
                    segs.get(i).expect("segment"),
                    "segment {i} mismatch for {path:?}"
                );
            }
        }
    }

    #[test]
    fn scan_segments_reports_total_beyond_capacity() {
        let mut starts = [0_usize; 2];
        let mut ends = [0_usize; 2];
        let count = scan_segments("/a/b/c/d", &mut starts, &mut ends);
        assert_eq!(count, 4);
        // The first two segments are still written.
        assert_eq!(&"/a/b/c/d"[starts[0]..ends[0]], "a");
        assert_eq!(&"/a/b/c/d"[starts[1]..ends[1]], "b");
    }

    #[cfg(target_arch = "x86_64")]
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
}
