// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A per-worker, allocation-free parse cache for stable header fields.

use headers::HeaderMapExt as _;
use http::HeaderMap;
use http::header::{ACCEPT_ENCODING, DATE, EXPIRES, HeaderName, LAST_MODIFIED};

use super::{AcceptEncoding, Date, Expires, LastModified};

/// The number of distinct values retained per cached header.
const CAPACITY: usize = 16;

/// The largest field value retained by the date caches.
const DATE_KEY: usize = 48;

/// The largest field value retained by the `Accept-Encoding` cache.
const ENCODING_KEY: usize = 128;

/// A caller-owned parse cache for expensive, high-stability header fields.
///
/// Values are keyed by exact raw bytes. Each header retains sixteen entries
/// with fixed inline keys; oversized values bypass the cache. Parsing is
/// delegated to the [`headers`] crate except for `Accept-Encoding`, which is
/// resolved directly into a compact [`Copy`] decision. The cache owns no shared
/// state and is intended to be held per worker.
///
/// Cached fields are `Date`, `Expires`, `Last-Modified`, and
/// `Accept-Encoding`. Request condition dates remain stateless through
/// [`HeaderExt`](super::HeaderExt).
///
/// # Examples
///
/// ```
/// use std::time::{Duration, SystemTime, UNIX_EPOCH};
///
/// use routerama::route::header::{Encoding, HeaderCache};
/// # use http::HeaderMap;
/// # use http::header::{ACCEPT_ENCODING, DATE};
///
/// let mut headers = HeaderMap::new();
/// headers.insert(
///     DATE,
///     "Sun, 06 Nov 1994 08:49:37 GMT"
///         .parse()
///         .expect("valid value"),
/// );
/// headers.insert(
///     ACCEPT_ENCODING,
///     "br, gzip;q=0.5".parse().expect("valid value"),
/// );
///
/// let mut cache = HeaderCache::new();
/// let date: SystemTime = cache.date(&headers).expect("valid Date").into();
/// assert_eq!(
///     date.duration_since(UNIX_EPOCH).expect("after the epoch"),
///     Duration::from_secs(784_111_777),
/// );
/// let encoding = cache
///     .accept_encoding(&headers)
///     .expect("Accept-Encoding present");
/// assert_eq!(
///     encoding.preferred([Encoding::Gzip, Encoding::Brotli]),
///     Some(Encoding::Brotli),
/// );
/// ```
#[derive(Clone, Debug)]
pub struct HeaderCache {
    date: Mru<Date, DATE_KEY>,
    last_modified: Mru<LastModified, DATE_KEY>,
    expires: Mru<Expires, DATE_KEY>,
    accept_encoding: Mru<AcceptEncoding, ENCODING_KEY>,
}

impl HeaderCache {
    /// Creates an empty cache.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            date: Mru::new(),
            last_modified: Mru::new(),
            expires: Mru::new(),
            accept_encoding: Mru::new(),
        }
    }

    /// Returns the parsed `Date` header, memoizing the decoded value.
    pub fn date(&mut self, headers: &HeaderMap) -> Option<Date> {
        let value = single(headers, DATE)?;
        self.date.resolve(value.as_bytes(), || headers.typed_get())
    }

    /// Returns the parsed `Last-Modified` header, memoizing the decoded value.
    pub fn last_modified(&mut self, headers: &HeaderMap) -> Option<LastModified> {
        let value = single(headers, LAST_MODIFIED)?;
        self.last_modified.resolve(value.as_bytes(), || headers.typed_get())
    }

    /// Returns the parsed `Expires` header, memoizing the decoded value.
    pub fn expires(&mut self, headers: &HeaderMap) -> Option<Expires> {
        let value = single(headers, EXPIRES)?;
        self.expires.resolve(value.as_bytes(), || headers.typed_get())
    }

    /// Returns the resolved `Accept-Encoding` decision, memoizing one field
    /// line by its exact raw bytes.
    ///
    /// Repeated field lines are combined without caching.
    pub fn accept_encoding(&mut self, headers: &HeaderMap) -> Option<AcceptEncoding> {
        let mut values = headers.get_all(ACCEPT_ENCODING).iter();
        let first = values.next()?;
        if values.next().is_some() {
            return AcceptEncoding::parse_all(headers.get_all(ACCEPT_ENCODING).iter().map(http::HeaderValue::as_bytes));
        }
        self.accept_encoding
            .resolve(first.as_bytes(), || AcceptEncoding::parse(first.as_bytes()))
    }
}

impl Default for HeaderCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns the sole field value for a header, or [`None`] if absent or repeated.
fn single(headers: &HeaderMap, name: HeaderName) -> Option<&http::HeaderValue> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    Some(value)
}

/// A bounded, allocation-free most-recently-inserted cache with inline keys.
#[derive(Clone, Debug)]
struct Mru<V: Copy, const KEY: usize> {
    slots: [Slot<V, KEY>; CAPACITY],
    len: usize,
    next: usize,
}

impl<V: Copy, const KEY: usize> Mru<V, KEY> {
    const fn new() -> Self {
        Self {
            slots: [Slot::EMPTY; CAPACITY],
            len: 0,
            next: 0,
        }
    }

    fn resolve(&mut self, key: &[u8], parse: impl FnOnce() -> Option<V>) -> Option<V> {
        let cacheable = key.len() <= KEY;
        if cacheable {
            for slot in self.slots.iter().take(self.len) {
                if let Some(value) = slot.get(key) {
                    return Some(value);
                }
            }
        }
        let value = parse()?;
        if cacheable {
            self.store(key, value);
        }
        Some(value)
    }

    fn store(&mut self, key: &[u8], value: V) {
        let index = if self.len < CAPACITY {
            let index = self.len;
            self.len += 1;
            index
        } else {
            let index = self.next;
            self.next = (self.next + 1) % CAPACITY;
            index
        };
        self.slots
            .get_mut(index)
            .expect("the insertion index is bounded by CAPACITY")
            .set(key, value);
    }
}

#[derive(Clone, Copy, Debug)]
struct Slot<V: Copy, const KEY: usize> {
    key: [u8; KEY],
    key_len: usize,
    value: Option<V>,
}

impl<V: Copy, const KEY: usize> Slot<V, KEY> {
    const EMPTY: Self = Self {
        key: [0; KEY],
        key_len: 0,
        value: None,
    };

    fn get(&self, key: &[u8]) -> Option<V> {
        if self.key.get(..self.key_len) == Some(key) {
            self.value
        } else {
            None
        }
    }

    fn set(&mut self, key: &[u8], value: V) {
        self.key
            .get_mut(..key.len())
            .expect("cache insertion is guarded by the maximum key length")
            .copy_from_slice(key);
        self.key_len = key.len();
        self.value = Some(value);
    }
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn headers(name: HeaderName, value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(name, HeaderValue::from_str(value).expect("valid header value"));
        headers
    }

    #[test]
    fn a_repeated_value_is_served_from_the_cache() {
        let mut cache = HeaderCache::new();
        let map = headers(DATE, "Sun, 06 Nov 1994 08:49:37 GMT");

        let first = cache.date(&map).expect("valid Date");
        let second = cache.date(&map).expect("valid Date");
        assert_eq!(first, second);
        assert_eq!(cache.date.len, 1);
    }

    #[test]
    fn distinct_values_fill_then_evict_round_robin() {
        let mut cache = HeaderCache::new();
        for minute in 0..CAPACITY + 4 {
            let value = alloc::format!("Sun, 06 Nov 1994 08:{minute:02}:37 GMT");
            assert!(cache.date(&headers(DATE, &value)).is_some(), "{value}");
        }
        assert_eq!(cache.date.len, CAPACITY);
    }

    #[test]
    fn repeated_or_absent_date_fields_return_none() {
        let mut cache = HeaderCache::new();
        assert!(cache.date(&HeaderMap::new()).is_none());

        let mut map = headers(DATE, "Sun, 06 Nov 1994 08:49:37 GMT");
        map.append(DATE, HeaderValue::from_static("Sun, 06 Nov 1994 08:49:38 GMT"));
        assert!(cache.date(&map).is_none());
    }

    #[test]
    fn an_oversized_encoding_bypasses_the_cache_but_still_parses() {
        let mut cache = HeaderCache::new();
        let padded = alloc::format!("{:width$}gzip", "", width = ENCODING_KEY);
        let map = headers(ACCEPT_ENCODING, &padded);

        assert!(cache.accept_encoding(&map).expect("valid").accepts(super::super::Encoding::Gzip));
        assert_eq!(cache.accept_encoding.len, 0);
    }

    #[test]
    fn repeated_encoding_lines_combine_without_caching() {
        let mut cache = HeaderCache::new();
        let mut map = headers(ACCEPT_ENCODING, "gzip;q=0.5");
        map.append(ACCEPT_ENCODING, HeaderValue::from_static("br"));

        let decision = cache.accept_encoding(&map).expect("valid");
        assert_eq!(decision.quality(super::super::Encoding::Gzip), 500);
        assert!(decision.accepts(super::super::Encoding::Brotli));
        assert_eq!(cache.accept_encoding.len, 0);
    }
}
