// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::Fields;

use crate::field_attrs::{FieldAttrCfg, is_phantom_data, parse_field_attrs};
use crate::transfer_expr;

pub fn build_struct_body(name: &syn::Ident, fields: &Fields, root_path: &syn::Path) -> syn::Result<proc_macro2::TokenStream> {
    Ok(match fields {
        Fields::Named(named) => {
            let mut bindings = Vec::new();
            let mut inits = Vec::new();
            for f in &named.named {
                let ident = f.ident.as_ref().expect("Field identifier is missing");
                let attr_cfg: FieldAttrCfg = parse_field_attrs(&f.attrs)?;
                bindings.push(quote! { #ident });
                let init_expr = if is_phantom_data(&f.ty) {
                    quote! { #ident }
                } else {
                    transfer_expr(ident, &attr_cfg, root_path)
                };
                inits.push(quote! { #ident: #init_expr });
            }
            quote! { let Self { #( #bindings ),* } = self; Self { #( #inits ),* } }
        }
        Fields::Unnamed(unnamed) => {
            let mut bindings = Vec::new();
            let mut inits = Vec::new();
            for (i, f) in unnamed.unnamed.iter().enumerate() {
                let ident = syn::Ident::new(&format!("_f{i}"), proc_macro2::Span::call_site());
                let attr_cfg: FieldAttrCfg = parse_field_attrs(&f.attrs)?;
                let init_expr = if is_phantom_data(&f.ty) {
                    quote! { #ident }
                } else {
                    transfer_expr(&ident, &attr_cfg, root_path)
                };
                bindings.push(quote! { #ident });
                inits.push(init_expr);
            }
            quote! { let #name( #( #bindings ),* ) = self; #name( #( #inits ),* ) }
        }
        Fields::Unit => quote! { self },
    })
}
