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

    // Type parameters in declaration order (for deterministic output) and as a set (for fast
    // membership tests).
    let generic_param_idents: Vec<syn::Ident> = generics
        .params
        .iter()
        .filter_map(|gp| match gp {
            GenericParam::Type(t) => Some(t.ident.clone()),
            _ => None,
        })
        .collect();
    let generic_idents: HashSet<syn::Ident> = generic_param_idents.iter().cloned().collect();

    // The idents that make a field type self-referential: the type being derived, and `Self`.
    let self_idents: HashSet<syn::Ident> = [input.ident.clone(), syn::Ident::new("Self", proc_macro2::Span::call_site())]
        .into_iter()
        .collect();

    let mut thread_aware_path = root_path.clone();
    thread_aware_path.segments.push(parse_quote!(ThreadAware));

    // Gather the relocated (non-skipped) field types in declaration order, and note whether any
    // field is skipped.
    let mut relocated_fields: Vec<Type> = Vec::new();
    let mut has_skipped_field = false;
    collect_relocated_fields(&input.data, &mut relocated_fields, &mut has_skipped_field)?;

    // The generated body relocates each field by calling `<field type>::relocate`, so the impl
    // owes `<field type>: ThreadAware` for every field whose relocation depends on a generic
    // parameter. Bounding the field type - rather than the parameters inside it - lets a type
    // with an unconditional impl, such as a wrapper that ignores its parameter, satisfy the
    // predicate for arguments no per-parameter bound could admit. A field type that reaches no
    // parameter is `ThreadAware` (or not) at the definition site and needs no predicate.
    let mut emitted_keys: Vec<String> = Vec::new();

    // Seed with the field-type predicates the author already wrote in a `where` clause, so a
    // generated one that duplicates it is suppressed - a redundant predicate trips
    // `clippy::trait_duplication_in_bounds` at the author's own declaration.
    if let Some(where_clause) = &input.generics.where_clause {
        for predicate in &where_clause.predicates {
            if let syn::WherePredicate::Type(pt) = predicate
                && pt
                    .bounds
                    .iter()
                    .any(|b| matches!(b, syn::TypeParamBound::Trait(t) if is_same_trait(&t.path, &thread_aware_path)))
            {
                emitted_keys.push(strip_group_paren(&pt.bounded_ty).to_token_stream().to_string());
            }
        }
    }

    let mut predicates: Vec<syn::WherePredicate> = Vec::new();
    for field_ty in &relocated_fields {
        if !type_reaches_ident(field_ty, &generic_idents) {
            continue;
        }

        // Choose what to bound. A field whose type names the type being derived (or `Self`) cannot
        // be bounded by its own type: `where <field type>: ThreadAware` would be a bound on the impl
        // under construction and the trait solver overflows on it. Bound the parameters it reaches
        // instead - those bottom out at the concrete argument, proving the recursive impl
        // inductively. Every other field is bounded by its own type.
        let targets: Vec<Type> = if type_reaches_ident(field_ty, &self_idents) {
            generic_param_idents
                .iter()
                .filter(|&param| {
                    let single: HashSet<syn::Ident> = std::iter::once(param.clone()).collect();
                    type_reaches_ident(field_ty, &single)
                })
                .map(|param| parse_quote!(#param))
                .collect()
        } else {
            vec![strip_group_paren(field_ty).clone()]
        };

        for target in targets {
            // Two fields owing the same predicate contribute it once; repeating it trips
            // `clippy::trait_duplication_in_bounds`.
            let key = target.to_token_stream().to_string();
            if emitted_keys.iter().any(|seen| seen == &key) {
                continue;
            }
            emitted_keys.push(key);

            // A target that is exactly a parameter the author already bounded by `ThreadAware` needs
            // no generated predicate: emitting one would duplicate the author's own bound and trip
            // `clippy::trait_duplication_in_bounds` at their declaration.
            if let Some(param) = as_bare_param(&target, &generic_idents)
                && param_has_thread_aware_bound(&generics, param, &thread_aware_path)
            {
                continue;
            }

            predicates.push(parse_quote!(#target: #thread_aware_path));
        }
    }

    if !predicates.is_empty() {
        let where_clause = generics.make_where_clause();
        for predicate in predicates {
            where_clause.predicates.push(predicate);
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
    if has_skipped_field {
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

/// Collects the relocated (non-skipped) field types of a struct or enum, in declaration order,
/// and records whether any field carries `#[thread_aware(skip)]`.
///
/// Mirrors exactly what the body generators relocate: a skipped field is absent from the
/// generated body, so it owes no `ThreadAware` predicate - only the `Self: Send` one. Keeping
/// this in step with `struct_gen`/`enum_gen` is what stops the header and the body disagreeing
/// about which fields are relocated.
fn collect_relocated_fields(data: &Data, out: &mut Vec<Type>, has_skipped_field: &mut bool) -> syn::Result<()> {
    match data {
        Data::Struct(s) => collect_relocated_from_fields(&s.fields, out, has_skipped_field)?,
        Data::Enum(e) => {
            for v in &e.variants {
                collect_relocated_from_fields(&v.fields, out, has_skipped_field)?;
            }
        }
        Data::Union(_) => {}
    }
    Ok(())
}

fn collect_relocated_from_fields(fields: &Fields, out: &mut Vec<Type>, has_skipped_field: &mut bool) -> syn::Result<()> {
    for field in fields {
        if parse_field_attrs(&field.attrs)?.skip {
            *has_skipped_field = true;
            continue;
        }
        out.push(field.ty.clone());
    }
    Ok(())
}

/// Strips any outer `Type::Group` and `Type::Paren` layers.
///
/// A `Type::Group` is the invisible wrapper macro expansion leaves around a captured type; a
/// `Type::Paren` is an explicit `(T)`. Neither changes the type, so the emitted predicate reads
/// more naturally written on what they wrap.
fn strip_group_paren(ty: &Type) -> &Type {
    let mut current = ty;
    loop {
        match current {
            Type::Group(g) => current = &g.elem,
            Type::Paren(p) => current = &p.elem,
            other => return other,
        }
    }
}

/// Returns the parameter a field type names directly, when the field type is exactly one of the
/// generic parameters. The caller has already stripped any `Group`/`Paren` layers.
fn as_bare_param<'a>(ty: &'a Type, generic_idents: &HashSet<syn::Ident>) -> Option<&'a syn::Ident> {
    if let Type::Path(TypePath { qself: None, path, .. }) = ty
        && let Some(ident) = path.get_ident()
        && generic_idents.contains(ident)
    {
        return Some(ident);
    }
    None
}

/// Reports whether the parameter's own declaration already carries an inline `ThreadAware` bound.
///
/// Only the inline bounds on the parameter are inspected. An equivalent predicate the author wrote
/// in a `where` clause is suppressed separately, by seeding the emitted-predicate set from that
/// `where` clause in `add_bounds`.
fn param_has_thread_aware_bound(generics: &syn::Generics, ident: &syn::Ident, thread_aware_path: &Path) -> bool {
    generics.params.iter().any(|param| {
        matches!(param, GenericParam::Type(ty_param)
            if &ty_param.ident == ident
                && ty_param
                    .bounds
                    .iter()
                    .any(|b| matches!(b, syn::TypeParamBound::Trait(t) if is_same_trait(&t.path, thread_aware_path))))
    })
}

/// Reports whether `ty` reaches one of `targets` through a shape the generated body relocates
/// through: a path's type arguments, a reference, a tuple, an array, a slice, or a `Group`/`Paren`
/// wrapper.
///
/// Used two ways: with the generic parameters, to decide whether a field owes a bound at all; and
/// with the type being derived (plus `Self`), to decide whether that bound would be self-referential
/// and must fall back to the reached parameters. `Slice` is traversed because `thread_aware_core`
/// implements `ThreadAware` for `[T]` conditionally on `T`, so a `[T]` (or `Box<[T]>`) field's
/// obligation does reduce to one on the parameter. The shapes left out - `Ptr`, `BareFn`,
/// `TraitObject`, `ImplTrait` - cannot: a safe `fn` pointer implements `ThreadAware` unconditionally,
/// so a parameter carried only for variance inside one (`PhantomData<fn(*const T)>`) owes no bound,
/// and the rest have no impl at all. This keeps the marker-payload idiom bound-free.
#[cfg_attr(coverage_nightly, coverage(off))] // can't figure out how to get to 100% coverage of this function
fn type_reaches_ident(ty: &Type, targets: &HashSet<syn::Ident>) -> bool {
    match ty {
        Type::Path(TypePath { path, .. }) => {
            for segment in &path.segments {
                if targets.contains(&segment.ident) {
                    return true;
                }
                if let PathArguments::AngleBracketed(ab) = &segment.arguments {
                    for arg in &ab.args {
                        if let syn::GenericArgument::Type(t) = arg
                            && type_reaches_ident(t, targets)
                        {
                            return true;
                        }
                    }
                }
            }
            false
        }
        Type::Reference(r) => type_reaches_ident(&r.elem, targets),
        Type::Tuple(t) => t.elems.iter().any(|elem| type_reaches_ident(elem, targets)),
        Type::Array(a) => type_reaches_ident(&a.elem, targets),
        Type::Slice(s) => type_reaches_ident(&s.elem, targets),
        Type::Group(g) => type_reaches_ident(&g.elem, targets),
        Type::Paren(p) => type_reaches_ident(&p.elem, targets),
        _ => false,
    }
}
