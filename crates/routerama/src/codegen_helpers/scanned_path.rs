// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::vec;
use core::fmt;

use super::scan::scan_segments_checked;

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPath;

/// A request path and the segment offsets produced by [`scan_segments`].
///
/// The private fields preserve the relationship between the path and its
/// offsets, allowing generated resolvers to access segments and captures
/// without emitting unsafe code.
#[doc(hidden)]
pub struct ScannedPath<'p, 's> {
    body: &'p str,
    starts: &'s [usize],
    ends: &'s [usize],
    capacity: usize,
    count: usize,
    valid: bool,
}

impl<'p> ScannedPath<'p, '_> {
    /// Returns the total number of path segments.
    #[must_use]
    #[inline]
    pub const fn count(&self) -> usize {
        self.count
    }

    #[must_use]
    #[inline]
    pub const fn is_valid(&self) -> bool {
        self.valid
    }

    /// Returns one scanned segment.
    #[must_use]
    #[inline]
    // Guard mutations can invoke unchecked indexing with invalid synthetic
    // offsets; valid and invalid ranges are covered directly below.
    #[cfg_attr(test, mutants::skip)]
    pub fn segment(&self, index: usize) -> Option<&'p str> {
        if index >= self.count || index >= self.capacity {
            return None;
        }
        // SAFETY: the explicit length checks above cover the starts buffer.
        let start = unsafe { *self.starts.get_unchecked(index) };
        // SAFETY: the explicit length checks above cover the ends buffer.
        let end = unsafe { *self.ends.get_unchecked(index) };
        // SAFETY: `scan_segments` produced these ordered offsets from `body`;
        // ASCII separators and string endpoints are UTF-8 boundaries.
        Some(unsafe { self.body.get_unchecked(start..end) })
    }

    /// Returns a capture spanning the inclusive segment range `first..=last`.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)]
    pub fn capture(&self, first: usize, last: usize) -> Option<&'p str> {
        if first > last || last >= self.count || last >= self.capacity {
            return None;
        }
        // SAFETY: the explicit length checks above cover the starts buffer.
        let start = unsafe { *self.starts.get_unchecked(first) };
        // SAFETY: the explicit length checks above cover the ends buffer.
        let end = unsafe { *self.ends.get_unchecked(last) };
        // SAFETY: both offsets came from this scan, and ordered segment indices
        // imply an ordered range with UTF-8-boundary endpoints.
        Some(unsafe { self.body.get_unchecked(start..end) })
    }

    /// Returns a capture from `first` through the end of the path.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)]
    pub fn rest(&self, first: usize) -> Option<&'p str> {
        if first == self.count {
            return Some("");
        }
        if first > self.count {
            return None;
        }
        if first >= self.capacity {
            return None;
        }
        // SAFETY: `first` was checked against the starts buffer length.
        let start = unsafe { *self.starts.get_unchecked(first) };
        // SAFETY: the start offset came from this scan and is therefore an
        // in-bounds UTF-8 boundary; the string end is also a boundary.
        Some(unsafe { self.body.get_unchecked(start..) })
    }

    /// Returns a segment capture after removing literal prefix and suffix bytes.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)]
    pub fn affix(&self, index: usize, prefix_len: usize, suffix_len: usize) -> Option<&'p str> {
        if index >= self.count || index >= self.capacity {
            return None;
        }
        // SAFETY: the explicit length checks above cover the starts buffer.
        let segment_start = unsafe { *self.starts.get_unchecked(index) };
        // SAFETY: the explicit length checks above cover the ends buffer.
        let segment_end = unsafe { *self.ends.get_unchecked(index) };
        let start = segment_start.checked_add(prefix_len)?;
        let end = segment_end.checked_sub(suffix_len)?;
        if start > end || !self.body.is_char_boundary(start) || !self.body.is_char_boundary(end) {
            return None;
        }
        // SAFETY: bounds and UTF-8 boundaries were checked above.
        Some(unsafe { self.body.get_unchecked(start..end) })
    }
}

impl fmt::Debug for ScannedPath<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScannedPath")
            .field("body", &self.body)
            .field("count", &self.count)
            .finish_non_exhaustive()
    }
}

/// Scans `body` and returns an opaque view tying it to the resulting offsets.
#[doc(hidden)]
#[must_use]
#[inline]
pub fn scan_path<'p, 's>(body: &'p str, starts: &'s mut [usize], ends: &'s mut [usize]) -> ScannedPath<'p, 's> {
    let result = scan_segments_checked(body, starts, ends);
    ScannedPath {
        body,
        starts,
        ends,
        capacity: starts.len().min(ends.len()),
        count: result.count,
        valid: result.valid,
    }
}

/// Scans a path with bounded inline storage and invokes `resolve` with it.
#[doc(hidden)]
#[inline]
pub fn with_scanned_path<'p, R>(
    body: &'p str,
    max_segments: usize,
    resolve: impl for<'s> FnOnce(&ScannedPath<'p, 's>) -> R,
) -> Result<R, InvalidPath> {
    const INLINE_SEGMENTS: usize = 16;

    let mut inline_starts = [0_usize; INLINE_SEGMENTS];
    let mut inline_ends = [0_usize; INLINE_SEGMENTS];
    let inline_capacity = max_segments.min(INLINE_SEGMENTS);
    let path = scan_path(body, &mut inline_starts[..inline_capacity], &mut inline_ends[..inline_capacity]);
    if !path.is_valid() {
        return Err(InvalidPath);
    }
    if max_segments <= INLINE_SEGMENTS || path.count() <= INLINE_SEGMENTS {
        return Ok(resolve(&path));
    }

    let capacity = path.count().min(max_segments);
    let mut offsets = vec![0_usize; capacity * 2];
    let (starts, ends) = offsets.split_at_mut(capacity);
    let path = scan_path(body, starts, ends);
    debug_assert!(path.is_valid());
    Ok(resolve(&path))
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::String;

    use super::*;

    #[test]
    fn exposes_only_ranges_produced_by_its_scan() {
        let mut starts = [0; 3];
        let mut ends = [0; 3];
        let path = scan_path("/a/bb/ccc", &mut starts, &mut ends);

        assert_eq!(path.count(), 3);
        assert_eq!(path.segment(1), Some("bb"));
        assert_eq!(path.capture(0, 1), Some("a/bb"));
        assert_eq!(path.rest(1), Some("bb/ccc"));
        assert_eq!(path.segment(3), None);
        assert_eq!(path.capture(2, 1), None);
    }

    #[test]
    fn affix_rejects_invalid_trimming() {
        let mut starts = [0; 1];
        let mut ends = [0; 1];
        let path = scan_path("/img-cat.png", &mut starts, &mut ends);

        assert_eq!(path.affix(0, 4, 4), Some("cat"));
        assert_eq!(path.affix(0, 20, 0), None);
    }

    #[test]
    fn invalid_manual_ranges_are_rejected_and_debugged() {
        let empty = ScannedPath {
            body: "/a",
            starts: &[],
            ends: &[],
            capacity: 0,
            count: 1,
            valid: true,
        };
        assert_eq!(empty.segment(0), None);
        assert_eq!(empty.capture(0, 0), None);
        assert_eq!(empty.rest(0), None);
        assert_eq!(empty.rest(2), None);
        assert_eq!(empty.affix(1, 0, 0), None);
        assert_eq!(format!("{empty:?}"), "ScannedPath { body: \"/a\", count: 1, .. }");
    }

    #[test]
    fn bounded_scanner_handles_paths_above_inline_route_depth() {
        let body = "/segment".repeat(20);
        let result = with_scanned_path(&body, 128, |path| (path.count(), path.segment(19).map(String::from))).expect("valid path");
        assert_eq!(result, (20, Some(String::from("segment"))));
    }

    #[test]
    fn bounded_scanner_keeps_inline_storage_for_shallow_requests() {
        let result = with_scanned_path("/one/two", 128, |path| (path.count(), path.starts.len())).expect("valid path");
        assert_eq!(result, (2, 16));
    }

    #[test]
    fn bounded_scanner_rejects_query_and_fragment_delimiters() {
        assert_eq!(with_scanned_path("/books?sort=title", 2, |_| ()), Err(InvalidPath));
        assert_eq!(with_scanned_path("/books#reviews", 2, |_| ()), Err(InvalidPath));
    }
}
