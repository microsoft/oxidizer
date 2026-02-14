// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use chumsky::error::Rich;
use proc_macro2::{Span, TokenStream};

#[ohno::error]
#[display("Failed to parse URI: {errors:?}")]
pub struct ParseError<'a> {
    errors: Vec<Rich<'a, char>>,
}

impl ParseError<'_> {
    pub(crate) fn to_compile_error(&self, span: Span) -> TokenStream {
        syn::Error::new(span, self.to_string()).to_compile_error()
    }
}
