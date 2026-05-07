// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::Fields;

use crate::field_attrs::{FieldAttrCfg, is_phantom_data, parse_field_attrs};

pub fn build_struct_body(_name: &syn::Ident, fields: &Fields, root_path: &syn::Path) -> syn::Result<proc_macro2::TokenStream> {
    Ok(match fields {
        Fields::Named(named) => {
            let mut stmts = Vec::new();
            for f in &named.named {
                let ident = f.ident.as_ref().expect("Field identifier is missing");
                let attr_cfg: FieldAttrCfg = parse_field_attrs(&f.attrs)?;
                if !is_phantom_data(&f.ty) && !attr_cfg.skip {
                    let mut path = root_path.clone();
                    path.segments.push(syn::parse_quote!(ThreadAware));
                    stmts.push(quote! { #path::relocate(&mut self.#ident, source, destination); });
                }
            }
            quote! { #( #stmts )* }
        }
        Fields::Unnamed(unnamed) => {
            let mut stmts = Vec::new();
            for (i, f) in unnamed.unnamed.iter().enumerate() {
                let attr_cfg: FieldAttrCfg = parse_field_attrs(&f.attrs)?;
                if !is_phantom_data(&f.ty) && !attr_cfg.skip {
                    let index = syn::Index::from(i);
                    let mut path = root_path.clone();
                    path.segments.push(syn::parse_quote!(ThreadAware));
                    stmts.push(quote! { #path::relocate(&mut self.#index, source, destination); });
                }
            }
            quote! { #( #stmts )* }
        }
        Fields::Unit => quote! {},
    })
}
