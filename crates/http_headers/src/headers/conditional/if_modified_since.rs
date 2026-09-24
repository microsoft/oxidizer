// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::time::SystemTime;

use super::shared::{date_header, parse_http_date};
use crate::{DecodeError, FieldName, FieldValue, FieldValueRef, SingleValueField};

date_header!(
    IfModifiedSince,
    IfModifiedSinceOwned,
    IfModifiedSinceView,
    "If-Modified-Since",
    &FieldName::IfModifiedSince,
    "Owned value for the `If-Modified-Since` header.",
    "Borrowed value for the `If-Modified-Since` header.",
    "Defined by [RFC 9110 section 13.1.3](https://www.rfc-editor.org/rfc/rfc9110#section-13.1.3).",
    "`If-Modified-Since: Tue, 15 Nov 1994 08:12:31 GMT` supplies an HTTP date."
);
