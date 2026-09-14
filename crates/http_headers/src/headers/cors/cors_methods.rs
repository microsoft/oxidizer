// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::iter::FusedIterator;
use std::{fmt, slice};

use super::super::MethodView;
use super::cors_tokens::CorsTokens;
use crate::FieldValue;

/// Borrowed method iterator for an owned CORS method list.
///
/// Methods retain their original spelling and order. Empty members are skipped.
///
/// # Examples
///
/// ```
/// use http_headers::headers::{AccessControlAllowMethodsOwned, CorsMethods};
///
/// let value = AccessControlAllowMethodsOwned::from_methods(["GET", "X-CUSTOM"])?;
/// let methods: CorsMethods<'_> = (&value).into_iter();
/// assert_eq!(
///     methods.map(|method| method.as_str()).collect::<Vec<_>>(),
///     ["GET", "X-CUSTOM"]
/// );
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct CorsMethods<'a> {
    tokens: CorsTokens<'a>,
}

impl<'a> CorsMethods<'a> {
    pub(super) const fn new(values: slice::Iter<'a, FieldValue>) -> Self {
        Self {
            tokens: CorsTokens::new(values),
        }
    }
}

impl fmt::Debug for CorsMethods<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CorsMethods").finish_non_exhaustive()
    }
}

impl<'a> Iterator for CorsMethods<'a> {
    type Item = MethodView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.tokens.next().map(MethodView::from_validated)
    }
}

impl FusedIterator for CorsMethods<'_> {}
