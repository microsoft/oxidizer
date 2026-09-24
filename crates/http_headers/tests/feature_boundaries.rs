// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration coverage for independently selectable header families.

#[cfg(any(feature = "headers-conditional", feature = "headers-range"))]
use http_headers::Field;

#[cfg(any(feature = "headers-conditional", feature = "headers-range"))]
fn assert_field<T: Field>() {}

#[cfg(feature = "headers-range")]
#[test]
fn range_family_exposes_range_headers() {
    use http_headers::headers::{AcceptRanges, ContentRange, Range};

    assert_field::<AcceptRanges>();
    assert_field::<ContentRange>();
    assert_field::<Range>();
}

#[cfg(feature = "headers-conditional")]
#[test]
fn conditional_family_includes_etag_headers() {
    use http_headers::headers::{ETag, IfMatch, IfModifiedSince, IfNoneMatch, IfRange, IfUnmodifiedSince, LastModified};

    assert_field::<ETag>();
    assert_field::<IfMatch>();
    assert_field::<IfModifiedSince>();
    assert_field::<IfNoneMatch>();
    assert_field::<IfRange>();
    assert_field::<IfUnmodifiedSince>();
    assert_field::<LastModified>();
}
