// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Decode-only shapes for conditional and range headers, excluding semantic reader costs.

use http_headers::DecodeErrorKind;
use http_headers::headers::{
    AcceptRanges, ContentRange, ETag, IfMatch, IfModifiedSince, IfNoneMatch, IfRange, IfUnmodifiedSince, LastModified, Range,
};

#[path = "http_headers_shapes_common.rs"]
mod shapes;

use shapes::Expected;

shapes::define_shapes!(
    "http_headers_conditional_range_shapes/parse";
    // Extra lengths isolate the scalar/word scanner and inline/shared storage boundaries.
    (etag_absent, ETag, &[], Strict, Expected::Absent),
    (etag_short, ETag, &["\"x\""], Strict, Expected::Valid),
    (etag_revision, ETag, &["\"revision-42\""], Strict, Expected::Valid),
    (etag_weak_revision, ETag, &["W/\"revision-42\""], Strict, Expected::Valid),
    (etag_len_seven, ETag, &["\"1234567\""], Strict, Expected::Valid),
    (etag_len_eight, ETag, &["\"12345678\""], Strict, Expected::Valid),
    (etag_len_nine, ETag, &["\"123456789\""], Strict, Expected::Valid),
    (etag_wire_sixty_four, ETag, &["\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd\""], Strict, Expected::Valid),
    (etag_wire_sixty_five, ETag, &["\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde\""], Strict, Expected::Valid),
    (etag_lowercase_weak_relaxed, ETag, &["w/\"revision-42\""], Relaxed, Expected::Valid),
    (etag_space_late, ETag, &["\"0123456789abcdef0123456789abcdef bad\""], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (etag_repeated, ETag, &["\"one\"", "\"two\""], Strict, Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)),

    (if_match_one, IfMatch, &["\"revision-42\""], Strict, Expected::Valid),
    (if_match_wildcard, IfMatch, &["*"], Strict, Expected::Valid),
    (if_match_two, IfMatch, &["\"a\", W/\"b\""], Strict, Expected::Valid),
    (if_match_long, IfMatch, &["\"sha256-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\""], Strict, Expected::Valid),
    (if_match_many, IfMatch, &["\"revision-0\", \"revision-1\", \"revision-2\", \"revision-3\", \"revision-4\", \"revision-5\", \"revision-6\", \"revision-7\", \"revision-8\", \"revision-9\", \"revision-10\", \"revision-11\", \"revision-12\", \"revision-13\", \"revision-14\", \"revision-15\""], Strict, Expected::Valid),
    (if_match_two_lines, IfMatch, &["\"one\"", "W/\"two\""], Strict, Expected::Valid),
    (if_match_comma_backslash, IfMatch, &["\"one,two\", \"comma,slash\\\""], Strict, Expected::Valid),
    (if_match_lowercase_relaxed, IfMatch, &["w/\"revision\", \"next\""], Relaxed, Expected::Valid),
    (if_match_late_invalid, IfMatch, &["\"one\"", "W/\"two\"", "not-a-tag"], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (if_match_empty_field, IfMatch, &[""], Strict, Expected::Error(DecodeErrorKind::MissingValue)),

    (if_none_match_one, IfNoneMatch, &["\"revision-42\""], Strict, Expected::Valid),
    (if_none_match_wildcard, IfNoneMatch, &["*"], Strict, Expected::Valid),
    (if_none_match_two, IfNoneMatch, &["W/\"a\", \"b\""], Strict, Expected::Valid),
    (if_none_match_long, IfNoneMatch, &["\"sha256-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\""], Strict, Expected::Valid),
    (if_none_match_many, IfNoneMatch, &["\"revision-0\", \"revision-1\", \"revision-2\", \"revision-3\", \"revision-4\", \"revision-5\", \"revision-6\", \"revision-7\", \"revision-8\", \"revision-9\", \"revision-10\", \"revision-11\", \"revision-12\", \"revision-13\", \"revision-14\", \"revision-15\""], Strict, Expected::Valid),
    (if_none_match_two_lines, IfNoneMatch, &["\"one\"", "W/\"two\""], Strict, Expected::Valid),
    (if_none_match_comma_backslash, IfNoneMatch, &["\"one,two\", \"comma,slash\\\""], Strict, Expected::Valid),
    (if_none_match_lowercase_relaxed, IfNoneMatch, &["w/\"revision\", \"next\""], Relaxed, Expected::Valid),
    (if_none_match_late_invalid, IfNoneMatch, &["\"one\"", "W/\"two\"", "not-a-tag"], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (if_none_match_empty_field, IfNoneMatch, &[""], Strict, Expected::Error(DecodeErrorKind::MissingValue)),

    (if_range_revision, IfRange, &["\"revision-42\""], Strict, Expected::Valid),
    (if_range_long_tag, IfRange, &["\"sha256-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\""], Strict, Expected::Valid),
    (if_range_imf_date, IfRange, &["Sun, 06 Nov 1994 08:49:37 GMT"], Strict, Expected::Valid),
    (if_range_rfc850, IfRange, &["Sunday, 06-Nov-94 08:49:37 GMT"], Strict, Expected::Valid),
    (if_range_asctime, IfRange, &["Sun Nov  6 08:49:37 1994"], Strict, Expected::Valid),
    (if_range_relaxed_date, IfRange, &[" Tue, 8 Nov 1994 8:49:37 UTC "], Relaxed, Expected::Valid),
    (if_range_relaxed_strict_date, IfRange, &["Sun, 06 Nov 1994 08:49:37 GMT"], Relaxed, Expected::Valid),
    (if_range_weak, IfRange, &["W/\"revision-42\""], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (if_range_unterminated, IfRange, &["\"unterminated"], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (if_range_repeated, IfRange, &["\"revision-42\"", "Sun, 06 Nov 1994 08:49:37 GMT"], Strict, Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)),

    (if_modified_since_imf, IfModifiedSince, &["Sun, 06 Nov 1994 08:49:37 GMT"], Strict, Expected::Valid),
    (if_modified_since_leap, IfModifiedSince, &["Thu, 29 Feb 2024 00:00:00 GMT"], Strict, Expected::Valid),
    (if_modified_since_rfc850, IfModifiedSince, &["Sunday, 06-Nov-94 08:49:37 GMT"], Strict, Expected::Valid),
    (if_modified_since_asctime, IfModifiedSince, &["Sun Nov  6 08:49:37 1994"], Strict, Expected::Valid),
    (if_modified_since_relaxed_canonical, IfModifiedSince, &["Sun, 06 Nov 1994 08:49:37 GMT"], Relaxed, Expected::Valid),
    (if_modified_since_ows_relaxed, IfModifiedSince, &[" Sun, 06 Nov 1994 08:49:37 GMT "], Relaxed, Expected::Valid),
    (if_modified_since_normalized, IfModifiedSince, &[" Tue, 8 Nov 1994 8:49:37 UTC "], Relaxed, Expected::Valid),
    (if_modified_since_wrong_weekday, IfModifiedSince, &["Mon, 06 Nov 1994 08:49:37 GMT"], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (if_modified_since_empty, IfModifiedSince, &[""], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (if_modified_since_repeated, IfModifiedSince, &["Sun, 06 Nov 1994 08:49:37 GMT", "Mon, 07 Nov 1994 08:49:37 GMT"], Strict, Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)),

    (if_unmodified_since_imf, IfUnmodifiedSince, &["Sun, 06 Nov 1994 08:49:37 GMT"], Strict, Expected::Valid),
    (if_unmodified_since_leap, IfUnmodifiedSince, &["Thu, 29 Feb 2024 00:00:00 GMT"], Strict, Expected::Valid),
    (if_unmodified_since_rfc850, IfUnmodifiedSince, &["Sunday, 06-Nov-94 08:49:37 GMT"], Strict, Expected::Valid),
    (if_unmodified_since_asctime, IfUnmodifiedSince, &["Sun Nov  6 08:49:37 1994"], Strict, Expected::Valid),
    (if_unmodified_since_relaxed_canonical, IfUnmodifiedSince, &["Sun, 06 Nov 1994 08:49:37 GMT"], Relaxed, Expected::Valid),
    (if_unmodified_since_ows_relaxed, IfUnmodifiedSince, &[" Sun, 06 Nov 1994 08:49:37 GMT "], Relaxed, Expected::Valid),
    (if_unmodified_since_normalized, IfUnmodifiedSince, &[" Tue, 8 Nov 1994 8:49:37 UTC "], Relaxed, Expected::Valid),
    (if_unmodified_since_wrong_weekday, IfUnmodifiedSince, &["Mon, 06 Nov 1994 08:49:37 GMT"], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (if_unmodified_since_empty, IfUnmodifiedSince, &[""], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (if_unmodified_since_repeated, IfUnmodifiedSince, &["Sun, 06 Nov 1994 08:49:37 GMT", "Mon, 07 Nov 1994 08:49:37 GMT"], Strict, Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)),

    (last_modified_imf, LastModified, &["Sun, 06 Nov 1994 08:49:37 GMT"], Strict, Expected::Valid),
    (last_modified_leap, LastModified, &["Thu, 29 Feb 2024 00:00:00 GMT"], Strict, Expected::Valid),
    (last_modified_rfc850, LastModified, &["Sunday, 06-Nov-94 08:49:37 GMT"], Strict, Expected::Valid),
    (last_modified_asctime, LastModified, &["Sun Nov  6 08:49:37 1994"], Strict, Expected::Valid),
    (last_modified_relaxed_canonical, LastModified, &["Sun, 06 Nov 1994 08:49:37 GMT"], Relaxed, Expected::Valid),
    (last_modified_ows_relaxed, LastModified, &[" Sun, 06 Nov 1994 08:49:37 GMT "], Relaxed, Expected::Valid),
    (last_modified_normalized, LastModified, &[" Tue, 8 Nov 1994 8:49:37 UTC "], Relaxed, Expected::Valid),
    (last_modified_wrong_weekday, LastModified, &["Mon, 06 Nov 1994 08:49:37 GMT"], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (last_modified_empty, LastModified, &[""], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (last_modified_repeated, LastModified, &["Sun, 06 Nov 1994 08:49:37 GMT", "Mon, 07 Nov 1994 08:49:37 GMT"], Strict, Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)),

    // Extra range cases distinguish scanner fallbacks from numeric and cardinality errors.
    (range_closed, Range, &["bytes=0-499"], Strict, Expected::Valid),
    (range_existing, Range, &["bytes=0-499, 1000-"], Strict, Expected::Valid),
    (range_suffix, Range, &["bytes=-500"], Strict, Expected::Valid),
    (range_many_members, Range, &["bytes=0-9, 20-29, 40-49, 60-69, 80-89, 100-109, 120-129, 140-149, 160-169, 180-189, 200-209, 220-229, 240-249, 260-269, 280-289, 300-309"], Strict, Expected::Valid),
    (range_large_offsets, Range, &["bytes=1048576-2097151, 3145728-4194303"], Strict, Expected::Valid),
    (range_maximum_open, Range, &["bytes=18446744073709551615-"], Strict, Expected::Valid),
    (range_leading_zeroes, Range, &["bytes=00000000000000000000005-6"], Strict, Expected::Valid),
    (range_case_fallback, Range, &["Bytes=0-499, 1000-"], Strict, Expected::Valid),
    (range_extension, Range, &["items=1-5"], Strict, Expected::Valid),
    (range_relaxed_whitespace, Range, &[" Bytes = 0 - 9 , - 5 "], Relaxed, Expected::Valid),
    (range_inverted, Range, &["bytes=10-9"], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (range_overflow, Range, &["bytes=18446744073709551616-"], Strict, Expected::Error(DecodeErrorKind::InvalidNumber)),
    (range_repeated, Range, &["bytes=0-1", "bytes=2-3"], Strict, Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)),

    // Quote, token and cardinality errors take distinct fallback paths.
    (accept_ranges_bytes, AcceptRanges, &["bytes"], Strict, Expected::Valid),
    (accept_ranges_none, AcceptRanges, &["none"], Strict, Expected::Valid),
    (accept_ranges_case_bytes, AcceptRanges, &["Bytes"], Strict, Expected::Valid),
    (accept_ranges_ows, AcceptRanges, &[" \tbytes \t"], Strict, Expected::Valid),
    (accept_ranges_two, AcceptRanges, &["bytes, items"], Strict, Expected::Valid),
    (accept_ranges_repeated, AcceptRanges, &["bytes", "items", "records"], Strict, Expected::Valid),
    (accept_ranges_many_lines, AcceptRanges, &["bytes", "items", "records", "frames", "pages", "lines", "chunks", "segments", "blocks", "entries", "samples", "packets", "rows", "columns", "cells", "objects"], Strict, Expected::Valid),
    (accept_ranges_none_conflict, AcceptRanges, &["none, bytes"], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (accept_ranges_late_invalid, AcceptRanges, &["bytes", "items", "bad/unit"], Strict, Expected::Error(DecodeErrorKind::InvalidToken)),
    (accept_ranges_unterminated, AcceptRanges, &["\"bytes"], Strict, Expected::Error(DecodeErrorKind::UnterminatedQuote)),
    (accept_ranges_empty_field, AcceptRanges, &[""], Strict, Expected::Error(DecodeErrorKind::MissingValue)),

    // Empty extensions, unknown lengths and unsatisfied responses have different grammars.
    (content_range_existing, ContentRange, &["bytes 0-499/1234"], Strict, Expected::Valid),
    (content_range_unknown, ContentRange, &["bytes 500-999/*"], Strict, Expected::Valid),
    (content_range_unsatisfied, ContentRange, &["bytes */1234"], Strict, Expected::Valid),
    (content_range_large, ContentRange, &["bytes 1048576-2097151/4294967296"], Strict, Expected::Valid),
    (content_range_leading_zeroes, ContentRange, &["bytes 00000000000000000000005-0006/000010"], Strict, Expected::Valid),
    (content_range_case, ContentRange, &["Bytes 0-499/1234"], Strict, Expected::Valid),
    (content_range_extension_empty, ContentRange, &["items "], Strict, Expected::Valid),
    (content_range_extension_long, ContentRange, &["example-unit opaque response segment-0000000000000001 to segment-0000000000000002"], Strict, Expected::Valid),
    (content_range_relaxed_spaces, ContentRange, &["Bytes 0 - 499 / 1234"], Relaxed, Expected::Valid),
    (content_range_overflow, ContentRange, &["bytes 0-1/18446744073709551616"], Strict, Expected::Error(DecodeErrorKind::InvalidNumber)),
    (content_range_length_too_small, ContentRange, &["bytes 0-499/499"], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (content_range_repeated, ContentRange, &["bytes 0-9/100", "bytes 20-29/100"], Strict, Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)),
);
