// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Segments`] path segment collection and its iterator.

/// The `/`-separated segments of a request path, retaining byte offsets so that
/// multi-segment captures (`**` / `{name=shelves/*}`) can be returned as
/// zero-copy slices of the original path.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::split_path;
///
/// let (segments, _verb) = split_path("/shelves/7/books");
/// assert_eq!(segments.len(), 3);
/// assert!(!segments.is_empty());
/// assert_eq!(segments.get(0), Some("shelves"));
/// assert_eq!(segments.span(1, usize::MAX), "7/books");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Segments<'p> {
    body: &'p str,
    bounds: Vec<(usize, usize)>,
}

impl<'p> Segments<'p> {
    pub(crate) fn new(body: &'p str) -> Self {
        let rest = body.strip_prefix('/').unwrap_or(body);
        let mut bounds = Vec::new();
        if !rest.is_empty() {
            bounds.reserve(rest.bytes().filter(|&b| b == b'/').count() + 1);
            let base = body.len() - rest.len();
            let mut start = 0_usize;
            for (idx, ch) in rest.char_indices() {
                if ch == '/' {
                    bounds.push((base + start, base + idx));
                    start = idx + 1;
                }
            }
            bounds.push((base + start, base + rest.len()));
        }
        Self { body, bounds }
    }

    /// The number of path segments. A root path (`/` or empty) has length `0`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::split_path;
    ///
    /// let (segments, _verb) = split_path("/shelves/7/books");
    /// assert_eq!(segments.len(), 3);
    ///
    /// let (root, _verb) = split_path("/");
    /// assert_eq!(root.len(), 0);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.bounds.len()
    }

    /// Returns `true` if there are no segments (a root path).
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::split_path;
    ///
    /// let (root, _verb) = split_path("/");
    /// assert!(root.is_empty());
    ///
    /// let (segments, _verb) = split_path("/shelves/7/books");
    /// assert!(!segments.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bounds.is_empty()
    }

    /// Returns the segment at index `i`, or [`None`] if out of range.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::split_path;
    ///
    /// let (segments, _verb) = split_path("/shelves/7/books");
    /// assert_eq!(segments.get(0), Some("shelves"));
    /// assert_eq!(segments.get(1), Some("7"));
    /// assert_eq!(segments.get(3), None);
    /// ```
    #[must_use]
    pub fn get(&self, i: usize) -> Option<&'p str> {
        self.bounds.get(i).map(|&(s, e)| &self.body[s..e])
    }

    /// Returns an iterator over the path segments, in order.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::split_path;
    ///
    /// let (segments, _verb) = split_path("/shelves/7/books");
    /// let collected: Vec<&str> = segments.iter().collect();
    /// assert_eq!(collected, ["shelves", "7", "books"]);
    /// ```
    #[must_use]
    pub fn iter(&self) -> SegmentsIter<'_, 'p> {
        SegmentsIter {
            segments: self,
            front: 0,
            back: self.len(),
        }
    }

    /// Returns the contiguous slice of the original path spanning segments
    /// `start..end` (joined by their original `/` separators).
    ///
    /// Returns `""` when `end <= start`. Indices past the end are clamped to the
    /// available range, so `span(k, usize::MAX)` captures "the rest from `k`".
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::split_path;
    ///
    /// let (segments, _verb) = split_path("/shelves/7/books");
    /// assert_eq!(segments.span(0, 2), "shelves/7");
    /// assert_eq!(segments.span(1, usize::MAX), "7/books");
    /// assert_eq!(segments.span(2, 2), "");
    /// ```
    #[must_use]
    pub fn span(&self, start: usize, end: usize) -> &'p str {
        let end = end.min(self.bounds.len());
        if end <= start || start >= self.bounds.len() {
            return "";
        }
        let s = self.bounds[start].0;
        let e = self.bounds[end - 1].1;
        &self.body[s..e]
    }
}

/// An iterator over the segments of a [`Segments`], yielding each `/`-separated
/// segment as a borrowed slice of the original path. Created by
/// [`Segments::iter`].
///
/// # Examples
///
/// ```
/// use rest_over_grpc::split_path;
///
/// let (segments, _verb) = split_path("/shelves/7/books");
/// let mut iter = segments.iter();
/// assert_eq!(iter.len(), 3);
/// assert_eq!(iter.next(), Some("shelves"));
/// assert_eq!(iter.collect::<Vec<_>>(), ["7", "books"]);
/// ```
#[derive(Debug, Clone)]
pub struct SegmentsIter<'a, 'p> {
    segments: &'a Segments<'p>,
    front: usize,
    back: usize,
}

impl<'p> Iterator for SegmentsIter<'_, 'p> {
    type Item = &'p str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.front >= self.back {
            return None;
        }
        let item = self.segments.get(self.front);
        self.front += 1;
        item
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.back - self.front;
        (remaining, Some(remaining))
    }
}

impl DoubleEndedIterator for SegmentsIter<'_, '_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front >= self.back {
            return None;
        }
        self.back -= 1;
        self.segments.get(self.back)
    }
}

impl ExactSizeIterator for SegmentsIter<'_, '_> {}

impl<'a, 'p> IntoIterator for &'a Segments<'p> {
    type Item = &'p str;
    type IntoIter = SegmentsIter<'a, 'p>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use crate::split_path;

    #[test]
    fn span_joins_segments() {
        let (segs, _) = split_path("/v1/shelves/3/4");
        assert_eq!(segs.span(1, 4), "shelves/3/4");
        assert_eq!(segs.span(1, 2), "shelves");
        assert_eq!(segs.span(2, usize::MAX), "3/4");
        assert_eq!(segs.span(2, 2), "");
        assert_eq!(segs.span(9, 10), "");
    }

    #[test]
    fn segments_iterate_in_order() {
        let (segments, _verb) = split_path("/shelves/7/books");
        let via_iter: Vec<&str> = segments.iter().collect();
        assert_eq!(via_iter, ["shelves", "7", "books"]);

        // `IntoIterator for &Segments` yields the same sequence.
        let via_into: Vec<&str> = (&segments).into_iter().collect();
        assert_eq!(via_into, ["shelves", "7", "books"]);

        // Reverse iteration via `DoubleEndedIterator`.
        let reversed: Vec<&str> = segments.iter().rev().collect();
        assert_eq!(reversed, ["books", "7", "shelves"]);

        // Meeting in the middle from both ends.
        let mut iter = segments.iter();
        assert_eq!(iter.next(), Some("shelves"));
        assert_eq!(iter.next_back(), Some("books"));
        assert_eq!(iter.next(), Some("7"));
        assert_eq!(iter.next_back(), None);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn segments_iter_reports_exact_len() {
        let (segments, _verb) = split_path("/a/b/c");
        let mut iter = segments.iter();
        assert_eq!(iter.size_hint(), (3, Some(3)));
        assert_eq!(iter.len(), 3);
        assert_eq!(iter.next(), Some("a"));
        assert_eq!(iter.size_hint(), (2, Some(2)));
        assert_eq!(iter.by_ref().count(), 2);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn empty_path_iterates_to_nothing() {
        let (segments, _verb) = split_path("/");
        assert_eq!(segments.iter().next(), None);
        assert_eq!(segments.iter().len(), 0);
    }
}
