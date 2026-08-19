// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

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
use quote::{ToTokens, quote};
use syn::{Data, DeriveInput, Fields, GenericParam, Path, PathArguments, Type, TypePath, parse_quote};

mod enum_gen;

/// Public so the wrapper proc-macro crate can access `is_phantom_data`
pub mod field_attrs; // public so the wrapper proc-macro crate can access FieldAttrCfg

mod struct_gen;

use enum_gen::build_enum_body;
use field_attrs::{is_phantom_data, parse_field_attrs};
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

    // Build paths: <root_path>::ThreadAware and <root_path>::affinity::Affinity
    let mut thread_aware_path = root_path.clone();
    let mut affinity_path = root_path.clone();
    // Append segments manually (Paths are immutable; construct via parse_quote!)
    thread_aware_path.segments.push(parse_quote!(ThreadAware));
    affinity_path.segments.push(parse_quote!(affinity));
    affinity_path.segments.push(parse_quote!(Affinity));

    Ok(quote! {
        impl #impl_generics #thread_aware_path for #name #ty_generics #where_clause {
            fn relocate(&mut self, source: Option<#affinity_path>, destination: #affinity_path) {
                #body
            }
        }
    })
}

fn add_bounds(input: &DeriveInput, root_path: &Path) -> syn::Result<syn::Generics> {
    let mut generics = input.generics.clone();
    let usage = match &input.data {
        Data::Struct(s) => collect_generics_in_fields(&s.fields, &generics)?,
        Data::Enum(e) => {
            let mut usage = GenericUsage::default();
            for v in &e.variants {
                let local = collect_generics_in_fields(&v.fields, &generics)?;
                usage.merge(local);
            }
            usage
        }
        Data::Union(_) => GenericUsage::default(),
    };

    for param in &mut generics.params {
        let GenericParam::Type(ty_param) = param else {
            continue;
        };

        if usage.relocated.contains(&ty_param.ident) {
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

    // Types that are present but never relocated still have to be `Send`, because the
    // generated impl must satisfy the `ThreadAware: Send` supertrait.
    //
    // The obligation is placed on the type itself rather than on its type parameters.
    // Reducing `PhantomData<X>` to the parameters named inside `X` and binding each by
    // `Send` is not sound: `&'a T` is `Send` only when `T: Sync`, `Arc<T>` only when
    // `T: Send + Sync`, and `<T as Tr>::Assoc` says nothing about `T` at all. Deferring
    // to the compiler gets every shape right and needs no per-shape reasoning.
    if !usage.send_required.is_empty() {
        let where_clause = generics.make_where_clause();
        for ty in &usage.send_required {
            where_clause.predicates.push(parse_quote!(#ty: ::core::marker::Send));
        }
    }

    Ok(generics)
}

/// How each field contributes to the bounds of the generated impl.
///
/// The two categories carry different obligations: a type parameter reachable through a
/// relocated field must implement `ThreadAware`, while a field that is never relocated
/// only has to be `Send`.
#[derive(Default)]
struct GenericUsage {
    /// Type parameters reachable through a field that is actually relocated.
    relocated: HashSet<syn::Ident>,

    /// Types that are never relocated and so only need a `Send` bound, in the order
    /// they were found.
    send_required: Vec<Type>,

    /// Rendered form of everything in `send_required`, used to suppress duplicates.
    seen_send: HashSet<String>,
}

impl GenericUsage {
    fn require_send(&mut self, ty: &Type) {
        if self.seen_send.insert(ty.to_token_stream().to_string()) {
            self.send_required.push(ty.clone());
        }
    }

    fn merge(&mut self, other: Self) {
        self.relocated.extend(other.relocated);
        for ty in &other.send_required {
            self.require_send(ty);
        }
    }
}

/// Reports whether `ty` names any of the type's own generic parameters.
///
/// Scans tokens rather than matching on [`Type`] variants so that every shape is covered,
/// including qualified paths, bare functions and const-generic expressions. Over-reporting
/// is harmless: it only ever adds a `Send` predicate that is already satisfied.
fn mentions_generic(ty: &Type, generic_idents: &HashSet<syn::Ident>) -> bool {
    fn scan(tokens: TokenStream2, generic_idents: &HashSet<syn::Ident>) -> bool {
        tokens.into_iter().any(|tree| match tree {
            proc_macro2::TokenTree::Ident(ident) => generic_idents.contains(&ident),
            proc_macro2::TokenTree::Group(group) => scan(group.stream(), generic_idents),
            _ => false,
        })
    }

    scan(ty.to_token_stream(), generic_idents)
}

/// Returns the type argument of a `PhantomData<..>`, or `None` if it has none.
fn phantom_data_argument(ty: &Type) -> Option<&Type> {
    let Type::Path(TypePath { path, .. }) = ty else {
        return None;
    };
    let PathArguments::AngleBracketed(ab) = &path.segments.last()?.arguments else {
        return None;
    };
    ab.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}

#[cfg_attr(coverage_nightly, coverage(off))] // can't figure out how to get to 100% coverage of this function
fn collect_generics_in_fields(fields: &Fields, generics: &syn::Generics) -> syn::Result<GenericUsage> {
    let mut usage = GenericUsage::default();
    let generic_idents: HashSet<_> = generics
        .params
        .iter()
        .filter_map(|gp| match gp {
            syn::GenericParam::Type(t) => Some(t.ident.clone()),
            _ => None,
        })
        .collect();
    for field in fields {
        // A skipped field is never relocated, so it needs no `ThreadAware` bound - but it
        // is still part of the type, so it must be `Send` like any other unrelocated field.
        if parse_field_attrs(&field.attrs)?.skip {
            if mentions_generic(&field.ty, &generic_idents) {
                usage.require_send(&field.ty);
            }
            continue;
        }
        collect_generics_in_type(&field.ty, &generic_idents, &mut usage)?;
    }
    Ok(usage)
}

#[cfg_attr(coverage_nightly, coverage(off))] // can't figure out how to get to 100% coverage of this function
fn collect_generics_in_type(ty: &Type, generic_idents: &HashSet<syn::Ident>, acc: &mut GenericUsage) -> syn::Result<()> {
    match ty {
        Type::Path(TypePath { path, .. }) => {
            if is_phantom_data(ty) {
                // Not relocated, so nothing here needs `ThreadAware`. Bind the marker's
                // argument instead of dropping it, which is what left phantom-only
                // parameters unbound.
                //
                // The predicate goes on the argument `X` rather than on `PhantomData<X>`:
                // `X: Send` yields both `PhantomData<X>: Send` (for the supertrait) and
                // `PhantomData<X>: ThreadAware` (when the marker is nested inside a
                // relocated field), whereas `PhantomData<X>: Send` yields neither.
                if let Some(argument) = phantom_data_argument(ty)
                    && mentions_generic(argument, generic_idents)
                {
                    acc.require_send(argument);
                }
                return Ok(());
            }
            for segment in &path.segments {
                if generic_idents.contains(&segment.ident) {
                    acc.relocated.insert(segment.ident.clone());
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
