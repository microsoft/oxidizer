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

    let ReturnType::Type(_, output) = function.sig.output.clone() else {
        errors.add(
            &function.sig,
            "`#[enrich_err(...)]` needs a return type to enrich. A function returning `()` has no error to carry the message",
        );
        return passthrough(&function, &mut errors);
    };

    function.block = Box::new(wrap(&function, &output, &message));
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
/// `output` is the declared return type, which the caller has already established is present.
///
/// The message is applied through `map_err`, so the wrapper works for any return type carrying one
/// — `Result`, and the `Poll<Result<..>>` an implemented `Future::poll` returns.
fn wrap(function: &ItemFn, output: &syn::Type, message: &Message) -> syn::Block {
    let block = &function.block;
    let rendered = message.render();
    let enrichable = paths::enrichable();
    let entry = paths::enrichment_entry();

    let evaluated = if function.sig.asyncness.is_some() {
        quote!(let __ohno_result: #output = async #block.await;)
    } else {
        quote!(let __ohno_result = (|| -> #output #block)();)
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

    /// Expands the attribute and pretty-prints the result.
    ///
    /// The whole expansion is snapshotted rather than searched for substrings, because what the
    /// wrapper has to get right is the shape of the code it emits: which body runs where, what the
    /// message is built from, and which parts of the signature survive. A substring assertion can
    /// only confirm that some token is present somewhere, so it passes on an expansion that is
    /// wrong everywhere else.
    fn rendered(args: TokenStream, item: Item) -> String {
        let expanded = expand(args, item);
        let file: syn::File = syn::parse2(expanded).expect("the expansion parses as a file");
        prettyplease::unparse(&file)
    }

    #[test]
    fn a_bare_attribute_names_the_function() {
        insta::assert_snapshot!(rendered(
            TokenStream::new(),
            parse_quote! {
                fn load() -> Result<(), MyError> { Err(MyError::new()) }
            }
        ));
    }

    #[test]
    fn a_literal_message_renders_without_format() {
        insta::assert_snapshot!(rendered(
            quote!("operation failed"),
            parse_quote!(
                fn load() -> Result<(), MyError> {
                    todo!()
                }
            )
        ));
    }

    #[test]
    fn an_inline_capture_goes_through_format() {
        insta::assert_snapshot!(rendered(
            quote!("failed for {path}"),
            parse_quote!(
                fn load(path: &str) -> Result<(), MyError> {
                    todo!()
                }
            )
        ));
    }

    #[test]
    fn arguments_are_passed_through_unchanged() {
        insta::assert_snapshot!(rendered(
            quote!("read {} bytes", data.len()),
            parse_quote!(
                fn load(data: &[u8]) -> Result<(), MyError> {
                    todo!()
                }
            )
        ));
    }

    #[test]
    fn a_self_prefixed_argument_is_left_alone() {
        insta::assert_snapshot!(rendered(
            quote!("counter {}", self.counter),
            parse_quote!(
                fn load(&self) -> Result<(), MyError> {
                    todo!()
                }
            )
        ));
    }

    #[test]
    fn the_body_runs_inside_a_closure() {
        insta::assert_snapshot!(rendered(
            quote!("failed"),
            parse_quote!(
                fn load() -> Result<(), MyError> {
                    Err(MyError::new())
                }
            )
        ));
    }

    #[test]
    fn an_async_function_awaits_an_async_block() {
        insta::assert_snapshot!(rendered(
            quote!("failed"),
            parse_quote!(
                async fn load() -> Result<(), MyError> {
                    Err(MyError::new())
                }
            )
        ));
    }

    #[test]
    fn the_signature_survives_untouched() {
        insta::assert_snapshot!(rendered(
            quote!("failed"),
            parse_quote! {
                /// Documented.
                pub unsafe extern "C" fn load<T: Clone>(&mut self, value: T) -> Result<T, MyError>
                where
                    T: Send,
                { todo!() }
            }
        ));
    }

    #[test]
    fn a_missing_return_type_is_rejected() {
        insta::assert_snapshot!(rendered(
            quote!("failed"),
            parse_quote!(
                fn load() {}
            )
        ));
    }

    #[test]
    fn a_non_literal_first_argument_is_rejected() {
        insta::assert_snapshot!(rendered(
            quote!(not_a_literal),
            parse_quote!(
                fn load() -> Result<(), MyError> {
                    todo!()
                }
            )
        ));
    }

    #[test]
    fn a_non_function_is_rejected() {
        insta::assert_snapshot!(rendered(
            quote!("failed"),
            parse_quote!(
                struct T;
            )
        ));
    }
}
