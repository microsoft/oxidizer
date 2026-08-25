// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::{DataEnum, Fields};

use crate::field_attrs::{FieldAttrCfg, parse_field_attrs};

pub(crate) fn build_enum_body(_name: &syn::Ident, data: &DataEnum, root_path: &syn::Path) -> syn::Result<proc_macro2::TokenStream> {
    // An enum with no variants is uninhabited, but `self` is a `&mut` reference, which rustc
    // always treats as inhabited - so `match self {}` is rejected as non-exhaustive. There is
    // nothing to relocate either way, so emit no body at all.
    if data.variants.is_empty() {
        return Ok(proc_macro2::TokenStream::new());
    }

    let mut arms = Vec::new();
    for variant in &data.variants {
        let v_ident = &variant.ident;
        match &variant.fields {
            Fields::Unit => {
                arms.push(quote! { Self::#v_ident => {} });
            }
            Fields::Unnamed(unnamed) => {
                let mut bindings = Vec::new();
                let mut stmts = Vec::new();
                for (i, f) in unnamed.unnamed.iter().enumerate() {
                    let ident = syn::Ident::new(&format!("_v{i}"), proc_macro2::Span::call_site());
                    let cfg: FieldAttrCfg = parse_field_attrs(&f.attrs)?;
                    bindings.push(quote! { #ident });
                    if !cfg.skip {
                        let mut path = root_path.clone();
                        path.segments.push(syn::parse_quote!(ThreadAware));
                        stmts.push(quote! { #path::relocate(#ident, source, destination); });
                    }
                }
                arms.push(quote! { Self::#v_ident( #( #bindings ),* ) => { #( #stmts )* } });
            }
            Fields::Named(named) => {
                let mut bindings = Vec::new();
                let mut stmts = Vec::new();
                for f in &named.named {
                    let ident = f.ident.as_ref().expect("Field identifier is missing");
                    let cfg: FieldAttrCfg = parse_field_attrs(&f.attrs)?;
                    if cfg.skip {
                        // Bind to `_` rather than to the name: the arm emits no statement for
                        // this field, and a named binding that is never read makes the
                        // generated code warn, which breaks a downstream `deny(warnings)`.
                        bindings.push(quote! { #ident: _ });
                    } else {
                        bindings.push(quote! { #ident });
                        let mut path = root_path.clone();
                        path.segments.push(syn::parse_quote!(ThreadAware));
                        stmts.push(quote! { #path::relocate(#ident, source, destination); });
                    }
                }
                arms.push(quote! { Self::#v_ident { #( #bindings ),* } => { #( #stmts )* } });
            }
        }
    }
    Ok(quote! { match self { #( #arms ),* } })
}
