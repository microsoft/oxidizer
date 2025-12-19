// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_auto_cfg))]

//! Macros for the [`thread_aware`](https://docs.rs/thread_aware) crate.

#![doc(
    html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware_macros_impl/logo.png"
)]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware_macros_impl/favicon.ico"
)]

// Internal implementation crate (no proc-macro entrypoints).
// Provides a parameterized function to generate a ThreadAware derive impl
// using an arbitrary crate root path

use std::collections::HashSet;

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Fields, GenericParam, Path, PathArguments, Type, TypePath, parse_quote};

mod enum_gen;

/// Public so the wrapper proc-macro crate can access `is_phantom_data`
pub mod field_attrs; // public so the wrapper proc-macro crate can access FieldAttrCfg

mod struct_gen;

use enum_gen::build_enum_body;
use field_attrs::{FieldAttrCfg, is_phantom_data};
use struct_gen::build_struct_body;

/// Core implementation used by both `thread_aware_macros` and `oxidizer_macros`.
///
/// This crate is a normal library crate (not `proc-macro`), so we operate purely
/// on `proc_macro2::TokenStream` and let the wrappers perform the conversion.
#[must_use]
pub fn derive_thread_aware(input: TokenStream2, root_path: &Path) -> TokenStream2 {
    let parsed: syn::Result<DeriveInput> = syn::parse2(input);
    parsed
        .and_then(|di| impl_transfer(&di, root_path))
        .unwrap_or_else(|e| e.to_compile_error())
}

fn impl_transfer(input: &DeriveInput, root_path: &Path) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let generics = add_bounds(input, root_path)?;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let body = match &input.data {
        Data::Struct(s) => build_struct_body(name, &s.fields, root_path)?,
        Data::Enum(e) => build_enum_body(name, e, root_path)?,
        Data::Union(_u) => {
            return Err(syn::Error::new_spanned(
                input.ident.clone(),
                "#[derive(ThreadAware)] does not support unions",
            ));
        }
    };

    // Build paths: <root_path>::ThreadAware and <root_path>::MemoryAffinity
    let mut thread_aware_path = root_path.clone();
    let mut affinity_path = root_path.clone();
    let mut pinned_affinity_path = root_path.clone();
    // Append segments manually (Paths are immutable; construct via parse_quote!)
    thread_aware_path.segments.push(parse_quote!(ThreadAware));
    affinity_path.segments.push(parse_quote!(affinity));
    pinned_affinity_path.segments.push(parse_quote!(affinity));
    affinity_path.segments.push(parse_quote!(MemoryAffinity));
    pinned_affinity_path.segments.push(parse_quote!(PinnedAffinity));

    Ok(quote! {
        impl #impl_generics #thread_aware_path for #name #ty_generics #where_clause {
            #[allow(clippy::redundant_clone, reason = "macro generated pattern moves each field once")]
            fn relocated(self, source: #affinity_path, destination: #pinned_affinity_path) -> Self {
                #body
            }
        }
    })
}

pub(crate) fn transfer_expr(ident: &syn::Ident, cfg: &FieldAttrCfg, root_path: &Path) -> TokenStream2 {
    if cfg.skip {
        quote! { #ident }
    } else {
        let mut path = root_path.clone();
        path.segments.push(parse_quote!(ThreadAware));
        quote! { #path::relocated(#ident, source, destination) }
    }
}

fn add_bounds(input: &DeriveInput, root_path: &Path) -> syn::Result<syn::Generics> {
    let mut generics = input.generics.clone();
    let used_generic_idents = match &input.data {
        Data::Struct(s) => collect_generics_in_fields(&s.fields, &generics)?,
        Data::Enum(e) => {
            let mut set = HashSet::new();
            for v in &e.variants {
                let local = collect_generics_in_fields(&v.fields, &generics)?;
                set.extend(local);
            }
            set
        }
        Data::Union(_) => HashSet::default(),
    };

    for param in &mut generics.params {
        if let GenericParam::Type(ty_param) = param
            && used_generic_idents.contains(&ty_param.ident)
        {
            let already = ty_param.bounds.iter().any(|b| {
                matches!(
                    b,
                    syn::TypeParamBound::Trait(trait_bound)
                        if trait_bound.path.segments.last().is_some_and(|seg| seg.ident == "ThreadAware")
                )
            });
            if !already {
                let mut ta_path = root_path.clone();
                ta_path.segments.push(parse_quote!(ThreadAware));
                ty_param.bounds.push(parse_quote!(#ta_path));
            }
        }
    }
    Ok(generics)
}

#[cfg_attr(coverage_nightly, coverage(off))] // can't figure out how to get to 100% coverage of this function
fn collect_generics_in_fields(fields: &Fields, generics: &syn::Generics) -> syn::Result<HashSet<syn::Ident>> {
    let mut set = HashSet::new();
    let generic_idents: HashSet<_> = generics
        .params
        .iter()
        .filter_map(|gp| match gp {
            syn::GenericParam::Type(t) => Some(t.ident.clone()),
            _ => None,
        })
        .collect();
    for ty in fields.iter().map(|f| &f.ty) {
        collect_generics_in_type(ty, &generic_idents, &mut set)?;
    }
    Ok(set)
}

#[cfg_attr(coverage_nightly, coverage(off))] // can't figure out how to get to 100% coverage of this function
fn collect_generics_in_type(ty: &Type, generic_idents: &HashSet<syn::Ident>, acc: &mut HashSet<syn::Ident>) -> syn::Result<()> {
    match ty {
        Type::Path(TypePath { path, .. }) => {
            if is_phantom_data(ty) {
                return Ok(());
            }
            for segment in &path.segments {
                if generic_idents.contains(&segment.ident) {
                    acc.insert(segment.ident.clone());
                }
                if let PathArguments::AngleBracketed(ab) = &segment.arguments {
                    for arg in &ab.args {
                        if let syn::GenericArgument::Type(t) = arg {
                            collect_generics_in_type(t, generic_idents, acc)?;
                        }
                    }
                }
            }
        }
        Type::Reference(r) => collect_generics_in_type(&r.elem, generic_idents, acc)?,
        Type::Tuple(t) => {
            for elem in &t.elems {
                collect_generics_in_type(elem, generic_idents, acc)?;
            }
        }
        Type::Array(a) => collect_generics_in_type(&a.elem, generic_idents, acc)?,
        Type::Group(g) => collect_generics_in_type(&g.elem, generic_idents, acc)?,
        Type::Paren(p) => collect_generics_in_type(&p.elem, generic_idents, acc)?,
        _ => {}
    }
    Ok(())
}

// We intentionally do not re-export FieldAttrCfg (wrapper crates access it via the module path).
