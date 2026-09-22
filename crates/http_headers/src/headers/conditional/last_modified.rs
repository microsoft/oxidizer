// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::time::SystemTime;

use super::shared::{date_header, parse_http_date};
use crate::{DecodeError, FieldName, FieldValue, FieldValueRef, SingleValueField};

date_header!(
    LastModified,
    LastModifiedOwned,
    LastModifiedView,
    "Last-Modified",
    &FieldName::LastModified,
    "Owned value for the `Last-Modified` header.",
    "Borrowed value for the `Last-Modified` header.",
    "Defined by [RFC 9110 section 8.8.2](https://www.rfc-editor.org/rfc/rfc9110#section-8.8.2).",
    "`Last-Modified: Thu, 01 Jan 2015 12:00:00 GMT` supplies the selected representation's modification time."
);
