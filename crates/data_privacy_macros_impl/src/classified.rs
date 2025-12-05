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

    let unnamed_fields = match &input.fields {
        Fields::Unnamed(unnamed_fields) => unnamed_fields,

        Fields::Named(fields) => {
            return Err(syn::Error::new_spanned(fields, "Named fields aren't supported"));
        }

        Fields::Unit => return Err(syn::Error::new_spanned(input, "Unit structs aren't supported")),
    };

    let field_count = unnamed_fields.unnamed.len();

    if field_count != 1 {
        return Err(syn::Error::new_spanned(unnamed_fields, "Tuple struct must have exactly one field"));
    }

    Ok(quote! {
        #input

        impl #impl_generics ::data_privacy::Classified for #struct_name #ty_generics #where_clause {
            fn data_class(&self) -> ::data_privacy::DataClass {
                #data_class.data_class()
            }
        }

        impl #impl_generics ::core::fmt::Debug for #struct_name #ty_generics #where_clause {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_fmt(format_args!("<CLASSIFIED:{}/{}>", data_privacy::Classified::data_class(self).taxonomy(), data_privacy::Classified::data_class(self).name()))
            }
        }

        impl #impl_generics ::core::fmt::Display for #struct_name #ty_generics #where_clause {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_fmt(format_args!("<CLASSIFIED:{}/{}>", data_privacy::Classified::data_class(self).taxonomy(), data_privacy::Classified::data_class(self).name()))
            }
        }

        impl #impl_generics ::data_privacy::RedactedDebug for #struct_name #ty_generics #where_clause {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "Converting from u64 to usize, value is known to be <= 128"
            )]
            fn fmt(&self, engine: &::data_privacy::RedactionEngine, output: &mut ::std::fmt::Formatter<'_>) -> ::core::fmt::Result {
                use data_privacy::Classified;

                let v = &self.0;
                let mut local_buf = [0u8; 128];
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
                    engine.redact(&self.data_class(), s, output)
                } else {
                    engine.redact(&self.data_class(), format!("{v:?}"), output)
                }
            }
        }

        impl #impl_generics ::data_privacy::RedactedDisplay for #struct_name #ty_generics #where_clause {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "Converting from u64 to usize, value is known to be <= 128"
            )]
            fn fmt(&self, engine: &::data_privacy::RedactionEngine, output: &mut ::std::fmt::Formatter) -> ::core::fmt::Result {
                use data_privacy::Classified;

                let v = &self.0;
                let mut local_buf = [0u8; 128];
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
                    engine.redact(&self.data_class(), s, output)
                } else {
                    engine.redact(&self.data_class(), format!("{v}"), output)
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
