// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[enrich_err(...)]`.
//!
//! The attribute wraps a function body so that a returned error gains an enrichment entry carrying
//! a message, `file!()` and `line!()`.
//!
//! It keeps no `Ast`: its whole input is a message and a signature, and the signature is re-emitted
//! rather than read, so decoding and checking fold into one step that yields a [`Message`] — the
//! same type the derive lowers `#[display(...)]` into.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::{Item, ItemFn, ReturnType};

use crate::diagnostics::Errors;
use crate::message::{FormatArgs, Message};
use crate::paths;

/// Expands the attribute, or renders everything that stopped it.
pub(crate) fn expand(args: TokenStream, item: Item) -> TokenStream {
    let mut errors = Errors::default();

    let Item::Fn(mut function) = item else {
        errors.add(&item, "`#[enrich_err(...)]` applies to functions only");
        return errors.into_compile_error();
    };

    let Some(message) = parse_message(args, &function, &mut errors) else {
        return passthrough(&function, &mut errors);
    };

    if matches!(function.sig.output, ReturnType::Default) {
        errors.add(
            &function.sig,
            "`#[enrich_err(...)]` needs a return type to enrich. A function returning `()` has no error to carry the message",
        );
        return passthrough(&function, &mut errors);
    }

    function.block = Box::new(wrap(&function, &message));
    function.into_token_stream()
}

/// Re-emits the function unchanged, alongside the diagnostics.
///
/// Emitting the function keeps `rustc` from reporting every one of its callers as unresolved on top
/// of the fault the macro already reported.
fn passthrough(function: &ItemFn, errors: &mut Errors) -> TokenStream {
    let diagnostics = std::mem::take(errors).into_compile_error();
    quote! {
        #diagnostics
        #function
    }
}

/// Decodes the attribute's arguments.
///
/// With no arguments the message names the function, which is the only thing known about it.
fn parse_message(args: TokenStream, function: &ItemFn, errors: &mut Errors) -> Option<Message> {
    if args.is_empty() {
        return Some(Message::Literal(format!("error in function {}", function.sig.ident)));
    }

    match syn::parse2::<FormatArgs>(args) {
        Ok(args) => Some(Message::opaque(
            args.template.value(),
            args.arguments.iter().map(ToTokens::to_token_stream).collect(),
        )),
        Err(error) => {
            errors.combine(error);
            None
        }
    }
}

/// Rewrites the body so that the message reaches a returned error.
///
/// The body runs inside an immediately-invoked closure, so a `?` inside it still returns from the
/// wrapped body rather than from the function. The closure is not `move`: capture is left to
/// inference, so a body that consumes `self` takes it by value while a body that only reads it
/// borrows, and the message can still name a parameter the body did not consume.
///
/// The message is applied through `map_err`, so the wrapper works for any return type carrying one
/// — `Result`, and the `Poll<Result<..>>` an implemented `Future::poll` returns.
fn wrap(function: &ItemFn, message: &Message) -> syn::Block {
    let block = &function.block;
    let output = &function.sig.output;
    let rendered = message.render();
    let enrichable = paths::enrichable();
    let entry = paths::enrichment_entry();

    let evaluated = if function.sig.asyncness.is_some() {
        let ReturnType::Type(_, ty) = output else {
            unreachable!("a missing return type is rejected before the body is rewritten")
        };
        quote!(let __ohno_result: #ty = async #block.await;)
    } else {
        quote!(let __ohno_result = (|| #output #block)();)
    };

    syn::parse_quote!({
        #evaluated
        __ohno_result.map_err(|mut __ohno_error| {
            #enrichable::add_enrichment(
                &mut __ohno_error,
                #entry::new(#rendered, ::core::file!(), ::core::line!()),
            );
            __ohno_error
        })
    })
}

#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::parse_quote;

    use super::*;

    fn expand_of(args: TokenStream, item: Item) -> String {
        expand(args, item).to_string()
    }

    #[test]
    fn a_bare_attribute_names_the_function() {
        let expanded = expand_of(
            TokenStream::new(),
            parse_quote! {
                fn load() -> Result<(), MyError> { Err(MyError::new()) }
            },
        );

        assert!(expanded.contains(r#""error in function load""#), "{expanded}");
        assert!(!expanded.contains("compile_error"), "{expanded}");
    }

    #[test]
    fn a_literal_message_renders_without_format() {
        let expanded = expand_of(
            quote!("operation failed"),
            parse_quote!(
                fn load() -> Result<(), MyError> {
                    todo!()
                }
            ),
        );

        assert!(expanded.contains(r#""operation failed""#), "{expanded}");
        assert!(!expanded.contains("format"), "{expanded}");
    }

    #[test]
    fn an_inline_capture_goes_through_format() {
        let expanded = expand_of(
            quote!("failed for {path}"),
            parse_quote!(
                fn load(path: &str) -> Result<(), MyError> {
                    todo!()
                }
            ),
        );

        assert!(expanded.contains("format"), "{expanded}");
        assert!(expanded.contains(r#""failed for {path}""#), "{expanded}");
    }

    #[test]
    fn arguments_are_passed_through_unchanged() {
        let expanded = expand_of(
            quote!("read {} bytes", data.len()),
            parse_quote!(
                fn load(data: &[u8]) -> Result<(), MyError> {
                    todo!()
                }
            ),
        );

        assert!(expanded.contains("data . len ()"), "{expanded}");
    }

    #[test]
    fn a_self_prefixed_argument_is_left_alone() {
        let expanded = expand_of(
            quote!("counter {}", self.counter),
            parse_quote!(
                fn load(&self) -> Result<(), MyError> {
                    todo!()
                }
            ),
        );

        assert!(expanded.contains("self . counter"), "{expanded}");
        assert!(!expanded.contains("compile_error"), "{expanded}");
    }

    #[test]
    fn the_body_runs_inside_a_closure() {
        let expanded = expand_of(
            quote!("failed"),
            parse_quote!(
                fn load() -> Result<(), MyError> {
                    Err(MyError::new())
                }
            ),
        );

        assert!(expanded.contains("(| | -> Result < () , MyError >"), "{expanded}");
        assert!(expanded.contains("map_err"), "{expanded}");
    }

    #[test]
    fn an_async_function_awaits_an_async_block() {
        let expanded = expand_of(
            quote!("failed"),
            parse_quote!(
                async fn load() -> Result<(), MyError> {
                    Err(MyError::new())
                }
            ),
        );

        assert!(expanded.contains("async"), "{expanded}");
        assert!(expanded.contains(". await"), "{expanded}");
        assert!(expanded.contains("__ohno_result : Result < () , MyError >"), "{expanded}");
    }

    #[test]
    fn the_signature_survives_untouched() {
        let expanded = expand_of(
            quote!("failed"),
            parse_quote! {
                /// Documented.
                pub unsafe extern "C" fn load<T: Clone>(&mut self, value: T) -> Result<T, MyError>
                where
                    T: Send,
                { todo!() }
            },
        );

        for expected in ["Documented.", "pub unsafe extern \"C\" fn load", "< T : Clone >", "where T : Send"] {
            assert!(expanded.contains(expected), "missing {expected} in {expanded}");
        }
    }

    #[test]
    fn a_missing_return_type_is_rejected() {
        let expanded = expand_of(
            quote!("failed"),
            parse_quote!(
                fn load() {}
            ),
        );

        assert!(expanded.contains("needs a return type"), "{expanded}");
        assert!(expanded.contains("fn load"), "{expanded}");
    }

    #[test]
    fn a_non_literal_first_argument_is_rejected() {
        let expanded = expand_of(
            quote!(not_a_literal),
            parse_quote!(
                fn load() -> Result<(), MyError> {
                    todo!()
                }
            ),
        );

        assert!(expanded.contains("compile_error"), "{expanded}");
        assert!(expanded.contains("fn load"), "{expanded}");
    }

    #[test]
    fn a_non_function_is_rejected() {
        let expanded = expand_of(
            quote!("failed"),
            parse_quote!(
                struct T;
            ),
        );

        assert!(expanded.contains("applies to functions only"), "{expanded}");
    }
}
