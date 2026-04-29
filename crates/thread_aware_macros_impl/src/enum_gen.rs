// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::{DataEnum, Fields};

use crate::field_attrs::{FieldAttrCfg, is_phantom_data, parse_field_attrs};

pub fn build_enum_body(_name: &syn::Ident, data: &DataEnum, root_path: &syn::Path) -> syn::Result<proc_macro2::TokenStream> {
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
                    if !is_phantom_data(&f.ty) && !cfg.skip {
                        let mut path = root_path.clone();
                        path.segments.push(syn::parse_quote!(ThreadAware));
                        stmts.push(quote! { #path::relocated(#ident, source, destination); });
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
                    bindings.push(quote! { #ident });
                    if !is_phantom_data(&f.ty) && !cfg.skip {
                        let mut path = root_path.clone();
                        path.segments.push(syn::parse_quote!(ThreadAware));
                        stmts.push(quote! { #path::relocated(#ident, source, destination); });
                    }
                }
                arms.push(quote! { Self::#v_ident { #( #bindings ),* } => { #( #stmts )* } });
            }
        }
    }
    Ok(quote! { match self { #( #arms ),* } })
}
