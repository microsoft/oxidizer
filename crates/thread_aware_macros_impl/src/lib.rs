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
use quote::quote;
use syn::{Data, DeriveInput, Fields, GenericParam, Path, PathArguments, Type, TypePath, parse_quote};

mod enum_gen;

/// Field attribute parsing for the derive.
///
/// Private: `derive_thread_aware` is this crate's entire public surface, so the parser stays
/// an implementation detail rather than semver-stable API.
mod field_attrs;

mod struct_gen;

use enum_gen::build_enum_body;
use field_attrs::parse_field_attrs;
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

    // Build paths: <root_path>::ThreadAware and <root_path>::Thread
    let mut thread_aware_path = root_path.clone();
    let mut thread_path = root_path.clone();
    // Append segments manually (Paths are immutable; construct via parse_quote!)
    thread_aware_path.segments.push(parse_quote!(ThreadAware));
    thread_path.segments.push(parse_quote!(Thread));

    let (source_ident, destination_ident) = param_idents();

    Ok(quote! {
        impl #impl_generics #thread_aware_path for #name #ty_generics #where_clause {
            fn relocate(
                &mut self,
                #source_ident: ::core::option::Option<&#thread_path>,
                #destination_ident: &#thread_path,
            ) {
                #body
            }
        }
    })
}

/// Names of the two parameters of the generated `relocate` method.
///
/// Deliberately obscure, for the same reason the field bindings are: a `const`, `static` or
/// const parameter of the same name that is in scope at the use site is not shadowed by a
/// function parameter. A `const` or const parameter is read as a pattern referring to that
/// item, and a `static` may not be shadowed at all; a type declaring `const source: usize`
/// would otherwise fail to derive. The trait's own declaration keeps the readable names; an
/// impl need not repeat them.
pub(crate) fn param_idents() -> (syn::Ident, syn::Ident) {
    (
        syn::Ident::new("__thread_aware_source", proc_macro2::Span::call_site()),
        syn::Ident::new("__thread_aware_destination", proc_macro2::Span::call_site()),
    )
}

fn add_bounds(input: &DeriveInput, root_path: &Path) -> syn::Result<syn::Generics> {
    let mut generics = input.generics.clone();
    let mut usage = GenericUsage::default();
    match &input.data {
        Data::Struct(s) => collect_generics_in_fields(&s.fields, &generics, &mut usage)?,
        Data::Enum(e) => {
            for v in &e.variants {
                collect_generics_in_fields(&v.fields, &generics, &mut usage)?;
            }
        }
        Data::Union(_) => {}
    }

    let mut thread_aware_path = root_path.clone();
    thread_aware_path.segments.push(parse_quote!(ThreadAware));

    for param in &mut generics.params {
        let GenericParam::Type(ty_param) = param else {
            continue;
        };

        if usage.relocated.contains(&ty_param.ident) {
            let already = ty_param
                .bounds
                .iter()
                .any(|b| matches!(b, syn::TypeParamBound::Trait(t) if is_same_trait(&t.path, &thread_aware_path)));
            if !already {
                ty_param.bounds.push(parse_quote!(#thread_aware_path));
            }
        }
    }

    // A `#[thread_aware(skip)]` field is never relocated, so it gains no `ThreadAware` bound,
    // but the `ThreadAware: Send` supertrait still has to hold.
    //
    // The obligation is stated once, on `Self`, which is exactly what the supertrait requires
    // and is discharged either structurally or by a manual `unsafe impl Send`. A predicate on
    // the field type would be strictly stronger: a type that is `Send` only through such an
    // `unsafe impl` would carry something like `where *const T: Send`, which no instantiation
    // can prove.
    if usage.has_skipped_field {
        let name = &input.ident;
        let (_, ty_generics, _) = input.generics.split_for_impl();
        let self_ty: Type = parse_quote!(#name #ty_generics);
        generics
            .make_where_clause()
            .predicates
            .push(parse_quote!(#self_ty: ::core::marker::Send));
    }

    Ok(generics)
}

/// Reports whether `candidate` names the same trait the derive would emit.
///
/// Compares every segment ident rather than only the last, so an unrelated
/// `some_crate::ThreadAware` is not mistaken for the real trait.
///
/// A bare single-segment `ThreadAware` is accepted as the real trait, deliberately: the name
/// alone cannot distinguish the imported trait from one of the author's own, and the imported
/// form is what real code writes. Rejecting it would emit a second bound beside the author's
/// own `T: ThreadAware`, tripping `clippy::trait_duplication_in_bounds` at their declaration.
/// Qualify either path to disambiguate the rarer case.
fn is_same_trait(candidate: &Path, emitted: &Path) -> bool {
    let candidate_idents: Vec<_> = candidate.segments.iter().map(|s| s.ident.to_string()).collect();
    let emitted_idents: Vec<_> = emitted.segments.iter().map(|s| s.ident.to_string()).collect();

    candidate_idents == emitted_idents || candidate_idents == ["ThreadAware"]
}

/// How the fields of a type contribute to the bounds of the generated impl.
#[derive(Default)]
struct GenericUsage {
    /// Type parameters the traversal reaches through a relocated field; each is bound by
    /// `ThreadAware`.
    relocated: HashSet<syn::Ident>,

    /// Whether any field carries `#[thread_aware(skip)]`, which is what makes the
    /// `Self: Send` predicate necessary.
    has_skipped_field: bool,
}

#[cfg_attr(coverage_nightly, coverage(off))] // can't figure out how to get to 100% coverage of this function
fn collect_generics_in_fields(fields: &Fields, generics: &syn::Generics, usage: &mut GenericUsage) -> syn::Result<()> {
    let generic_idents: HashSet<_> = generics
        .params
        .iter()
        .filter_map(|gp| match gp {
            syn::GenericParam::Type(t) => Some(t.ident.clone()),
            _ => None,
        })
        .collect();
    for field in fields {
        // Mirror exactly what the body generators skip. A skipped field is absent from the
        // generated body, so it needs no `ThreadAware` bound; the `Self: Send` predicate
        // covers it instead. Keeping this test identical to the one in `struct_gen`/`enum_gen`
        // is what stops the header and the body disagreeing about which fields are relocated.
        if parse_field_attrs(&field.attrs)?.skip {
            usage.has_skipped_field = true;
            continue;
        }
        collect_generics_in_type(&field.ty, &generic_idents, usage)?;
    }
    Ok(())
}

#[cfg_attr(coverage_nightly, coverage(off))] // can't figure out how to get to 100% coverage of this function
fn collect_generics_in_type(ty: &Type, generic_idents: &HashSet<syn::Ident>, acc: &mut GenericUsage) -> syn::Result<()> {
    match ty {
        Type::Path(TypePath { path, .. }) => {
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
        // Not traversed: `Type::Slice`, `Type::Ptr`, `Type::BareFn`, `Type::TraitObject` and
        // `Type::ImplTrait`. A bare `fn` pointer has `ThreadAware` impls in `impls.rs`, but
        // they are unconditional - no bound on the argument or return types - so descending
        // would emit a bound nothing requires. The rest have no impl, so an enclosing field
        // cannot be relocated through one and no bound is owed.
        //
        // This list is not a mirror of `impls.rs` and should not be read as one: `Array` and
        // `Reference` are traversed here although `impls.rs` implements neither, so those emit
        // a bound for a field that cannot be relocated at all. The maintenance rule runs one
        // way only - adding a *conditional* impl in `impls.rs` for any shape listed above
        // means adding the matching arm here, or the header will under-constrain the body.
        _ => {}
    }
    Ok(())
}
