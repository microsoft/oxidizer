// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A lowered `format!` call.
//!
//! Both macros end at the same place — a format string and a list of argument expressions — even
//! though they reach it differently. `#[display(...)]` lowers field names into `self`-scoped
//! accesses; `#[enrich_err(...)]` passes its literal and arguments through unchanged, because its
//! placeholders name function parameters that `rustc` resolves and its arguments are ordinary
//! expressions in the function's own scope.

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, LitStr, Token};

/// A message, ready to render.
#[derive(Debug)]
pub(crate) enum Message {
    /// Rendered as a string literal, with any `{{` and `}}` escapes already resolved.
    Literal(String),
    /// Rendered as `format!(template, arguments...)`.
    Formatted {
        /// A `format!` string.
        template: String,
        /// One expression per placeholder that consumes an argument, in order.
        arguments: Vec<TokenStream>,
    },
}

impl Message {
    /// Builds a message from a template that the caller has not interpreted.
    ///
    /// The template is rendered as a literal only when it carries no arguments and no braces at
    /// all, because a brace may open a placeholder that `format!` still has to resolve.
    #[must_use]
    pub(crate) fn opaque(template: String, arguments: Vec<TokenStream>) -> Self {
        if arguments.is_empty() && !template.contains(['{', '}']) {
            Self::Literal(template)
        } else {
            Self::Formatted { template, arguments }
        }
    }

    /// The expression that produces the message.
    ///
    /// Either a `&'static str` or a `String`; both satisfy the `Into<Cow<'static, str>>` and
    /// `Into<Cow<'_, str>>` bounds the runtime asks for.
    #[must_use]
    pub(crate) fn render(&self) -> TokenStream {
        match self {
            Self::Literal(text) => LitStr::new(text, Span::call_site()).into_token_stream(),
            Self::Formatted { template, arguments } => {
                let template = LitStr::new(template, Span::call_site());
                quote!(::std::format!(#template #(, #arguments)*))
            }
        }
    }
}

/// The arguments both macros accept: a string literal, then zero or more expressions.
#[derive(Debug)]
pub(crate) struct FormatArgs {
    /// The template, kept whole so a diagnostic can point at it.
    pub template: LitStr,
    /// The positional arguments that follow it.
    pub arguments: Vec<Expr>,
}

impl Parse for FormatArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let template = input.parse::<LitStr>()?;

        let arguments = if input.peek(Token![,]) {
            _ = input.parse::<Token![,]>()?;
            Punctuated::<Expr, Token![,]>::parse_terminated(input)?.into_iter().collect()
        } else {
            Vec::new()
        };

        Ok(Self { template, arguments })
    }
}
