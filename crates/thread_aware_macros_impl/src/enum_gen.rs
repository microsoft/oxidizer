// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::{DataEnum, Fields};

use crate::field_attrs::{FieldAttrCfg, parse_field_attrs};
use crate::param_idents;

/// Builds the identifier a generated pattern binds a field to.
///
/// The name is deliberately obscure. A binding cannot shadow a `const`, `static` or const
/// parameter of the same name that is in scope at the use site: a `const` or const parameter
/// is read as a pattern referring to that item rather than as a new binding, and a `static`
/// may not be shadowed at all. Either way the generated arm fails to compile, and macro
/// hygiene does not prevent it: under `Span::mixed_site()` the pattern still resolves against
/// the item. A name no caller would plausibly declare is the only guard available, which is
/// why `serde` generates `__field0` rather than `field0`.
fn field_binding(index: usize) -> syn::Ident {
    syn::Ident::new(&format!("__thread_aware_field_{index}"), proc_macro2::Span::call_site())
}

pub(crate) fn build_enum_body(_name: &syn::Ident, data: &DataEnum, root_path: &syn::Path) -> syn::Result<proc_macro2::TokenStream> {
    let (source, destination) = param_idents();

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
                    let cfg: FieldAttrCfg = parse_field_attrs(&f.attrs)?;
                    if cfg.skip {
                        // Match the position without binding it, as the named path does. A
                        // binding here would be read by nothing, and every generated name is
                        // one more chance to collide with a caller's constant.
                        bindings.push(quote! { _ });
                    } else {
                        let ident = field_binding(i);
                        bindings.push(quote! { ref mut #ident });
                        let mut path = root_path.clone();
                        path.segments.push(syn::parse_quote!(ThreadAware));
                        stmts.push(quote! { #path::relocate(#ident, #source, #destination); });
                    }
                }
                arms.push(quote! { Self::#v_ident( #( #bindings ),* ) => { #( #stmts )* } });
            }
            Fields::Named(named) => {
                let mut bindings = Vec::new();
                let mut stmts = Vec::new();
                for (i, f) in named.named.iter().enumerate() {
                    let ident = f.ident.as_ref().expect("Field identifier is missing");
                    let cfg: FieldAttrCfg = parse_field_attrs(&f.attrs)?;
                    if cfg.skip {
                        // Bind to `_` rather than to the name: the arm emits no statement for
                        // this field, and a named binding that is never read makes the
                        // generated code warn, which breaks a downstream `deny(warnings)`.
                        bindings.push(quote! { #ident: _ });
                    } else {
                        // Rebind to a name of the derive's own choosing rather than using the
                        // field-name shorthand, so that no field name can reach the relocation
                        // call. A field spelled like one of the generated parameters would
                        // otherwise shadow it, and the call would pass the field where an
                        // `Affinity` is expected.
                        let binding = field_binding(i);
                        bindings.push(quote! { #ident: ref mut #binding });
                        let mut path = root_path.clone();
                        path.segments.push(syn::parse_quote!(ThreadAware));
                        stmts.push(quote! { #path::relocate(#binding, #source, #destination); });
                    }
                }
                arms.push(quote! { Self::#v_ident { #( #bindings ),* } => { #( #stmts )* } });
            }
        }
    }

    // `match *self` with explicit `ref mut` bindings, rather than `match self` relying on
    // default binding modes. A bare identifier pattern whose name is taken by a `const` in
    // scope is resolved as a reference to that constant rather than as a binding, and the arm
    // then fails somewhere unhelpful - a type mismatch, or a non-exhaustive match. `ref mut`
    // cannot be read that way, so rustc reports `E0530` naming the shadowed item instead.
    Ok(quote! { match *self { #( #arms ),* } })
}
