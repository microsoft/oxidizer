// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::{DataEnum, Fields};

use crate::field_attrs::{FieldAttrCfg, is_phantom_data, parse_field_attrs};
use crate::transfer_expr;

pub fn build_enum_body(_name: &syn::Ident, data: &DataEnum, root_path: &syn::Path) -> syn::Result<proc_macro2::TokenStream> {
    let mut arms = Vec::new();
    for variant in &data.variants {
        let v_ident = &variant.ident;
        match &variant.fields {
            Fields::Unit => {
                arms.push(quote! { Self::#v_ident => Self::#v_ident });
            }
            Fields::Unnamed(unnamed) => {
                let mut bindings = Vec::new();
                let mut outputs = Vec::new();
                for (i, f) in unnamed.unnamed.iter().enumerate() {
                    let ident = syn::Ident::new(&format!("_v{i}"), proc_macro2::Span::call_site());
                    let cfg: FieldAttrCfg = parse_field_attrs(&f.attrs)?;
                    bindings.push(quote! { #ident });
                    let out = if is_phantom_data(&f.ty) {
                        quote! { #ident }
                    } else {
                        transfer_expr(&ident, &cfg, root_path)
                    };
                    outputs.push(out);
                }
                arms.push(quote! { Self::#v_ident( #( #bindings ),* ) => Self::#v_ident( #( #outputs ),* ) });
            }
            Fields::Named(named) => {
                let mut bindings = Vec::new();
                let mut inits = Vec::new();
                for f in &named.named {
                    let ident = f.ident.as_ref().unwrap();
                    let cfg: FieldAttrCfg = parse_field_attrs(&f.attrs)?;
                    bindings.push(quote! { #ident });
                    let expr = if is_phantom_data(&f.ty) {
                        quote! { #ident }
                    } else {
                        transfer_expr(ident, &cfg, root_path)
                    };
                    inits.push(quote! { #ident: #expr });
                }
                arms.push(quote! { Self::#v_ident { #( #bindings ),* } => Self::#v_ident { #( #inits ),* } });
            }
        }
    }
    // NOTE: We replaced fine-grained transfer generation logic; for simplicity reuse original file directly instead.
    // This stub is not used because we retained original logic in calling site; keep for completeness if needed.
    Ok(quote! { match self { #( #arms ),* } })
}
