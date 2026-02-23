// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Macros supporting the `#[classified]` attribute.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parse;
use syn::spanned::Spanned;
use syn::{Fields, ItemStruct, Path, parse2};

type SynResult<T> = Result<T, syn::Error>;

struct MacroArgs {
    data_class: Path,
}

impl MacroArgs {
    pub fn parse(attr_args: TokenStream) -> SynResult<Self> {
        if attr_args.is_empty() {
            Err(syn::Error::new(
                attr_args.span(),
                "classified attribute requires a taxonomy and data class name argument",
            ))
        } else {
            parse2(attr_args)
        }
    }
}

impl Parse for MacroArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let data_class: Path = input.parse()?;
        Ok(Self { data_class })
    }
}

fn resolve_field(input: &ItemStruct) -> SynResult<(TokenStream, TokenStream)> {
    match &input.fields {
        Fields::Unnamed(unnamed_fields) => {
            if unnamed_fields.unnamed.len() != 1 {
                return Err(syn::Error::new_spanned(unnamed_fields, "Tuple struct must have exactly one field"));
            }
            let field_type = &unnamed_fields.unnamed.first().unwrap().ty;
            Ok((quote!(self.0), quote!(#field_type)))
        }

        Fields::Named(named_fields) => {
            if named_fields.named.len() != 1 {
                return Err(syn::Error::new_spanned(named_fields, "Struct must have exactly one field"));
            }
            let field = named_fields.named.first().unwrap();
            let field_name = field.ident.as_ref().unwrap();
            let field_type = &field.ty;
            Ok((quote!(self.#field_name), quote!(#field_type)))
        }

        Fields::Unit => Err(syn::Error::new_spanned(input, "Unit structs aren't supported")),
    }
}

#[expect(
    missing_docs,
    clippy::missing_errors_doc,
    reason = "this is documented in the data_privacy reexport"
)]
pub fn classified(attr_args: TokenStream, item: TokenStream) -> SynResult<TokenStream> {
    let macro_args = MacroArgs::parse(attr_args)?;
    let input: ItemStruct = parse2(item)?;

    let struct_name = &input.ident;
    let data_class = macro_args.data_class;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let (field_access, field_type) = resolve_field(&input)?;

    let existing_predicates = generics.where_clause.as_ref().map(|w| &w.predicates);
    let debug_where_clause = quote!(where #existing_predicates #field_type: ::core::fmt::Debug);
    let display_where_clause = quote!(where #existing_predicates #field_type: ::core::fmt::Display);

    Ok(quote! {
        #input

        impl #impl_generics ::data_privacy::Classified for #struct_name #ty_generics #where_clause {
            fn data_class(&self) -> &::data_privacy::DataClass {
                #data_class.as_ref()
            }
        }

        impl #impl_generics ::core::fmt::Debug for #struct_name #ty_generics #where_clause {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                let dc = <Self as ::data_privacy::Classified>::data_class(self);
                write!(f, "<CLASSIFIED:{}/{}>", dc.taxonomy(), dc.name())
            }
        }

        impl #impl_generics ::core::fmt::Display for #struct_name #ty_generics #where_clause {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                let dc = <Self as ::data_privacy::Classified>::data_class(self);
                write!(f, "<CLASSIFIED:{}/{}>", dc.taxonomy(), dc.name())
            }
        }

        impl #impl_generics ::data_privacy::RedactedDebug for #struct_name #ty_generics #debug_where_clause {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "Converting from u64 to usize, value is known to be <= STACK_BUFFER_SIZE"
            )]
            fn fmt(&self, engine: &::data_privacy::RedactionEngine, output: &mut ::std::fmt::Formatter<'_>) -> ::core::fmt::Result {
                const STACK_BUFFER_SIZE: usize = 128;
                let v = &#field_access;
                let dc = <Self as ::data_privacy::Classified>::data_class(self);
                let mut local_buf = [0u8; STACK_BUFFER_SIZE];
                let amount = {
                    let mut cursor = ::std::io::Cursor::new(&mut local_buf[..]);
                    if ::std::io::Write::write_fmt(&mut cursor, format_args!("{v:?}")).is_ok() {
                        cursor.position() as usize
                    } else {
                        local_buf.len() + 1 // force fallback case on write errors
                    }
                };
                if amount <= local_buf.len() {
                    let s = unsafe { ::core::str::from_utf8_unchecked(&local_buf[..amount]) };
                    engine.redact(dc, s, output)
                } else {
                    engine.redact(dc, format!("{v:?}"), output)
                }
            }
        }

        impl #impl_generics ::data_privacy::RedactedDisplay for #struct_name #ty_generics #display_where_clause {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "Converting from u64 to usize, value is known to be <= STACK_BUFFER_SIZE"
            )]
            fn fmt(&self, engine: &::data_privacy::RedactionEngine, output: &mut ::std::fmt::Formatter) -> ::core::fmt::Result {
                const STACK_BUFFER_SIZE: usize = 128;
                let v = &#field_access;
                let dc = <Self as ::data_privacy::Classified>::data_class(self);
                let mut local_buf = [0u8; STACK_BUFFER_SIZE];
                let amount = {
                    let mut cursor = ::std::io::Cursor::new(&mut local_buf[..]);
                    if ::std::io::Write::write_fmt(&mut cursor, format_args!("{v}")).is_ok() {
                        cursor.position() as usize
                    } else {
                        local_buf.len() + 1 // force fallback case on write errors
                    }
                };
                if amount <= local_buf.len() {
                    let s = unsafe { ::core::str::from_utf8_unchecked(&local_buf[..amount]) };
                    engine.redact(dc, s, output)
                } else {
                    engine.redact(dc, format!("{v}"), output)
                }
            }
        }

        impl #impl_generics core::ops::Deref for #struct_name #ty_generics #where_clause {
            type Target = ::core::convert::Infallible;

            fn deref(&self) -> &Self::Target {
                panic!("Deref to Infallible should never happen")
            }
        }

        impl #impl_generics ::core::ops::DerefMut for #struct_name #ty_generics #where_clause {
            fn deref_mut(&mut self) -> &mut Self::Target {
                panic!("Deref to Infallible should never happen")
            }
        }
    })
}
