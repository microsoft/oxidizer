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
#[must_use]
pub(crate) fn expand(args: TokenStream, item: Item) -> TokenStream {
    let mut errors = Errors::default();

    let Item::Fn(mut function) = item else {
        errors.add(&item, "`#[enrich_err(...)]` applies to functions only");
        return errors.into_compile_error();
    };

    let Some(message) = parse_message(args, &function, &mut errors) else {
        return passthrough(&function, &mut errors);
    };

    let ReturnType::Type(..) = function.sig.output else {
        errors.add(
            &function.sig,
            "`#[enrich_err(...)]` needs a return type to enrich. A function returning `()` has no error to carry the message",
        );
        return passthrough(&function, &mut errors);
    };

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
/// Neither arm names the declared return type. The wrapper's tail is the function's return
/// expression, so inference flows back from the signature on its own, and naming the type would put
/// it in a closure return type or a `let` annotation — positions an opaque type is not allowed in.
///
/// The message is applied through `map_err`, so the wrapper works for any return type carrying one
/// — `Result`, and the `Poll<Result<..>>` an implemented `Future::poll` returns.
fn wrap(function: &ItemFn, message: &Message) -> syn::Block {
    let block = &function.block;
    let rendered = message.render();
    let enrichable = paths::enrichable();
    let entry = paths::enrichment_entry();

    let evaluated = if function.sig.asyncness.is_some() {
        quote!(let __ohno_result = async #block.await;)
    } else {
        quote!(let __ohno_result = (|| #block)();)
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
