// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use super::shared::{CorsHeaderNameView, CorsList, CorsListView, define_header_name_list, impl_header_name_wildcard};
use crate::sink::{FieldSink, InsertError};
use crate::source::FieldSource;
use crate::{DecodeError, Field, FieldName, FieldValue, FieldValueRef, validate};

define_header_name_list!(
    AccessControlExposeHeaders,
    AccessControlExposeHeadersOwned,
    AccessControlExposeHeadersView,
    "Access-Control-Expose-Headers",
    &FieldName::AccessControlExposeHeaders,
    true,
    "Defined by the Fetch standard's [CORS protocol and credentials section](https://fetch.spec.whatwg.org/#http-access-control-expose-headers).",
    "`Access-Control-Expose-Headers: X-Request-Id, Content-Length` lists exposed fields, `Access-Control-Expose-Headers: *` uses a wildcard, and an empty field value is accepted."
);

impl_header_name_wildcard!(AccessControlExposeHeadersOwned, AccessControlExposeHeadersView);

impl AccessControlExposeHeadersOwned {
    /// Constructs a present field with an empty field-name list.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::AccessControlExposeHeadersOwned;
    ///
    /// let value = AccessControlExposeHeadersOwned::empty();
    /// assert!(value.is_empty());
    /// assert_eq!(value.len(), 0);
    /// ```
    pub fn empty() -> Self {
        Self(CorsList::empty())
    }
}
