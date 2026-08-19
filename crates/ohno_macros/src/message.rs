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
pub(crate) struct FormatArgs {
    /// The template, kept whole so a diagnostic can point at it.
    pub(crate) template: LitStr,
    /// The positional arguments that follow it.
    pub(crate) arguments: Vec<Expr>,
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

#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn a_plain_template_renders_as_a_literal() {
        let rendered = Message::opaque("plain text".to_owned(), Vec::new()).render().to_string();
        assert_eq!(rendered, r#""plain text""#);
    }

    #[test]
    fn a_template_with_braces_renders_through_format() {
        let rendered = Message::opaque("value {name}".to_owned(), Vec::new()).render().to_string();
        assert!(rendered.contains("format"), "{rendered}");
        assert!(rendered.contains(r#""value {name}""#), "{rendered}");
    }

    #[test]
    fn arguments_force_a_format_call() {
        let rendered = Message::opaque("plain".to_owned(), vec![quote!(x)]).render().to_string();
        assert!(rendered.contains("format"), "{rendered}");
        assert!(rendered.ends_with("x)"), "{rendered}");
    }

    #[test]
    fn a_literal_message_renders_without_format() {
        let rendered = Message::Literal("only { one brace".to_owned()).render().to_string();
        assert!(!rendered.contains("format"), "{rendered}");
    }

    #[test]
    fn format_args_accept_a_bare_template() {
        let args: FormatArgs = parse_quote!("just text");
        assert_eq!(args.template.value(), "just text");
        assert!(args.arguments.is_empty());
    }

    #[test]
    fn format_args_accept_a_template_and_arguments() {
        let args: FormatArgs = parse_quote!("text {} {}", first, second.len());
        assert_eq!(args.template.value(), "text {} {}");
        assert_eq!(args.arguments.len(), 2);
    }

    #[test]
    fn format_args_accept_a_trailing_comma() {
        let args: FormatArgs = parse_quote!("text {}", first,);
        assert_eq!(args.arguments.len(), 1);
    }

    #[test]
    fn format_args_reject_a_leading_expression() {
        let parsed = syn::parse2::<FormatArgs>(quote!(not_a_literal));
        assert!(parsed.is_err());
    }
}
