// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The trait implementations the derive owes: `Display`, `Error`, `Enrichable`, `ErrorExt` and
//! `Debug`.
//!
//! All but `Debug` delegate to the core. `OhnoCore` appends `caused by:`, the enrichment lines and
//! the backtrace itself, so the generated code decides only the message.

use proc_macro2::TokenStream;
use quote::quote;
use syn::LitStr;

use super::core_member;
use crate::derive_error::ast::Style;
use crate::derive_error::member_name;
use crate::derive_error::model::Model;
use crate::paths;

/// The type's name, which is the message the runtime falls back to.
fn default_message(model: &Model) -> LitStr {
    LitStr::new(&model.ident.to_string(), model.ident.span())
}

/// The `#[display(...)]` message as an `Option<Cow<'_, str>>`.
///
/// `Cow::from` accepts both a `&'static str` and a `String`, so a static message stays
/// allocation-free and a rendered one is owned, without the generator branching on which it is.
fn override_message(model: &Model) -> TokenStream {
    model.message.as_ref().map_or_else(
        || quote!(::core::option::Option::None),
        |message| {
            let rendered = message.render();
            quote!(::core::option::Option::Some(::std::borrow::Cow::from(#rendered)))
        },
    )
}

/// `impl Display`, which renders the message and lets the core append the rest.
#[must_use]
pub(crate) fn display(model: &Model) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = model.generics.split_for_impl();
    let ident = &model.ident;
    let core = core_member(model);
    let name = default_message(model);
    let message = override_message(model);

    quote! {
        #[automatically_derived]
        impl #impl_generics ::core::fmt::Display for #ident #ty_generics #where_clause {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                self.#core.format_error(f, #name, #message)
            }
        }
    }
}

/// `impl std::error::Error`, whose `source` is the core's.
#[must_use]
pub(crate) fn error(model: &Model) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = model.generics.split_for_impl();
    let ident = &model.ident;
    let core = core_member(model);

    quote! {
        #[automatically_derived]
        impl #impl_generics ::std::error::Error for #ident #ty_generics #where_clause {
            fn source(&self) -> ::core::option::Option<&(dyn ::std::error::Error + 'static)> {
                self.#core.source()
            }
        }
    }
}

/// `impl ohno::Enrichable`, which appends an entry to the core.
#[must_use]
pub(crate) fn enrichable(model: &Model) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = model.generics.split_for_impl();
    let ident = &model.ident;
    let core = core_member(model);
    let enrichable = paths::enrichable();
    let entry = paths::enrichment_entry();

    quote! {
        #[automatically_derived]
        impl #impl_generics #enrichable for #ident #ty_generics #where_clause {
            fn add_enrichment(&mut self, entry: #entry) {
                #enrichable::add_enrichment(&mut self.#core, entry);
            }
        }
    }
}

/// `impl ohno::ErrorExt`, whose message is the one `Display` renders.
///
/// Both read the same `override_message`, so the two agree by construction rather than by being
/// written twice.
#[must_use]
pub(crate) fn error_ext(model: &Model) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = model.generics.split_for_impl();
    let ident = &model.ident;
    let core = core_member(model);
    let name = default_message(model);
    let message = override_message(model);
    let error_ext = paths::error_ext();

    quote! {
        #[automatically_derived]
        impl #impl_generics #error_ext for #ident #ty_generics #where_clause {
            fn message(&self) -> ::std::string::String {
                self.#core.format_message(#name, #message)
            }

            fn backtrace(&self) -> &::std::backtrace::Backtrace {
                self.#core.backtrace()
            }
        }
    }
}

/// `impl Debug`, unless `#[no_debug]` was written.
///
/// Prints every field, the core included, in the shape `#[derive(Debug)]` would.
#[must_use]
pub(crate) fn debug(model: &Model) -> TokenStream {
    if !model.debug {
        return TokenStream::new();
    }

    let (impl_generics, ty_generics, where_clause) = model.generics.split_for_impl();
    let ident = &model.ident;
    let name = default_message(model);

    let body = match model.shape.style {
        Style::Named => {
            let fields = model.shape.all().map(|field| {
                let member = &field.member;
                let label = LitStr::new(&member_name(member), model.ident.span());
                quote!(.field(#label, &self.#member))
            });
            quote!(f.debug_struct(#name) #(#fields)* .finish())
        }
        Style::Tuple => {
            let fields = model.shape.all().map(|field| {
                let member = &field.member;
                quote!(.field(&self.#member))
            });
            quote!(f.debug_tuple(#name) #(#fields)* .finish())
        }
    };

    quote! {
        // Not `#[automatically_derived]`: dead-code analysis skips field reads in a derived
        // `Debug`, so marking this one would make every field that only `Debug` reads look unused
        // in the user's own crate.
        impl #impl_generics ::core::fmt::Debug for #ident #ty_generics #where_clause {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                #body
            }
        }
    }
}
