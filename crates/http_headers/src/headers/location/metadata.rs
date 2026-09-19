// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retained boundaries in the validated semantic spelling.

use std::num::NonZeroUsize;
use std::ops::Range;

use fluent_uri::Uri;
use http_headers_simd::find_either;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Metadata {
    pub(super) scheme_end: Option<NonZeroUsize>,
    pub(super) authority_host: Option<Range<usize>>,
    pub(super) path: Range<usize>,
    pub(super) query_end: Option<NonZeroUsize>,
    pub(super) fragment_start: Option<NonZeroUsize>,
}

impl Metadata {
    pub(super) fn from_parsed(parsed: &Uri<&str>) -> Self {
        let scheme_end = parsed.scheme().and_then(|scheme| NonZeroUsize::new(scheme.as_str().len()));
        let prefix_end = scheme_end.map_or(0, |end| end.get() + 1);
        let authority_host = parsed.authority().map(|authority| {
            let start = prefix_end + 2;
            let host_start = start + authority.userinfo().map_or(0, |userinfo| userinfo.as_str().len() + 1);
            host_start..host_start + authority.host().as_str().len()
        });
        let path_start = prefix_end + parsed.authority().map_or(0, |authority| 2 + authority.as_str().len());
        let path_end = path_start + parsed.path().as_str().len();
        let query_end = parsed
            .query()
            .and_then(|query| NonZeroUsize::new(path_end + 1 + query.as_str().len()));
        let fragment_start = parsed
            .fragment()
            .and_then(|_fragment| NonZeroUsize::new(query_end.map_or(path_end, NonZeroUsize::get) + 1));
        Self {
            scheme_end,
            authority_host,
            path: path_start..path_end,
            query_end,
            fragment_start,
        }
    }

    /// Projects the subset already accepted by the SIMD scanner, not arbitrary input.
    pub(super) fn from_simple(text: &str) -> Self {
        let bytes = text.as_bytes();
        let (scheme_end, authority_host, path_start, path_end) = if bytes.first() == Some(&b'/') {
            let path_end = find_either(bytes, b'?', b'#').unwrap_or(bytes.len());
            (None, None, 0, path_end)
        } else {
            let scheme_end = find_either(bytes, b':', b':').expect("the simple absolute subset includes a scheme colon");
            let start = scheme_end + 3;
            let path_end = find_either(&bytes[start..], b'?', b'#').map_or(bytes.len(), |offset| start + offset);
            // Colons and slashes in a query or fragment are not authority boundaries.
            let host_end = find_either(&bytes[start..path_end], b':', b'/').map_or(path_end, |offset| start + offset);
            let path_start = if bytes.get(host_end) == Some(&b':') {
                find_either(&bytes[host_end + 1..path_end], b'/', b'/').map_or(path_end, |offset| host_end + 1 + offset)
            } else {
                host_end
            };
            (NonZeroUsize::new(scheme_end), Some(start..host_end), path_start, path_end)
        };
        let (query_end, fragment_start) = if bytes.get(path_end) == Some(&b'?') {
            let query_end = find_either(&bytes[path_end + 1..], b'#', b'#').map_or(bytes.len(), |offset| path_end + 1 + offset);
            (
                NonZeroUsize::new(query_end),
                (query_end < bytes.len()).then(|| NonZeroUsize::new(query_end + 1)).flatten(),
            )
        } else {
            (None, (path_end < bytes.len()).then(|| NonZeroUsize::new(path_end + 1)).flatten())
        };
        Self {
            scheme_end,
            authority_host,
            path: path_start..path_end,
            query_end,
            fragment_start,
        }
    }
}
