// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::iter::FusedIterator;
use std::{fmt, slice};

use super::super::FieldNameView;
use super::cors_tokens::CorsTokens;
use crate::FieldValue;

/// Borrowed field-name iterator for an owned CORS header-name list.
///
/// Names retain their original spelling and order. Empty members are skipped.
///
/// # Examples
///
/// ```
/// use http_headers::headers::{AccessControlAllowHeadersOwned, CorsHeaderNames};
///
/// let value = AccessControlAllowHeadersOwned::from_header_names(["X-Trace", "content-type"])?;
/// let names: CorsHeaderNames<'_> = (&value).into_iter();
/// assert_eq!(
///     names.map(|name| name.as_str()).collect::<Vec<_>>(),
///     ["X-Trace", "content-type"]
/// );
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct CorsHeaderNames<'a> {
    tokens: CorsTokens<'a>,
}

impl<'a> CorsHeaderNames<'a> {
    pub(super) const fn new(values: slice::Iter<'a, FieldValue>) -> Self {
        Self {
            tokens: CorsTokens::new(values),
        }
    }
}

impl fmt::Debug for CorsHeaderNames<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CorsHeaderNames").finish_non_exhaustive()
    }
}

impl<'a> Iterator for CorsHeaderNames<'a> {
    type Item = FieldNameView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.tokens.next().map(FieldNameView::from_validated)
    }
}

impl FusedIterator for CorsHeaderNames<'_> {}
