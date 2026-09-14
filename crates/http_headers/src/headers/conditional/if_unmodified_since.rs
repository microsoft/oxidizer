// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::time::SystemTime;

use super::shared::{date_header, parse_http_date};
use crate::{DecodeError, FieldName, FieldValue, FieldValueRef, SingleValueField};

date_header!(
    IfUnmodifiedSince,
    IfUnmodifiedSinceOwned,
    IfUnmodifiedSinceView,
    "If-Unmodified-Since",
    &FieldName::IfUnmodifiedSince,
    "Owned value for the `If-Unmodified-Since` header.",
    "Borrowed value for the `If-Unmodified-Since` header.",
    "Defined by [RFC 9110 section 13.1.4](https://www.rfc-editor.org/rfc/rfc9110#section-13.1.4).",
    "`If-Unmodified-Since: Sat, 29 Oct 2022 19:43:31 GMT` supplies an HTTP date."
);
