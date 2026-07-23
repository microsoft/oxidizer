// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![expect(
    clippy::needless_pass_by_value,
    reason = "token fragments are cheap values consumed by generated quote expressions"
)]
#![expect(
    clippy::too_many_arguments,
    reason = "generation helpers explicitly carry the complete derive context"
)]
#![expect(
    clippy::too_many_lines,
    reason = "the enum generator handles all externally tagged variant shapes together"
)]

//! Implementation of the `multitude` arena-aware deserialization derive.
//!
//! Arena-specific derive configuration is parsed from `#[multitude(...)]`;
//! Serde's own configuration remains under `#[serde(...)]`.

use std::collections::HashSet;

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{ToTokens, format_ident, quote};
use syn::{Data, DataEnum, DeriveInput, Field, Fields, GenericParam, Generics, Ident, Lifetime, LifetimeParam, Path, Variant, parse_quote};

mod attrs;

use attrs::{ContainerAttrs, DefaultValue, FieldAttrs, RenameRule, parse_container, parse_field, parse_variant};

/// Generates an implementation of `DeserializeIn` using `root_path`.
#[must_use]
pub fn derive_deserialize_in(input: TokenStream2, root_path: &Path) -> TokenStream2 {
    syn::parse2::<DeriveInput>(input)
        .and_then(|input| expand(&input, root_path))
        .unwrap_or_else(syn::Error::into_compile_error)
}

#[derive(Clone)]
struct FieldInfo<'a> {
    field: &'a Field,
    attrs: FieldAttrs,
    wire_name: String,
}

struct InternalNames {
    allocator: Ident,
    serde: Ident,
}

fn collect_identifiers(tokens: TokenStream2, identifiers: &mut HashSet<String>) {
    for token in tokens {
        match token {
            proc_macro2::TokenTree::Ident(ident) => {
                identifiers.insert(ident.to_string());
            }
            proc_macro2::TokenTree::Group(group) => collect_identifiers(group.stream(), identifiers),
            proc_macro2::TokenTree::Punct(_) | proc_macro2::TokenTree::Literal(_) => {}
        }
    }
}

fn fresh_internal_ident(identifiers: &HashSet<String>, base: &str) -> Ident {
    let mut candidate = base.to_owned();
    while identifiers.contains(&candidate) {
        candidate.push('_');
    }
    Ident::new(&candidate, Span::mixed_site())
}

fn internal_names(input: &DeriveInput) -> InternalNames {
    let mut identifiers = HashSet::new();
    collect_identifiers(input.to_token_stream(), &mut identifiers);
    InternalNames {
        allocator: fresh_internal_ident(&identifiers, "__A"),
        serde: fresh_internal_ident(&identifiers, "__Serde"),
    }
}

fn fields_info(fields: &Fields, rename_all: Option<RenameRule>) -> syn::Result<Vec<FieldInfo<'_>>> {
    fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let attrs = parse_field(&field.attrs)?;
            let natural = field.ident.as_ref().map_or_else(
                || index.to_string(),
                |ident| rename_all.map_or_else(|| ident.to_string(), |rule| rule.field(&ident.to_string())),
            );
            let wire_name = attrs.rename.clone().unwrap_or(natural);
            Ok(FieldInfo { field, attrs, wire_name })
        })
        .collect()
}

fn accepted_field_names<'a>(infos: &'a [FieldInfo<'_>]) -> Vec<&'a str> {
    infos
        .iter()
        .filter(|info| !info.attrs.skip)
        .flat_map(|info| std::iter::once(info.wire_name.as_str()).chain(info.attrs.aliases.iter().map(String::as_str)))
        .collect()
}

fn expand(input: &DeriveInput, default_root: &Path) -> syn::Result<TokenStream2> {
    const RESERVED_GENERICS: &[&str] = &["__D", "__M", "__S", "__E", "__FieldVisitor", "__VariantVisitor"];
    if let Some(ident) = input.generics.params.iter().find_map(|param| match param {
        GenericParam::Type(param)
            if RESERVED_GENERICS.iter().any(|reserved| param.ident == *reserved) || param.ident.to_string().starts_with("__Multitude") =>
        {
            Some(&param.ident)
        }
        GenericParam::Const(param)
            if RESERVED_GENERICS.iter().any(|reserved| param.ident == *reserved) || param.ident.to_string().starts_with("__Multitude") =>
        {
            Some(&param.ident)
        }
        _ => None,
    }) {
        return Err(syn::Error::new_spanned(
            ident,
            format!("`DeserializeIn` reserves the internal generic parameter name `{ident}`"),
        ));
    }
    if let Some(lifetime) = input.generics.lifetimes().find(|param| param.lifetime.ident == "de") {
        return Err(syn::Error::new_spanned(
            &lifetime.lifetime,
            "`DeserializeIn` reserves the deserializer lifetime name `'de`",
        ));
    }
    let container = parse_container(&input.attrs)?;
    let names = internal_names(input);
    let custom_root = container.multitude_crate.as_ref().map(|multitude_crate| {
        let mut root = multitude_crate.clone();
        root.segments.push(parse_quote!(de));
        root
    });
    let root = custom_root.as_ref().unwrap_or(default_root);
    let all_fields = match &input.data {
        Data::Struct(data) => fields_info(&data.fields, container.rename_all)?,
        Data::Enum(data) => {
            let mut fields = Vec::new();
            for variant in &data.variants {
                let attrs = parse_variant(&variant.attrs)?;
                if !attrs.skip && attrs.serde_with.is_none() && attrs.multitude_with.is_none() {
                    fields.extend(fields_info(&variant.fields, attrs.rename_all.or(container.rename_all_fields))?);
                }
            }
            fields
        }
        Data::Union(_) => Vec::new(),
    };
    if container.default.is_some() && !matches!(&input.data, Data::Struct(data) if matches!(data.fields, Fields::Named(_))) {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "serde container `default` is supported only for named structs",
        ));
    }
    if container.transparent {
        let Data::Struct(data) = &input.data else {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "serde `transparent` is supported only for structs",
            ));
        };
        let fields = fields_info(&data.fields, container.rename_all)?;
        if fields.iter().filter(|field| !field.attrs.skip).count() != 1 {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "serde `transparent` requires exactly one non-skipped field",
            ));
        }
        if container.default.is_some() {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "serde container `default` cannot be combined with `transparent`",
            ));
        }
    }

    let de = fresh_lifetime(input, "de");
    let arena = fresh_lifetime(input, "__arena");
    let bounded = bounded_generics(input, &all_fields, &container, &de, root, &names);
    let helper_base = helper_generics(&input.generics, &arena, root, &names);
    let name = &input.ident;
    let visitor = format_ident!("__MultitudeVisitorFor{name}");
    let body = match &input.data {
        Data::Struct(data) => struct_tokens(
            input,
            &data.fields,
            &container,
            root,
            &de,
            &arena,
            &bounded,
            &helper_base,
            &visitor,
            &names,
        )?,
        Data::Enum(data) => enum_tokens(input, data, &container, root, &de, &arena, &bounded, &helper_base, &visitor, &names)?,
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "#[derive(DeserializeIn)] does not support unions",
            ));
        }
    };

    Ok(quote! {
        const _: () = {
            #body
        };
    })
}

fn fresh_lifetime(input: &DeriveInput, base: &str) -> Lifetime {
    let used: HashSet<_> = input.generics.lifetimes().map(|param| param.lifetime.ident.to_string()).collect();
    let mut name = base.to_owned();
    while used.contains(name.trim_start_matches('\'')) {
        name.push('_');
    }
    Lifetime::new(&format!("'{name}"), Span::call_site())
}

fn without_defaults(mut generics: Generics) -> Generics {
    for param in &mut generics.params {
        match param {
            GenericParam::Type(param) => param.default = None,
            GenericParam::Const(param) => param.default = None,
            GenericParam::Lifetime(_) => {}
        }
    }
    generics
}

fn allocator_predicate(root: &Path, names: &InternalNames) -> syn::WherePredicate {
    let allocator = &names.allocator;
    parse_quote!(#allocator: #root::__private::allocator_api2::alloc::Allocator + ::core::clone::Clone)
}

fn arena_path(root: &Path) -> Path {
    let mut path = root.clone();
    path.segments.pop();
    path.segments.push(parse_quote!(Arena));
    path
}

fn bounded_generics(
    input: &DeriveInput,
    fields: &[FieldInfo<'_>],
    container: &ContainerAttrs,
    de: &Lifetime,
    root: &Path,
    names: &InternalNames,
) -> Generics {
    let allocator = &names.allocator;
    let mut generics = without_defaults(input.generics.clone());
    generics.params.insert(0, GenericParam::Lifetime(LifetimeParam::new(de.clone())));
    generics.params.push(parse_quote!(#allocator));
    let where_clause = generics.make_where_clause();
    where_clause.predicates.push(allocator_predicate(root, names));
    if matches!(container.default.as_ref(), Some(DefaultValue::Trait)) {
        let target = original_type(input);
        where_clause.predicates.push(parse_quote!(#target: ::core::default::Default));
    }
    let mut seen = HashSet::new();
    for info in fields {
        let ty = &info.field.ty;
        if matches!(info.attrs.default, Some(DefaultValue::Trait))
            || (info.attrs.skip && info.attrs.default.is_none() && container.default.is_none())
        {
            let key = ty.to_token_stream().to_string();
            if seen.insert((key, "default")) {
                where_clause.predicates.push(parse_quote!(#ty: ::core::default::Default));
            }
        }
        if info.attrs.skip || container.deserialize_bounds.is_some() {
            continue;
        }
        let key = ty.to_token_stream().to_string();
        let flavor = if info.attrs.via_serde {
            "serde"
        } else if info.attrs.serde_with.is_some() || info.attrs.multitude_with.is_some() {
            "custom"
        } else {
            "arena"
        };
        if seen.insert((key, flavor)) {
            if info.attrs.via_serde {
                where_clause
                    .predicates
                    .push(parse_quote!(#ty: #root::__private::serde::Deserialize<#de>));
            } else if info.attrs.serde_with.is_none() && info.attrs.multitude_with.is_none() {
                where_clause
                    .predicates
                    .push(parse_quote!(#ty: #root::DeserializeIn<#de, #allocator>));
            }
        }
    }
    if let Some(predicates) = &container.deserialize_bounds {
        where_clause.predicates.extend(predicates.iter().cloned());
    }
    generics
}

fn helper_generics(original: &Generics, arena: &Lifetime, root: &Path, names: &InternalNames) -> Generics {
    let allocator = &names.allocator;
    let mut generics = without_defaults(original.clone());
    generics.params.insert(0, GenericParam::Lifetime(LifetimeParam::new(arena.clone())));
    generics.params.push(parse_quote!(#allocator));
    generics.make_where_clause().predicates.push(allocator_predicate(root, names));
    generics
}

fn helper_impl_generics(bounded: &Generics, arena: &Lifetime) -> Generics {
    let mut generics = bounded.clone();
    generics.params.insert(1, GenericParam::Lifetime(LifetimeParam::new(arena.clone())));
    generics
}

fn original_type(input: &DeriveInput) -> TokenStream2 {
    let name = &input.ident;
    let (_, ty_generics, _) = input.generics.split_for_impl();
    quote!(#name #ty_generics)
}

fn visitor_type_args(input: &DeriveInput, arena: &Lifetime, names: &InternalNames) -> TokenStream2 {
    let allocator = &names.allocator;
    let args = input.generics.params.iter().map(|param| match param {
        GenericParam::Lifetime(param) => {
            let lifetime = &param.lifetime;
            quote!(#lifetime)
        }
        GenericParam::Type(param) => {
            let ident = &param.ident;
            quote!(#ident)
        }
        GenericParam::Const(param) => {
            let ident = &param.ident;
            quote!(#ident)
        }
    });
    quote!(<#arena, #(#args,)* #allocator>)
}

fn custom_seed_name(owner: &Ident, index: usize) -> Ident {
    format_ident!("{owner}Field{index}Seed")
}

fn seed(info: &FieldInfo<'_>, root: &Path, owner: &Ident, index: usize, names: &InternalNames) -> TokenStream2 {
    let allocator = &names.allocator;
    let ty = &info.field.ty;
    if info.attrs.serde_with.is_some() || info.attrs.multitude_with.is_some() {
        let seed = custom_seed_name(owner, index);
        quote!(#seed { arena: self.arena, marker: ::core::marker::PhantomData })
    } else if info.attrs.via_serde {
        quote!(#root::DeserializeSeed::<#ty>::new())
    } else {
        quote!(#root::DeserializeInSeed::<#ty, #allocator>::new(self.arena))
    }
}

fn custom_seed_definition(
    input: &DeriveInput,
    info: &FieldInfo<'_>,
    index: usize,
    root: &Path,
    de: &Lifetime,
    arena: &Lifetime,
    bounded: &Generics,
    helper: &Generics,
    owner: &Ident,
    names: &InternalNames,
) -> Option<TokenStream2> {
    let allocator = &names.allocator;
    let serde = &names.serde;
    let path = info.attrs.serde_with.as_ref().or(info.attrs.multitude_with.as_ref())?;
    let name = custom_seed_name(owner, index);
    let ty = &info.field.ty;
    let target = original_type(input);
    let arena_path = arena_path(root);
    let (helper_params, _, helper_where) = helper.split_for_impl();
    let helper_impl = helper_impl_generics(bounded, arena);
    let (impl_params, _, where_clause) = helper_impl.split_for_impl();
    let args = visitor_type_args(input, arena, names);
    let call = if info.attrs.multitude_with.is_some() {
        quote!(#path(self.arena, deserializer))
    } else {
        quote!(#path(deserializer))
    };
    Some(quote! {
        struct #name #helper_params #helper_where {
            arena: &#arena #arena_path<#allocator>,
            marker: ::core::marker::PhantomData<fn() -> (#ty, #target)>,
        }

        impl #impl_params #serde::de::DeserializeSeed<#de> for #name #args #where_clause {
            type Value = #ty;

            fn deserialize<__D>(self, deserializer: __D) -> ::core::result::Result<Self::Value, __D::Error>
            where
                __D: #serde::Deserializer<#de>,
            {
                #call
            }
        }
    })
}

fn custom_variant_seed_definition(
    input: &DeriveInput,
    info: &VariantInfo<'_>,
    index: usize,
    root: &Path,
    de: &Lifetime,
    arena: &Lifetime,
    bounded: &Generics,
    helper: &Generics,
    owner: &Ident,
    names: &InternalNames,
) -> Option<(TokenStream2, TokenStream2)> {
    let allocator = &names.allocator;
    let serde = &names.serde;
    let path = info.attrs.serde_with.as_ref().or(info.attrs.multitude_with.as_ref())?;
    let name = format_ident!("{owner}Variant{index}Seed");
    let field_types: Vec<_> = info.variant.fields.iter().map(|field| &field.ty).collect();
    let value_type = if field_types.len() == 1 {
        let ty = field_types[0];
        quote!(#ty)
    } else {
        quote!((#(#field_types),*))
    };
    let target = original_type(input);
    let arena_path = arena_path(root);
    let (helper_params, _, helper_where) = helper.split_for_impl();
    let helper_impl = helper_impl_generics(bounded, arena);
    let (impl_params, _, where_clause) = helper_impl.split_for_impl();
    let args = visitor_type_args(input, arena, names);
    let call = if info.attrs.multitude_with.is_some() {
        quote!(#path(self.arena, deserializer))
    } else {
        quote!(#path(deserializer))
    };
    let definition = quote! {
        struct #name #helper_params #helper_where {
            arena: &#arena #arena_path<#allocator>,
            marker: ::core::marker::PhantomData<fn() -> (#value_type, #target)>,
        }

        impl #impl_params #serde::de::DeserializeSeed<#de> for #name #args #where_clause {
            type Value = #value_type;

            fn deserialize<__D>(self, deserializer: __D) -> ::core::result::Result<Self::Value, __D::Error>
            where
                __D: #serde::Deserializer<#de>,
            {
                #call
            }
        }
    };
    let seed = quote!(#name { arena: self.arena, marker: ::core::marker::PhantomData });
    Some((definition, seed))
}

fn construct_custom_variant(enum_name: &Ident, variant: &Ident, fields: &Fields) -> TokenStream2 {
    match fields {
        Fields::Unit => quote! {
            let _: () = __value;
            #enum_name::#variant
        },
        Fields::Unnamed(fields) if fields.unnamed.is_empty() => quote! {
            let _: () = __value;
            #enum_name::#variant()
        },
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => quote!(#enum_name::#variant(__value)),
        Fields::Unnamed(fields) => {
            let indexes = (0..fields.unnamed.len()).map(syn::Index::from);
            quote!(#enum_name::#variant(#(__value.#indexes),*))
        }
        Fields::Named(fields) if fields.named.is_empty() => quote! {
            let _: () = __value;
            #enum_name::#variant {}
        },
        Fields::Named(fields) if fields.named.len() == 1 => {
            let field = fields
                .named
                .first()
                .and_then(|field| field.ident.as_ref())
                .expect("guarded by the one-named-field match arm");
            quote!(#enum_name::#variant { #field: __value })
        }
        Fields::Named(fields) => {
            let field_names = fields.named.iter().map(|field| field.ident.as_ref().expect("named field"));
            let indexes = (0..fields.named.len()).map(syn::Index::from);
            quote!(#enum_name::#variant { #(#field_names: __value.#indexes),* })
        }
    }
}

fn default_expr(info: &FieldInfo<'_>, error: TokenStream2, names: &InternalNames) -> TokenStream2 {
    let serde = &names.serde;
    match &info.attrs.default {
        Some(DefaultValue::Trait) => quote!(::core::default::Default::default()),
        Some(DefaultValue::Path(path)) => quote!(#path()),
        None if info.attrs.skip => quote!(::core::default::Default::default()),
        None => {
            let name = &info.wire_name;
            quote!(return ::core::result::Result::Err(<#error as #serde::de::Error>::missing_field(#name)))
        }
    }
}

fn visitor_definition(
    input: &DeriveInput,
    root: &Path,
    arena: &Lifetime,
    helper: &Generics,
    visitor: &Ident,
    names: &InternalNames,
) -> TokenStream2 {
    let allocator = &names.allocator;
    let target = original_type(input);
    let arena_path = arena_path(root);
    let (helper_params, _, helper_where) = helper.split_for_impl();
    quote! {
        struct #visitor #helper_params #helper_where {
            arena: &#arena #arena_path<#allocator>,
            marker: ::core::marker::PhantomData<fn() -> #target>,
        }
    }
}

fn field_enum(name: &Ident, infos: &[FieldInfo<'_>], deny_unknown: bool, names: &InternalNames) -> syn::Result<TokenStream2> {
    let serde = &names.serde;
    let fields_const = format_ident!("{name}_FIELDS");
    let variants: Vec<_> = (0..infos.len()).map(|index| format_ident!("Field{index}")).collect();
    let declared_variants: Vec<_> = infos
        .iter()
        .enumerate()
        .filter(|(_, info)| !info.attrs.skip)
        .map(|(index, _)| &variants[index])
        .collect();
    let mut seen = HashSet::new();
    let mut string_arms = Vec::new();
    let mut byte_arms = Vec::new();
    let expected = accepted_field_names(infos);
    for (index, info) in infos.iter().enumerate() {
        if info.attrs.skip && deny_unknown {
            continue;
        }
        let target = if info.attrs.skip {
            quote!(#name::Ignore)
        } else {
            let variant = &variants[index];
            quote!(#name::#variant)
        };
        for wire_name in std::iter::once(&info.wire_name).chain(&info.attrs.aliases) {
            if !seen.insert(wire_name.clone()) {
                return Err(syn::Error::new_spanned(
                    info.field,
                    format!("duplicate deserialization field name or alias `{wire_name}`"),
                ));
            }
            let bytes = syn::LitByteStr::new(wire_name.as_bytes(), Span::call_site());
            string_arms.push(quote!(#wire_name => ::core::result::Result::Ok(#target)));
            byte_arms.push(quote!(#bytes => ::core::result::Result::Ok(#target)));
        }
    }
    let unknown_str = if deny_unknown {
        quote!(::core::result::Result::Err(__E::unknown_field(__value, #fields_const)))
    } else {
        quote!(::core::result::Result::Ok(#name::Ignore))
    };
    let unknown_bytes = if deny_unknown {
        quote! {
            match ::core::str::from_utf8(__value) {
                ::core::result::Result::Ok(__value) => ::core::result::Result::Err(__E::unknown_field(__value, #fields_const)),
                ::core::result::Result::Err(_) => ::core::result::Result::Err(__E::invalid_value(
                    #serde::de::Unexpected::Bytes(__value),
                    &self,
                )),
            }
        }
    } else {
        quote!(::core::result::Result::Ok(#name::Ignore))
    };
    let ordinal_arms = infos
        .iter()
        .enumerate()
        .filter(|(_, info)| !info.attrs.skip)
        .enumerate()
        .map(|(ordinal, (index, _))| {
            let ordinal = ordinal as u64;
            let variant = &variants[index];
            quote!(#ordinal => ::core::result::Result::Ok(#name::#variant))
        });
    let ordinal_expecting = format!("field index 0 <= i < {}", infos.iter().filter(|info| !info.attrs.skip).count());
    let unknown_ordinal = if deny_unknown {
        quote! {
            ::core::result::Result::Err(__E::invalid_value(
                #serde::de::Unexpected::Unsigned(__value),
                &#ordinal_expecting,
            ))
        }
    } else {
        quote!(::core::result::Result::Ok(#name::Ignore))
    };
    Ok(quote! {
        enum #name {
            #(#declared_variants,)*
            Ignore,
        }

        impl<'__field_de> #serde::Deserialize<'__field_de> for #name {
            fn deserialize<__D>(deserializer: __D) -> ::core::result::Result<Self, __D::Error>
            where
                __D: #serde::Deserializer<'__field_de>,
            {
                struct __FieldVisitor;
                impl<'__field_de> #serde::de::Visitor<'__field_de> for __FieldVisitor {
                    type Value = #name;

                    fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                        formatter.write_str("a field name")
                    }

                    fn visit_str<__E>(self, __value: &str) -> ::core::result::Result<Self::Value, __E>
                    where
                        __E: #serde::de::Error,
                    {
                        match __value {
                            #(#string_arms,)*
                            _ => #unknown_str,
                        }
                    }

                    fn visit_bytes<__E>(self, __value: &[u8]) -> ::core::result::Result<Self::Value, __E>
                    where
                        __E: #serde::de::Error,
                    {
                        match __value {
                            #(#byte_arms,)*
                            _ => #unknown_bytes,
                        }
                    }

                    fn visit_u64<__E>(self, __value: u64) -> ::core::result::Result<Self::Value, __E>
                    where
                        __E: #serde::de::Error,
                    {
                        match __value {
                            #(#ordinal_arms,)*
                            _ => #unknown_ordinal,
                        }
                    }
                }
                #serde::Deserializer::deserialize_identifier(deserializer, __FieldVisitor)
            }
        }
        const #fields_const: &[&str] = &[#(#expected),*];
    })
}

fn named_visitor(
    input: &DeriveInput,
    infos: &[FieldInfo<'_>],
    constructor: TokenStream2,
    container: &ContainerAttrs,
    root: &Path,
    de: &Lifetime,
    arena: &Lifetime,
    bounded: &Generics,
    helper: &Generics,
    visitor: &Ident,
    field_name: &Ident,
    names: &InternalNames,
) -> syn::Result<TokenStream2> {
    let serde = &names.serde;
    let definition = visitor_definition(input, root, arena, helper, visitor, names);
    let custom_seeds = infos
        .iter()
        .enumerate()
        .filter_map(|(index, info)| custom_seed_definition(input, info, index, root, de, arena, bounded, helper, visitor, names));
    let field_definition = field_enum(field_name, infos, container.deny_unknown_fields, names)?;
    let helper_impl = helper_impl_generics(bounded, arena);
    let (impl_params, _, where_clause) = helper_impl.split_for_impl();
    let visitor_args = visitor_type_args(input, arena, names);
    let target = original_type(input);
    let declarations = infos.iter().enumerate().filter(|(_, info)| !info.attrs.skip).map(|(index, _)| {
        let binding = format_ident!("__field{index}");
        quote!(let mut #binding = ::core::option::Option::None;)
    });
    let map_arms = infos.iter().enumerate().filter(|(_, info)| !info.attrs.skip).map(|(index, info)| {
        let variant = format_ident!("Field{index}");
        let binding = format_ident!("__field{index}");
        let value_seed = seed(info, root, visitor, index, names);
        let wire_name = &info.wire_name;
        quote! {
            #field_name::#variant => {
                if #binding.is_some() {
                    return ::core::result::Result::Err(<__M::Error as #serde::de::Error>::duplicate_field(#wire_name));
                }
                #binding = ::core::option::Option::Some(
                    #serde::de::MapAccess::next_value_seed(&mut __map, #value_seed)?
                );
            }
        }
    });
    let container_default_fields: Vec<_> = infos
        .iter()
        .enumerate()
        .filter(|(_, info)| info.attrs.default.is_none() && container.default.is_some())
        .collect();
    let default_setup = if let Some(container_default) = &container.default {
        let container_default = match container_default {
            DefaultValue::Trait => quote!(::core::default::Default::default()),
            DefaultValue::Path(path) => quote!(#path()),
        };
        if container_default_fields.is_empty() {
            quote!(let _: #target = #container_default;)
        } else {
            let field_patterns = container_default_fields.iter().map(|(index, info)| {
                let field = info.field.ident.as_ref().expect("named field");
                let default = format_ident!("__container_default{index}");
                quote!(#field: #default)
            });
            let some_values = container_default_fields.iter().map(|(index, _)| {
                let default = format_ident!("__container_default{index}");
                quote!(::core::option::Option::Some(#default))
            });
            let option_bindings = container_default_fields.iter().map(|(index, _)| {
                let default = format_ident!("__container_default{index}");
                quote!(#default)
            });
            quote! {
                let (#(#option_bindings,)*) = {
                    let #constructor { #(#field_patterns,)* .. } = #container_default;
                    (#(#some_values,)*)
                };
            }
        }
    } else {
        quote!()
    };
    let map_values = infos.iter().enumerate().map(|(index, info)| {
        if container.default.is_some() && info.attrs.default.is_none() {
            let binding = format_ident!("__field{index}");
            let default = format_ident!("__container_default{index}");
            if info.attrs.skip {
                quote! {
                    match #default {
                        ::core::option::Option::Some(__value) => __value,
                        ::core::option::Option::None => ::core::unreachable!(),
                    }
                }
            } else {
                quote! {
                    match #binding {
                        ::core::option::Option::Some(__value) => __value,
                        ::core::option::Option::None => match #default {
                            ::core::option::Option::Some(__value) => __value,
                            ::core::option::Option::None => ::core::unreachable!(),
                        },
                    }
                }
            }
        } else if info.attrs.skip {
            default_expr(info, quote!(__M::Error), names)
        } else {
            let binding = format_ident!("__field{index}");
            let missing = default_expr(info, quote!(__M::Error), names);
            quote!(match #binding { ::core::option::Option::Some(__value) => __value, ::core::option::Option::None => #missing })
        }
    });
    let mut ordinal = 0usize;
    let seq_bindings = infos.iter().enumerate().map(|(index, info)| {
        let binding = format_ident!("__field{index}");
        if info.attrs.skip {
            let value = if container.default.is_some() && info.attrs.default.is_none() {
                let default = format_ident!("__container_default{index}");
                quote! {
                    match #default {
                        ::core::option::Option::Some(__value) => __value,
                        ::core::option::Option::None => ::core::unreachable!(),
                    }
                }
            } else {
                default_expr(info, quote!(__S::Error), names)
            };
            return quote!(let #binding = #value;);
        }

        let current_ordinal = ordinal;
        ordinal += 1;
        let value_seed = seed(info, root, visitor, index, names);
        let missing = if container.default.is_some() && info.attrs.default.is_none() {
            let default = format_ident!("__container_default{index}");
            quote! {
                match #default {
                    ::core::option::Option::Some(__value) => __value,
                    ::core::option::Option::None => ::core::unreachable!(),
                }
            }
        } else {
            match &info.attrs.default {
                Some(DefaultValue::Trait) => quote!(::core::default::Default::default()),
                Some(DefaultValue::Path(path)) => quote!(#path()),
                None => quote! {
                    return ::core::result::Result::Err(
                        <__S::Error as #serde::de::Error>::invalid_length(#current_ordinal, &self)
                    )
                },
            }
        };
        quote! {
            let #binding = match #serde::de::SeqAccess::next_element_seed(&mut __seq, #value_seed)? {
                    ::core::option::Option::Some(__value) => __value,
                    ::core::option::Option::None => #missing,
            };
        }
    });
    let field_idents = infos.iter().map(|info| info.field.ident.as_ref().expect("named field"));
    let seq_field_idents = field_idents.clone();
    let seq_value_bindings = infos.iter().enumerate().map(|(index, _)| format_ident!("__field{index}"));
    let seq_parameter = if infos.iter().any(|info| !info.attrs.skip) {
        quote!(mut __seq)
    } else {
        quote!(_)
    };
    let expecting = container.expecting.as_deref().unwrap_or("a struct");
    Ok(quote! {
        #field_definition
        #(#custom_seeds)*
        #definition
        impl #impl_params #serde::de::Visitor<#de> for #visitor #visitor_args #where_clause {
            type Value = #target;

            fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(#expecting)
            }

            fn visit_seq<__S>(self, #seq_parameter: __S) -> ::core::result::Result<Self::Value, __S::Error>
            where
                __S: #serde::de::SeqAccess<#de>,
            {
                #default_setup
                #(#seq_bindings)*
                ::core::result::Result::Ok(#constructor { #(#seq_field_idents: #seq_value_bindings),* })
            }

            fn visit_map<__M>(self, mut __map: __M) -> ::core::result::Result<Self::Value, __M::Error>
            where
                __M: #serde::de::MapAccess<#de>,
            {
                #(#declarations)*
                while let ::core::option::Option::Some(__key) = #serde::de::MapAccess::next_key::<#field_name>(&mut __map)? {
                    match __key {
                        #(#map_arms)*
                        #field_name::Ignore => {
                            let _: #serde::de::IgnoredAny = #serde::de::MapAccess::next_value(&mut __map)?;
                        }
                    }
                }
                #default_setup
                ::core::result::Result::Ok(#constructor { #(#field_idents: #map_values),* })
            }
        }
    })
}

fn tuple_visitor(
    input: &DeriveInput,
    infos: &[FieldInfo<'_>],
    constructor: TokenStream2,
    container: &ContainerAttrs,
    root: &Path,
    de: &Lifetime,
    arena: &Lifetime,
    bounded: &Generics,
    helper: &Generics,
    visitor: &Ident,
    names: &InternalNames,
) -> TokenStream2 {
    let serde = &names.serde;
    let definition = visitor_definition(input, root, arena, helper, visitor, names);
    let custom_seeds = infos
        .iter()
        .enumerate()
        .filter_map(|(index, info)| custom_seed_definition(input, info, index, root, de, arena, bounded, helper, visitor, names));
    let helper_impl = helper_impl_generics(bounded, arena);
    let (impl_params, _, where_clause) = helper_impl.split_for_impl();
    let visitor_args = visitor_type_args(input, arena, names);
    let target = original_type(input);
    let expecting = container.expecting.as_deref().unwrap_or("a sequence");
    let reads = infos.iter().enumerate().map(|(index, info)| {
        if info.attrs.skip {
            default_expr(info, quote!(__S::Error), names)
        } else {
            let value_seed = seed(info, root, visitor, index, names);
            let missing = default_expr(info, quote!(__S::Error), names);
            quote! {
                match #serde::de::SeqAccess::next_element_seed(&mut __seq, #value_seed)? {
                    ::core::option::Option::Some(__value) => __value,
                    ::core::option::Option::None => #missing,
                }
            }
        }
    });
    quote! {
        #(#custom_seeds)*
        #definition
        impl #impl_params #serde::de::Visitor<#de> for #visitor #visitor_args #where_clause {
            type Value = #target;

            fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(#expecting)
            }

            fn visit_seq<__S>(self, mut __seq: __S) -> ::core::result::Result<Self::Value, __S::Error>
            where
                __S: #serde::de::SeqAccess<#de>,
            {
                ::core::result::Result::Ok(#constructor(#(#reads),*))
            }
        }
    }
}

fn unit_visitor(
    input: &DeriveInput,
    constructor: TokenStream2,
    container: &ContainerAttrs,
    root: &Path,
    de: &Lifetime,
    arena: &Lifetime,
    bounded: &Generics,
    helper: &Generics,
    visitor: &Ident,
    names: &InternalNames,
) -> TokenStream2 {
    let serde = &names.serde;
    let definition = visitor_definition(input, root, arena, helper, visitor, names);
    let helper_impl = helper_impl_generics(bounded, arena);
    let (impl_params, _, where_clause) = helper_impl.split_for_impl();
    let visitor_args = visitor_type_args(input, arena, names);
    let target = original_type(input);
    let expecting = container.expecting.as_deref().unwrap_or("a unit value");
    quote! {
        #definition
        impl #impl_params #serde::de::Visitor<#de> for #visitor #visitor_args #where_clause {
            type Value = #target;

            fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(#expecting)
            }

            fn visit_unit<__E>(self) -> ::core::result::Result<Self::Value, __E>
            where
                __E: #serde::de::Error,
            {
                ::core::result::Result::Ok(#constructor)
            }
        }
    }
}

fn newtype_visitor(
    input: &DeriveInput,
    info: &FieldInfo<'_>,
    constructor: TokenStream2,
    container: &ContainerAttrs,
    root: &Path,
    de: &Lifetime,
    arena: &Lifetime,
    bounded: &Generics,
    helper: &Generics,
    visitor: &Ident,
    names: &InternalNames,
) -> TokenStream2 {
    let serde = &names.serde;
    let definition = visitor_definition(input, root, arena, helper, visitor, names);
    let custom_seed = custom_seed_definition(input, info, 0, root, de, arena, bounded, helper, visitor, names);
    let helper_impl = helper_impl_generics(bounded, arena);
    let (impl_params, _, where_clause) = helper_impl.split_for_impl();
    let visitor_args = visitor_type_args(input, arena, names);
    let target = original_type(input);
    let value_seed = seed(info, root, visitor, 0, names);
    let expecting = container.expecting.as_deref().unwrap_or("a newtype value");
    quote! {
        #custom_seed
        #definition
        impl #impl_params #serde::de::Visitor<#de> for #visitor #visitor_args #where_clause {
            type Value = #target;

            fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(#expecting)
            }

            fn visit_newtype_struct<__D>(self, deserializer: __D) -> ::core::result::Result<Self::Value, __D::Error>
            where
                __D: #serde::Deserializer<#de>,
            {
                let __value = #serde::de::DeserializeSeed::deserialize(#value_seed, deserializer)?;
                ::core::result::Result::Ok(#constructor(__value))
            }
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "the helper carries the complete derive context")]
fn transparent_visitor(
    input: &DeriveInput,
    infos: &[FieldInfo<'_>],
    active_index: usize,
    has_named_fields: bool,
    container: &ContainerAttrs,
    root: &Path,
    de: &Lifetime,
    arena: &Lifetime,
    bounded: &Generics,
    helper: &Generics,
    visitor: &Ident,
    names: &InternalNames,
) -> TokenStream2 {
    let serde = &names.serde;
    let definition = visitor_definition(input, root, arena, helper, visitor, names);
    let custom_seed = custom_seed_definition(
        input,
        &infos[active_index],
        active_index,
        root,
        de,
        arena,
        bounded,
        helper,
        visitor,
        names,
    );
    let helper_impl = helper_impl_generics(bounded, arena);
    let (impl_params, _, where_clause) = helper_impl.split_for_impl();
    let visitor_args = visitor_type_args(input, arena, names);
    let target = original_type(input);
    let value_seed = seed(&infos[active_index], root, visitor, active_index, names);
    let expecting = container.expecting.as_deref().unwrap_or("a transparent value");
    let name = &input.ident;
    let values: Vec<_> = infos
        .iter()
        .enumerate()
        .map(|(index, info)| {
            if index == active_index {
                quote!(__transparent_value)
            } else {
                default_expr(info, quote!(__D::Error), names)
            }
        })
        .collect();
    let construction = if has_named_fields {
        let fields = infos.iter().map(|info| info.field.ident.as_ref().expect("named field"));
        quote!(#name { #(#fields: #values),* })
    } else {
        quote!(#name(#(#values),*))
    };
    quote! {
        #custom_seed
        #definition
        impl #impl_params #serde::de::Visitor<#de> for #visitor #visitor_args #where_clause {
            type Value = #target;

            fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(#expecting)
            }

            fn visit_newtype_struct<__D>(self, deserializer: __D) -> ::core::result::Result<Self::Value, __D::Error>
            where
                __D: #serde::Deserializer<#de>,
            {
                let __transparent_value = #serde::de::DeserializeSeed::deserialize(#value_seed, deserializer)?;
                ::core::result::Result::Ok(#construction)
            }
        }
    }
}

fn target_impl(
    input: &DeriveInput,
    root: &Path,
    de: &Lifetime,
    bounded: &Generics,
    deserialize: TokenStream2,
    names: &InternalNames,
) -> TokenStream2 {
    let allocator = &names.allocator;
    let serde = &names.serde;
    let (impl_params, _, where_clause) = bounded.split_for_impl();
    let target = original_type(input);
    let arena_path = arena_path(root);
    quote! {
        impl #impl_params #root::DeserializeIn<#de, #allocator> for #target #where_clause {
            fn deserialize_in<__D>(
                arena: &#arena_path<#allocator>,
                deserializer: __D,
            ) -> ::core::result::Result<Self, __D::Error>
            where
                __D: #serde::Deserializer<#de>,
            {
                #deserialize
            }
        }
    }
}

fn struct_tokens(
    input: &DeriveInput,
    fields: &Fields,
    container: &ContainerAttrs,
    root: &Path,
    de: &Lifetime,
    arena: &Lifetime,
    bounded: &Generics,
    helper: &Generics,
    visitor: &Ident,
    names: &InternalNames,
) -> syn::Result<TokenStream2> {
    let serde = &names.serde;
    let infos = fields_info(fields, container.rename_all)?;
    let type_name = container.rename.clone().unwrap_or_else(|| input.ident.to_string());
    let name = &input.ident;
    let visitor_value = quote!(#visitor { arena, marker: ::core::marker::PhantomData });
    if container.transparent {
        let active_index = infos
            .iter()
            .position(|field| !field.attrs.skip)
            .expect("transparent field count validated");
        let visitor_tokens = transparent_visitor(
            input,
            &infos,
            active_index,
            matches!(fields, Fields::Named(_)),
            container,
            root,
            de,
            arena,
            bounded,
            helper,
            visitor,
            names,
        );
        let deserialize = quote!(#serde::Deserializer::deserialize_newtype_struct(deserializer, #type_name, #visitor_value));
        let implementation = target_impl(input, root, de, bounded, deserialize, names);
        return Ok(quote! {
            use #root::__private::serde as #serde;
            #visitor_tokens
            #implementation
        });
    }
    let (visitor_tokens, deserialize) = match fields {
        Fields::Named(_) => {
            let field = format_ident!("__MultitudeFieldFor{name}");
            let visitor_tokens = named_visitor(
                input,
                &infos,
                quote!(#name),
                container,
                root,
                de,
                arena,
                bounded,
                helper,
                visitor,
                &field,
                names,
            )?;
            let field_names = accepted_field_names(&infos);
            (
                visitor_tokens,
                quote!(#serde::Deserializer::deserialize_struct(deserializer, #type_name, &[#(#field_names),*], #visitor_value)),
            )
        }
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 && !infos[0].attrs.skip => (
            newtype_visitor(
                input,
                &infos[0],
                quote!(#name),
                container,
                root,
                de,
                arena,
                bounded,
                helper,
                visitor,
                names,
            ),
            quote!(#serde::Deserializer::deserialize_newtype_struct(deserializer, #type_name, #visitor_value)),
        ),
        Fields::Unnamed(_) => {
            let active_len = infos.iter().filter(|info| !info.attrs.skip).count();
            (
                tuple_visitor(
                    input,
                    &infos,
                    quote!(#name),
                    container,
                    root,
                    de,
                    arena,
                    bounded,
                    helper,
                    visitor,
                    names,
                ),
                quote!(#serde::Deserializer::deserialize_tuple_struct(deserializer, #type_name, #active_len, #visitor_value)),
            )
        }
        Fields::Unit => (
            unit_visitor(input, quote!(#name), container, root, de, arena, bounded, helper, visitor, names),
            quote!(#serde::Deserializer::deserialize_unit_struct(deserializer, #type_name, #visitor_value)),
        ),
    };
    let implementation = target_impl(input, root, de, bounded, deserialize, names);
    Ok(quote! {
        use #root::__private::serde as #serde;
        #visitor_tokens
        #implementation
    })
}

#[derive(Clone)]
struct VariantInfo<'a> {
    variant: &'a Variant,
    attrs: FieldAttrs,
    wire_name: String,
}

fn accepted_variant_names<'a>(infos: &'a [VariantInfo<'_>]) -> Vec<&'a str> {
    infos
        .iter()
        .filter(|info| !info.attrs.skip)
        .flat_map(|info| std::iter::once(info.wire_name.as_str()).chain(info.attrs.aliases.iter().map(String::as_str)))
        .collect()
}

fn variants_info(data: &DataEnum, rename_all: Option<RenameRule>) -> syn::Result<Vec<VariantInfo<'_>>> {
    data.variants
        .iter()
        .map(|variant| {
            let attrs = parse_variant(&variant.attrs)?;
            if (attrs.default.is_some() && !attrs.skip) || attrs.via_serde {
                return Err(syn::Error::new_spanned(
                    variant,
                    "`default` and `via_serde` are field-only multitude attributes",
                ));
            }
            let wire_name = attrs
                .rename
                .clone()
                .unwrap_or_else(|| rename_all.map_or_else(|| variant.ident.to_string(), |rule| rule.variant(&variant.ident.to_string())));
            Ok(VariantInfo { variant, attrs, wire_name })
        })
        .collect()
}

fn variant_enum(name: &Ident, infos: &[VariantInfo<'_>], names: &InternalNames) -> syn::Result<TokenStream2> {
    let serde = &names.serde;
    let variants: Vec<_> = (0..infos.len()).map(|index| format_ident!("Variant{index}")).collect();
    let declared_variants: Vec<_> = infos
        .iter()
        .enumerate()
        .filter(|(_, info)| !info.attrs.skip)
        .map(|(index, _)| &variants[index])
        .collect();
    let expected = accepted_variant_names(infos);
    let expected_const = format_ident!("{name}_VARIANTS");
    let mut seen = HashSet::new();
    let mut string_arms = Vec::new();
    let mut byte_arms = Vec::new();
    for (index, info) in infos.iter().enumerate().filter(|(_, info)| !info.attrs.skip) {
        let variant = &variants[index];
        for wire_name in std::iter::once(&info.wire_name).chain(&info.attrs.aliases) {
            if !seen.insert(wire_name.clone()) {
                return Err(syn::Error::new_spanned(
                    info.variant,
                    format!("duplicate deserialization variant name or alias `{wire_name}`"),
                ));
            }
            let bytes = syn::LitByteStr::new(wire_name.as_bytes(), Span::call_site());
            string_arms.push(quote!(#wire_name => ::core::result::Result::Ok(#name::#variant)));
            byte_arms.push(quote!(#bytes => ::core::result::Result::Ok(#name::#variant)));
        }
    }
    let ordinal_arms = infos
        .iter()
        .enumerate()
        .filter(|(_, info)| !info.attrs.skip)
        .enumerate()
        .map(|(ordinal, (index, _))| {
            let ordinal = ordinal as u64;
            let variant = &variants[index];
            quote!(#ordinal => ::core::result::Result::Ok(#name::#variant))
        });
    let ordinal_expecting = format!("variant index 0 <= i < {}", infos.iter().filter(|info| !info.attrs.skip).count());
    Ok(quote! {
        enum #name {
            #(#declared_variants,)*
        }

        impl<'__variant_de> #serde::Deserialize<'__variant_de> for #name {
            fn deserialize<__D>(deserializer: __D) -> ::core::result::Result<Self, __D::Error>
            where
                __D: #serde::Deserializer<'__variant_de>,
            {
                struct __VariantVisitor;
                impl<'__variant_de> #serde::de::Visitor<'__variant_de> for __VariantVisitor {
                    type Value = #name;

                    fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                        formatter.write_str("a variant name")
                    }

                    fn visit_str<__E>(self, __value: &str) -> ::core::result::Result<Self::Value, __E>
                    where
                        __E: #serde::de::Error,
                    {
                        match __value {
                            #(#string_arms,)*
                            _ => ::core::result::Result::Err(__E::unknown_variant(__value, #expected_const)),
                        }
                    }

                    fn visit_bytes<__E>(self, __value: &[u8]) -> ::core::result::Result<Self::Value, __E>
                    where
                        __E: #serde::de::Error,
                    {
                        match __value {
                            #(#byte_arms,)*
                            _ => match ::core::str::from_utf8(__value) {
                                ::core::result::Result::Ok(__value) => {
                                    ::core::result::Result::Err(__E::unknown_variant(__value, #expected_const))
                                }
                                ::core::result::Result::Err(_) => ::core::result::Result::Err(__E::invalid_value(
                                    #serde::de::Unexpected::Bytes(__value),
                                    &self,
                                )),
                            },
                        }
                    }

                    fn visit_u64<__E>(self, __value: u64) -> ::core::result::Result<Self::Value, __E>
                    where
                        __E: #serde::de::Error,
                    {
                        match __value {
                            #(#ordinal_arms,)*
                            _ => ::core::result::Result::Err(__E::invalid_value(
                                #serde::de::Unexpected::Unsigned(__value),
                                &#ordinal_expecting,
                            )),
                        }
                    }
                }
                #serde::Deserializer::deserialize_identifier(deserializer, __VariantVisitor)
            }
        }
        const #expected_const: &[&str] = &[#(#expected),*];
    })
}

fn enum_tokens(
    input: &DeriveInput,
    data: &DataEnum,
    container: &ContainerAttrs,
    root: &Path,
    de: &Lifetime,
    arena: &Lifetime,
    bounded: &Generics,
    helper: &Generics,
    visitor: &Ident,
    names: &InternalNames,
) -> syn::Result<TokenStream2> {
    let serde = &names.serde;
    let infos = variants_info(data, container.rename_all)?;
    let enum_name = &input.ident;
    let type_name = container.rename.clone().unwrap_or_else(|| enum_name.to_string());
    let variant_key = format_ident!("__MultitudeVariantFor{enum_name}");
    let variant_definition = variant_enum(&variant_key, &infos, names)?;
    let definition = visitor_definition(input, root, arena, helper, visitor, names);
    let helper_impl = helper_impl_generics(bounded, arena);
    let (impl_params, _, where_clause) = helper_impl.split_for_impl();
    let visitor_args = visitor_type_args(input, arena, names);
    let target = original_type(input);
    let expecting = container.expecting.as_deref().unwrap_or("an externally tagged enum");
    let mut payload_visitors = Vec::new();
    let mut variant_arms = Vec::new();

    for (index, info) in infos.iter().enumerate().filter(|(_, info)| !info.attrs.skip) {
        let variant_key_arm = format_ident!("Variant{index}");
        let variant = &info.variant.ident;
        let fields = fields_info(&info.variant.fields, info.attrs.rename_all.or(container.rename_all_fields))?;
        if let Some((definition, seed)) =
            custom_variant_seed_definition(input, info, index, root, de, arena, bounded, helper, visitor, names)
        {
            payload_visitors.push(definition);
            let construction = construct_custom_variant(enum_name, variant, &info.variant.fields);
            variant_arms.push(quote! {
                #variant_key::#variant_key_arm => {
                    let __value = #serde::de::VariantAccess::newtype_variant_seed(__access, #seed)?;
                    ::core::result::Result::Ok({ #construction })
                }
            });
            continue;
        }
        match &info.variant.fields {
            Fields::Unit => variant_arms.push(quote! {
                #variant_key::#variant_key_arm => {
                    #serde::de::VariantAccess::unit_variant(__access)?;
                    ::core::result::Result::Ok(#enum_name::#variant)
                }
            }),
            Fields::Unnamed(fields_decl) if fields_decl.unnamed.len() == 1 && !fields[0].attrs.skip => {
                let seed_owner = format_ident!("{visitor}Variant{index}");
                if let Some(definition) = custom_seed_definition(input, &fields[0], 0, root, de, arena, bounded, helper, &seed_owner, names)
                {
                    payload_visitors.push(definition);
                }
                let value_seed = seed(&fields[0], root, &seed_owner, 0, names);
                variant_arms.push(quote! {
                    #variant_key::#variant_key_arm => {
                        let __value = #serde::de::VariantAccess::newtype_variant_seed(__access, #value_seed)?;
                        ::core::result::Result::Ok(#enum_name::#variant(__value))
                    }
                });
            }
            Fields::Unnamed(_) => {
                let payload_visitor = format_ident!("__MultitudeVisitorFor{enum_name}Variant{index}");
                payload_visitors.push(tuple_visitor(
                    input,
                    &fields,
                    quote!(#enum_name::#variant),
                    container,
                    root,
                    de,
                    arena,
                    bounded,
                    helper,
                    &payload_visitor,
                    names,
                ));
                let active_len = fields.iter().filter(|field| !field.attrs.skip).count();
                variant_arms.push(quote! {
                    #variant_key::#variant_key_arm => #serde::de::VariantAccess::tuple_variant(
                        __access,
                        #active_len,
                        #payload_visitor { arena: self.arena, marker: ::core::marker::PhantomData },
                    )
                });
            }
            Fields::Named(_) => {
                let payload_visitor = format_ident!("__MultitudeVisitorFor{enum_name}Variant{index}");
                let field_key = format_ident!("__MultitudeFieldFor{enum_name}Variant{index}");
                payload_visitors.push(named_visitor(
                    input,
                    &fields,
                    quote!(#enum_name::#variant),
                    container,
                    root,
                    de,
                    arena,
                    bounded,
                    helper,
                    &payload_visitor,
                    &field_key,
                    names,
                )?);
                let field_names = accepted_field_names(&fields);
                variant_arms.push(quote! {
                    #variant_key::#variant_key_arm => #serde::de::VariantAccess::struct_variant(
                        __access,
                        &[#(#field_names),*],
                        #payload_visitor { arena: self.arena, marker: ::core::marker::PhantomData },
                    )
                });
            }
        }
    }
    let visitor_impl = quote! {
        #definition
        impl #impl_params #serde::de::Visitor<#de> for #visitor #visitor_args #where_clause {
            type Value = #target;

            fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(#expecting)
            }

            fn visit_enum<__E>(self, __data: __E) -> ::core::result::Result<Self::Value, __E::Error>
            where
                __E: #serde::de::EnumAccess<#de>,
            {
                let (__variant, __access) = #serde::de::EnumAccess::variant::<#variant_key>(__data)?;
                match __variant {
                    #(#variant_arms,)*
                }
            }
        }
    };
    let variant_names = accepted_variant_names(&infos);
    let deserialize = quote! {
        #serde::Deserializer::deserialize_enum(
            deserializer,
            #type_name,
            &[#(#variant_names),*],
            #visitor { arena, marker: ::core::marker::PhantomData },
        )
    };
    let implementation = target_impl(input, root, de, bounded, deserialize, names);
    Ok(quote! {
        use #root::__private::serde as #serde;
        #variant_definition
        #(#payload_visitors)*
        #visitor_impl
        #implementation
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(source: &str) -> String {
        let input: DeriveInput = syn::parse_str(source).unwrap();
        let root: Path = parse_quote!(::multitude::de);
        let generated = super::expand(&input, &root).unwrap();
        let file: syn::File = syn::parse2(generated).unwrap();
        prettyplease::unparse(&file)
    }

    #[test]
    fn generates_all_data_shapes() {
        for source in [
            "struct Named<T> { value: T, #[multitude(via_serde)] id: u32 }",
            "struct Tuple<T>(T, #[multitude(skip)] u8);",
            "struct Unit;",
            "enum E<T> { Unit, New(T), Tuple(T, T), Struct { value: T } }",
        ] {
            let generated = expand(source);
            assert!(generated.contains("DeserializeIn"));
        }
        let newtype = expand("struct HasField<T>(T);");
        assert!(newtype.contains("deserialize_newtype_struct"));
        assert!(newtype.contains("DeserializeInSeed"));

        let skipped_single = expand("struct Skipped(#[serde(skip)] u8);");
        let compact: String = skipped_single.split_whitespace().collect();
        assert!(skipped_single.contains("deserialize_tuple_struct"));
        assert!(!skipped_single.contains("deserialize_newtype_struct"));
        assert!(compact.contains("deserialize_tuple_struct(deserializer,\"Skipped\",0usize,"));

        let tuple = expand("struct Pair(u8, u16);");
        assert!(tuple.contains("struct __MultitudeVisitorForPair"));
        assert!(tuple.contains("formatter.write_str(\"a sequence\")"));
        assert!(tuple.contains("SeqAccess::next_element_seed"));

        let unit = expand("struct Unit;");
        assert!(unit.contains("struct __MultitudeVisitorForUnit"));
        assert!(unit.contains("fn visit_unit"));
    }

    #[test]
    fn named_visitors_generate_sequence_and_compact_ordinal_support() {
        let structure = expand("struct S { first: u8, #[serde(skip)] skipped: u8, #[serde(default)] last: u8 }");
        assert!(structure.contains("fn visit_seq"));
        assert_eq!(structure.matches("SeqAccess::next_element_seed").count(), 2);
        assert!(structure.contains("fn visit_u64"));
        assert!(structure.contains("1u64 =>"));
        assert!(structure.contains("Field2"));
        assert!(structure.contains("Ignore"));
        assert!(!structure.contains("Field1,"));

        let required = expand("struct Required { first: u8, second: u8 }");
        let compact: String = required.split_whitespace().collect();
        assert!(compact.contains("invalid_length(1usize,&self)"));

        let strict = expand("#[serde(deny_unknown_fields)] struct S { first: u8, last: u8 }");
        assert!(strict.contains("field index 0 <= i < 2"));

        let container_skip = expand(r#"#[serde(default = "make")] struct S { value: u8, #[serde(skip)] skipped: NoDefault }"#);
        assert!(container_skip.contains("__container_default1"));

        let explicit_defaults =
            expand(r#"#[serde(default = "make")] struct S { #[serde(default)] value: u8, #[serde(skip, default)] skipped: u8 }"#);
        assert!(explicit_defaults.contains("let _: S = make();"));
        assert!(!explicit_defaults.contains("__container_default1"));

        let all_skipped = expand("struct AllSkipped { #[serde(skip)] value: u8 }");
        assert!(all_skipped.contains("fn visit_seq"));
        assert!(!all_skipped.contains("mut __seq"));
        assert!(!all_skipped.contains("let mut __field0"));

        let enumeration = expand("enum E { First, #[serde(skip)] Hidden, Last }");
        let compact_enumeration: String = enumeration.split_whitespace().collect();
        assert!(enumeration.contains("fn visit_u64"));
        assert!(enumeration.contains("variant index 0 <= i < 2"));
        assert!(enumeration.contains("Variant2"));
        assert!(!enumeration.contains("Variant1,"));
        assert!(compact_enumeration.contains("1u64=>{::core::result::Result::Ok(__MultitudeVariantForE::Variant2)"));
        assert!(!compact_enumeration.contains("2u64=>"));
    }

    #[test]
    fn rejects_representation_changes() {
        let input: DeriveInput = syn::parse_str("#[serde(untagged)] enum E { A }").unwrap();
        let root: Path = parse_quote!(::multitude::de);
        assert!(super::expand(&input, &root).unwrap_err().to_string().contains("temporary borrow"));
    }

    #[test]
    fn adds_default_bounds_including_for_skipped_fields() {
        let generated = expand("struct Defaults<T, U> { #[serde(default)] value: T, #[multitude(skip)] skipped: U }");
        assert!(generated.contains("T: ::core::default::Default"));
        assert!(generated.contains("U: ::core::default::Default"));

        let path_defaults =
            expand("#[serde(default = \"make\")] struct Paths<T, U> { value: T, #[serde(skip, default = \"other\")] skipped: U }");
        assert!(!path_defaults.contains("T: ::core::default::Default"));
        assert!(!path_defaults.contains("U: ::core::default::Default"));

        let custom_then_regular = expand("struct CustomThenRegular { #[serde(deserialize_with = \"decode\")] first: u8, second: u8 }");
        assert!(custom_then_regular.contains("u8: ::multitude::de::DeserializeIn"));

        let required = expand("struct Required { value: u64 }");
        assert!(!required.contains("u64: ::core::default::Default"));
    }

    #[test]
    fn deny_unknown_does_not_recognize_skipped_names() {
        let generated = expand("#[serde(deny_unknown_fields)] struct Strict { #[serde(skip)] ignored: u8 }");
        assert!(!generated.contains("\"ignored\" =>"));
    }

    #[test]
    fn skipped_variant_payload_does_not_add_bounds() {
        let generated = expand("enum E<T> { #[serde(skip)] Hidden(T), Visible }");
        assert!(!generated.contains("T: ::multitude::de::DeserializeIn"));
    }

    #[test]
    fn rejects_reserved_generic_names() {
        let input: DeriveInput = syn::parse_str("struct Collision<__D>(__D);").unwrap();
        let root: Path = parse_quote!(::multitude::de);
        assert!(super::expand(&input, &root).unwrap_err().to_string().contains("reserves"));
    }

    #[test]
    fn internal_names_do_not_shadow_user_identifiers() {
        let allocator = expand("struct __A<__Serde> { value: __Serde }");
        assert!(allocator.contains("for __A<__Serde>"));
        assert!(allocator.contains("__A_"));
        assert!(allocator.contains("__Serde_"));
    }

    #[test]
    fn uses_custom_multitude_crate_path() {
        let generated = expand("#[multitude(crate = \"renamed\")] struct Custom<T> { value: T }");
        assert!(generated.contains("renamed::de::DeserializeIn"));
        assert!(!generated.contains("::multitude::de::DeserializeIn"));
    }

    #[test]
    fn generates_all_serde_rename_rules() {
        let cases = [
            ("lowercase", "my_field", "my_field", "HttpServer", "httpserver"),
            ("UPPERCASE", "my_field", "MY_FIELD", "HttpServer", "HTTPSERVER"),
            ("PascalCase", "my_field", "MyField", "HttpServer", "HttpServer"),
            ("camelCase", "my_field", "myField", "HttpServer", "httpServer"),
            ("snake_case", "my_field", "my_field", "HttpServer", "http_server"),
            ("SCREAMING_SNAKE_CASE", "my_field", "MY_FIELD", "HttpServer", "HTTP_SERVER"),
            ("kebab-case", "my_field", "my-field", "HttpServer", "http-server"),
            ("SCREAMING-KEBAB-CASE", "my_field", "MY-FIELD", "HttpServer", "HTTP-SERVER"),
        ];
        for (rule, field, expected_field, variant, expected_variant) in cases {
            let structure = expand(&format!(
                "#[serde(rename_all(deserialize = \"{rule}\"))] struct S {{ {field}: u8 }}"
            ));
            assert!(structure.contains(&format!("\"{expected_field}\"")));
            let enumeration = expand(&format!("#[serde(rename_all = \"{rule}\")] enum E {{ {variant} }}"));
            assert!(enumeration.contains(&format!("\"{expected_variant}\"")));
        }

        let pascal = expand("#[serde(rename_all = \"PascalCase\")] struct S { http_URL: u8 }");
        assert!(pascal.contains("\"HttpURL\""));
        let camel = expand("#[serde(rename_all = \"camelCase\")] struct S { http_URL: u8 }");
        assert!(camel.contains("\"httpURL\""));

        let variant = expand(
            "#[serde(rename_all_fields = \"SCREAMING_SNAKE_CASE\")] enum E { #[serde(rename_all = \"camelCase\")] V { some_field: u8 } }",
        );
        assert!(variant.contains("\"someField\""));
        assert!(!variant.contains("\"SOME_FIELD\""));
    }

    #[test]
    fn accepted_name_slices_include_aliases() {
        let structure = expand("struct S { #[serde(rename = \"current\", alias = \"old\", alias = \"legacy\")] value: u8 }");
        assert!(structure.contains("&[\"current\", \"old\", \"legacy\"]"));

        let enumeration = expand("enum E { #[serde(rename = \"Current\", alias = \"Old\")] Value }");
        assert!(enumeration.contains("&[\"Current\", \"Old\"]"));

        let struct_variant = expand("enum E { V { #[serde(rename = \"current\", alias = \"old\")] value: u8 } }");
        assert!(struct_variant.contains("&[\"current\", \"old\"]"));
    }

    #[test]
    fn generates_container_default_and_explicit_bounds() {
        let generated = expand("#[serde(default, bound(deserialize = \"T: SomeTrait<'de>\"))] struct S<T> { value: T }");
        assert!(generated.contains("S<T>: ::core::default::Default"));
        assert!(generated.contains("T: SomeTrait<'de>"));
        assert!(!generated.contains("T: ::multitude::de::DeserializeIn"));
        assert!(generated.contains("let S { value: __container_default0"));
    }

    #[test]
    fn generates_container_default_paths_and_custom_expectations() {
        let generated = expand("#[serde(default = \"make_default\", expecting = \"a custom record\")] struct S<T> { value: T }");
        assert!(generated.contains("= make_default();"));
        assert!(!generated.contains("S<T>: ::core::default::Default"));
        assert!(generated.contains("formatter.write_str(\"a custom record\")"));
    }

    #[test]
    fn generates_serde_and_multitude_custom_deserializers() {
        let serde_generated = expand("struct S { #[serde(deserialize_with = \"ordinary\")] value: u8 }");
        assert!(serde_generated.contains("ordinary(deserializer)"));
        assert!(serde_generated.contains("__MultitudeVisitorForSField0Seed {"));
        assert!(!serde_generated.contains("u8: ::multitude::de::DeserializeIn"));

        let with_generated = expand("struct S { #[serde(with = \"module\")] value: u8 }");
        assert!(with_generated.contains("module::deserialize(deserializer)"));

        let multitude_generated = expand("struct S { #[multitude(deserialize_with = \"arena_value\")] value: u8 }");
        assert!(multitude_generated.contains("arena_value(self.arena, deserializer)"));
        assert!(multitude_generated.contains("__MultitudeVisitorForSField0Seed {"));

        let variant = expand(
            r#"enum E<T> {
                #[serde(deserialize_with = "decode_unit")]
                Unit,
                #[serde(deserialize_with = "decode_unit")]
                EmptyTuple(),
                #[serde(deserialize_with = "decode_unit")]
                EmptyNamed {},
                #[serde(with = "codec")]
                New(T),
                #[multitude(deserialize_with = "decode_pair")]
                Pair(T, T),
                #[serde(deserialize_with = "decode_single")]
                Single { value: T },
                #[serde(deserialize_with = "decode_named")]
                Named { left: T, right: T },
            }"#,
        );
        assert!(variant.contains("decode_unit(deserializer)"));
        assert!(variant.contains("codec::deserialize(deserializer)"));
        assert!(variant.contains("decode_pair(self.arena, deserializer)"));
        assert!(variant.contains("decode_single(deserializer)"));
        assert!(variant.contains("decode_named(deserializer)"));
        assert!(!variant.contains("T: ::multitude::de::DeserializeIn"));

        let compact: String = variant.split_whitespace().collect();
        assert_eq!(compact.matches("let_:()=__value;").count(), 3);
        let single_named_constructor = ["E::Single", "{", "value:__value", "}"].concat();
        assert!(compact.contains(&single_named_constructor));
        for constructor in [
            "E::Unit",
            "E::EmptyTuple()",
            "E::EmptyNamed{}",
            "E::New(__value)",
            "E::Pair(__value.0,__value.1)",
            "E::Named",
            "left:__value.0",
            "right:__value.1",
        ] {
            assert!(compact.contains(constructor), "missing generated constructor `{constructor}`");
        }
    }

    #[test]
    fn generates_transparent_structs() {
        let named = expand("#[serde(transparent)] struct Named<T> { value: T, #[serde(skip)] marker: u8 }");
        assert!(named.contains("deserialize_newtype_struct"));
        assert!(named.contains("Named {"));
        assert!(named.contains("value: __transparent_value"));

        let tuple = expand("#[serde(transparent)] struct Tuple<T>(#[serde(skip)] u8, T);");
        assert!(tuple.contains("::core::default::Default::default()"));
        assert!(tuple.contains("__transparent_value"));
        assert!(tuple.contains("DeserializeInSeed::<T, __A>"));
        assert!(!tuple.contains("DeserializeInSeed::<u8, __A>"));

        let asymmetric = expand("#[serde(transparent)] struct Asymmetric<T>(#[serde(skip)] u8, T, #[serde(skip)] u16);");
        assert!(asymmetric.contains("DeserializeInSeed::<T, __A>"));
    }

    #[test]
    fn rejects_invalid_transparent_shapes() {
        let root: Path = parse_quote!(::multitude::de);
        for source in [
            "#[serde(transparent)] enum E { A }",
            "#[serde(transparent)] struct Empty;",
            "#[serde(transparent)] struct Two(u8, u8);",
        ] {
            let input: DeriveInput = syn::parse_str(source).unwrap();
            super::expand(&input, &root).unwrap_err();
        }
    }

    #[test]
    fn public_entry_point_and_top_level_validation_errors_are_covered() {
        let root: Path = parse_quote!(::multitude::de);
        assert!(
            derive_deserialize_in(quote!(not valid rust), &root)
                .to_string()
                .contains("compile_error")
        );

        for source in [
            "union Unsupported { value: u64 }",
            "struct ReservedConst<const __S: usize>;",
            "struct ReservedPrefix<__MultitudeValue>(__MultitudeValue);",
            "struct ReservedLifetime<'de>(&'de str);",
            "#[serde(default)] struct Tuple(u64);",
            "#[serde(transparent, default)] struct Transparent { value: u64 }",
        ] {
            let input: DeriveInput = syn::parse_str(source).unwrap();
            assert!(super::expand(&input, &root).is_err(), "{source}");
        }
    }

    #[test]
    fn generic_and_default_generation_helpers_cover_all_parameter_kinds() {
        let generated = expand("struct Generic<'a, T = u64, const N: usize = 1> { value: T, #[multitude(via_serde)] marker: &'a [u8; N] }");
        assert!(generated.contains("Generic<'a, T, N>"));
        assert!(generated.contains("__arena"));
        assert!(!generated.contains("T = u64"));
        assert!(!generated.contains("N: usize = 1"));
        assert!(generated.contains("struct __MultitudeVisitorForGeneric<'__arena, 'a, T, const N: usize, __A>"));
        assert!(generated.contains("for __MultitudeVisitorForGeneric<'__arena, 'a, T, N, __A>"));

        let bounded: Generics = parse_quote!(<'de, T, __A>);
        let arena: Lifetime = parse_quote!('__arena);
        let helper_impl = helper_impl_generics(&bounded, &arena);
        let compact: String = helper_impl.to_token_stream().to_string().split_whitespace().collect();
        assert_eq!(compact, "<'de,'__arena,T,__A>");

        let colliding_lifetime = expand("struct Lifetimes<'__arena, '__arena_> { value: &'__arena str, other: &'__arena_ str }");
        assert!(colliding_lifetime.contains("'__arena__"));

        let path_default = expand("struct PathDefault { #[serde(default = \"make_value\")] value: u64 }");
        assert!(path_default.contains("make_value()"));
        let duplicate_default_type = expand("struct DuplicateDefault { #[serde(default)] one: u64, #[serde(default)] two: u64 }");
        assert!(duplicate_default_type.contains("u64: ::core::default::Default"));
        let missing = expand("struct Missing { value: u64 }");
        assert!(missing.contains("missing_field"));
    }

    #[test]
    fn duplicate_names_and_invalid_variant_attributes_are_rejected() {
        let root: Path = parse_quote!(::multitude::de);
        for source in [
            "struct Duplicate { #[serde(alias = \"same\")] one: u8, #[serde(rename = \"same\")] two: u8 }",
            "enum Duplicate { #[serde(alias = \"Same\")] One, #[serde(rename = \"Same\")] Two }",
            "enum DuplicateFields { Variant { #[serde(alias = \"same\")] one: u8, #[serde(rename = \"same\")] two: u8 } }",
            "enum DefaultVariant { #[serde(default)] One }",
            "enum ViaVariant { #[multitude(via_serde)] One }",
        ] {
            let input: DeriveInput = syn::parse_str(source).unwrap();
            assert!(super::expand(&input, &root).is_err(), "{source}");
        }
    }

    #[test]
    fn skipped_unknown_and_custom_enum_payload_generation_is_covered() {
        let skipped = expand("struct Lenient { #[serde(skip)] ignored: u8, value: u8 }");
        assert!(skipped.contains("\"ignored\" =>"));
        assert!(skipped.contains("Ignore"));
        assert!(!skipped.contains("__MultitudeFieldForLenient::Field0 =>"));
        assert!(skipped.contains("__MultitudeFieldForLenient::Field1 =>"));

        let custom = expand(
            "enum Custom { New(#[serde(deserialize_with = \"decode\")] u64), Tuple(#[serde(skip)] u8, u64), Struct { #[multitude(deserialize_with = \"arena_decode\")] value: u64 } }",
        );
        assert!(custom.contains("decode(deserializer)"));
        assert!(custom.contains("arena_decode(self.arena, deserializer)"));
        assert!(custom.contains("tuple_variant"));
        assert!(custom.contains("struct_variant"));

        let shapes = expand("enum Shapes { New(u8), OneSkipped(#[serde(skip)] u8), Unit, Named { value: u8 } }");
        let compact: String = shapes.split_whitespace().collect();
        assert!(compact.contains("__MultitudeVariantForShapes::Variant0=>{let__value=__Serde::de::VariantAccess::newtype_variant_seed"));
        assert!(compact.contains("__MultitudeVariantForShapes::Variant1=>{__Serde::de::VariantAccess::tuple_variant(__access,0usize,"));
        assert!(shapes.contains("tuple_variant"));
        assert!(shapes.contains("struct_variant"));
        assert!(shapes.contains("unit_variant"));

        let tuple = expand("enum TupleShape { Pair(u8, u16) }");
        let compact: String = tuple.split_whitespace().collect();
        assert!(compact.contains("tuple_variant(__access,2usize,"));
    }
}
