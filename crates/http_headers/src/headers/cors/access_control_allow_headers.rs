// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use super::shared::{CorsHeaderNameView, CorsList, CorsListView, define_header_name_list, header_name_ref, impl_header_name_wildcard};
use crate::sink::{FieldSink, InsertError};
use crate::source::FieldSource;
use crate::{DecodeError, Field, FieldName, FieldValue, FieldValueRef, validate};

define_header_name_list!(
    AccessControlAllowHeaders,
    AccessControlAllowHeadersOwned,
    AccessControlAllowHeadersView,
    "Access-Control-Allow-Headers",
    &FieldName::AccessControlAllowHeaders,
    true,
    "Defined by the Fetch standard's [CORS protocol and credentials section](https://fetch.spec.whatwg.org/#http-access-control-allow-headers).",
    "`Access-Control-Allow-Headers: Content-Type, Authorization` lists field names, `Access-Control-Allow-Headers: *` uses a wildcard, and an empty field value is accepted."
);

impl_header_name_wildcard!(AccessControlAllowHeadersOwned, AccessControlAllowHeadersView);

impl AccessControlAllowHeadersOwned {
    /// Constructs a present field with an empty field-name list.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AccessControlAllowHeadersOwned;
    ///
    /// let value = AccessControlAllowHeadersOwned::empty();
    /// assert!(value.is_empty());
    /// assert_eq!(value.len(), 0);
    /// ```
    pub fn empty() -> Self {
        Self(CorsList::empty())
    }
}
