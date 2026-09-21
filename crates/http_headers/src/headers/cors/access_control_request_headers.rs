// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use super::super::FieldNameView;
use super::shared::{CorsList, CorsListView, define_header_name_list};
use crate::sink::{FieldSink, InsertError};
use crate::source::FieldSource;
use crate::{DecodeError, Field, FieldName, FieldValue, FieldValueRef, validate};

define_header_name_list!(
    AccessControlRequestHeaders,
    AccessControlRequestHeadersOwned,
    AccessControlRequestHeadersView,
    "Access-Control-Request-Headers",
    &FieldName::AccessControlRequestHeaders,
    false,
    "Defined by the Fetch standard's [CORS-preflight fetch section](https://fetch.spec.whatwg.org/#http-access-control-request-headers).",
    "`Access-Control-Request-Headers: Content-Type` names one field; `Access-Control-Request-Headers: X-Custom-Field, Authorization` names several."
);
