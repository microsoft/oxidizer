// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Borrowed iteration over owned CORS collections preserves their token lists.

#![cfg(feature = "headers-cors")]

use std::fmt::{self, Write as _};

use http_headers::FieldValue;
use http_headers::headers::{
    AccessControlAllowHeadersOwned, AccessControlAllowMethodsOwned, AccessControlExposeHeadersOwned, AccessControlRequestHeadersOwned,
};

struct RejectWriter;

impl fmt::Write for RejectWriter {
    fn write_str(&mut self, _text: &str) -> fmt::Result {
        Err(fmt::Error)
    }
}

#[test]
fn borrowed_owned_collection_iteration_keeps_order_case_duplicates_and_wildcards() {
    macro_rules! check {
        ($owned:ty, $first:literal, $last:literal, $expected:expr, $debug_name:literal) => {{
            let value = <$owned>::from_field_values(vec![
                FieldValue::from_static($first),
                FieldValue::from_static(""),
                FieldValue::from_static($last),
            ])
            .unwrap();
            let expected = $expected;
            assert_eq!(value.iter().map(|item| item.as_str()).collect::<Vec<_>>(), expected);
            assert_eq!((&value).into_iter().map(|item| item.as_str()).collect::<Vec<_>>(), expected);
            let mut seen = Vec::new();
            for item in &value {
                seen.push(item.as_str());
            }
            assert_eq!(seen, expected);
            assert_eq!(value.field_values().count(), 3);
            assert_eq!(value.len(), expected.len());
            let mut iter = (&value).into_iter();
            for expected in expected {
                assert_eq!(iter.next().unwrap().as_str(), expected);
            }
            assert!(iter.next().is_none());
            assert!(iter.next().is_none());
            assert_eq!(format!("{iter:?}"), concat!($debug_name, " { .. }"));
            assert_eq!(write!(&mut RejectWriter, "{:?}", value.iter()), Err(fmt::Error));
        }};
    }
    check!(
        AccessControlAllowHeadersOwned,
        " \t, X-Trace,, content-type ,",
        "x-trace,*,",
        ["X-Trace", "content-type", "x-trace", "*"],
        "CorsHeaderNames"
    );
    check!(
        AccessControlExposeHeadersOwned,
        " \t, X-Trace,, content-type ,",
        "x-trace,*,",
        ["X-Trace", "content-type", "x-trace", "*"],
        "CorsHeaderNames"
    );
    check!(
        AccessControlRequestHeadersOwned,
        " \t, X-Trace,, content-type ,",
        "x-trace,*,",
        ["X-Trace", "content-type", "x-trace", "*"],
        "CorsHeaderNames"
    );
    check!(
        AccessControlAllowMethodsOwned,
        " \t, GET,, get ,",
        "X-CUSTOM,*,GET",
        ["GET", "get", "X-CUSTOM", "*", "GET"],
        "CorsMethods"
    );
}

#[test]
fn permitted_empty_collections_are_empty_borrowed_iterators() {
    macro_rules! check {
        ($owned:ty) => {{
            for value in [
                <$owned>::empty(),
                <$owned>::from_field_values(vec![FieldValue::from_static(" ,,\t"), FieldValue::from_static("")]).unwrap(),
            ] {
                let mut iter = (&value).into_iter();
                assert!(iter.next().is_none());
                assert!(iter.next().is_none());
                assert_eq!(value.iter().count(), 0);
            }
        }};
    }
    check!(AccessControlAllowHeadersOwned);
    check!(AccessControlExposeHeadersOwned);
    check!(AccessControlAllowMethodsOwned);
}
