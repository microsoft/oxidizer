// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementations of the direct query codec derives.

use alloc::borrow::ToOwned as _;
use alloc::format;
use alloc::string::{String, ToString as _};
use alloc::vec::Vec;
use std::collections::{HashMap, HashSet};

use proc_macro2::{Group, Ident, Span, TokenStream, TokenTree};
use quote::{format_ident, quote};
use syn::ext::IdentExt as _;
use syn::spanned::Spanned as _;
use syn::{
    Attribute, Data, DeriveInput, Expr, Field, Fields, GenericArgument, GenericParam, Generics, LitStr, PathArguments, Token, Type,
    parenthesized,
};

use super::resolver::runtime_path;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Camel,
    Snake,
    Kebab,
    ScreamingSnake,
}

impl Case {
    fn parse(value: &LitStr) -> syn::Result<Self> {
        match value.value().as_str() {
            "camelCase" => Ok(Self::Camel),
            "snake_case" => Ok(Self::Snake),
            "kebab-case" => Ok(Self::Kebab),
            "SCREAMING_SNAKE_CASE" => Ok(Self::ScreamingSnake),
            _ => Err(syn::Error::new(
                value.span(),
                "supported rename rules are camelCase, snake_case, kebab-case, and SCREAMING_SNAKE_CASE",
            )),
        }
    }

    fn apply(self, name: &str) -> String {
        let words = words(name);
        match self {
            Self::Camel => {
                let mut iter = words.into_iter();
                let first = iter.next().unwrap_or_default();
                first + &iter.map(|word| capitalize(&word)).collect::<String>()
            }
            Self::Snake => words.join("_"),
            Self::Kebab => words.join("-"),
            Self::ScreamingSnake => words.join("_").to_ascii_uppercase(),
        }
    }
}

fn words(name: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    for character in name.chars() {
        if character == '_' || character == '-' {
            if !current.is_empty() {
                result.push(current.to_ascii_lowercase());
                current.clear();
            }
        } else if character.is_ascii_uppercase() && !current.is_empty() {
            result.push(current.to_ascii_lowercase());
            current.clear();
            current.push(character);
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        result.push(current.to_ascii_lowercase());
    }
    result
}

fn capitalize(word: &str) -> String {
    let mut characters = word.chars();
    characters
        .next()
        .map(|first| first.to_ascii_uppercase().to_string() + characters.as_str())
        .unwrap_or_default()
}

#[derive(Default)]
struct RawAttrs {
    rename: Option<(String, Span)>,
    rename_all: Option<(Case, Span)>,
    aliases: Vec<(String, Span)>,
    default: Option<Span>,
    flatten: Option<Span>,
    skip: Option<Span>,
    deny_unknown_fields: Option<Span>,
}

fn parse_attrs(attrs: &[Attribute], namespace: &str) -> syn::Result<RawAttrs> {
    let mut result = RawAttrs::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident(namespace)) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let value: LitStr = meta.value()?.parse()?;
                set_string(&mut result.rename, &value, "rename")
            } else if meta.path.is_ident("rename_all") {
                let value: LitStr = meta.value()?.parse()?;
                let case = Case::parse(&value)?;
                set_once(&mut result.rename_all, (case, value.span()), "rename_all", meta.path.span())
            } else if meta.path.is_ident("alias") {
                let value: LitStr = meta.value()?.parse()?;
                result.aliases.push((value.value(), value.span()));
                Ok(())
            } else if meta.path.is_ident("default") {
                set_flag(&mut result.default, meta.path.span(), "default")
            } else if meta.path.is_ident("flatten") {
                set_flag(&mut result.flatten, meta.path.span(), "flatten")
            } else if meta.path.is_ident("skip") {
                set_flag(&mut result.skip, meta.path.span(), "skip")
            } else if namespace == "query" && meta.path.is_ident("deny_unknown_fields") {
                set_flag(&mut result.deny_unknown_fields, meta.path.span(), "deny_unknown_fields")
            } else if namespace == "query" && meta.path.is_ident("repeated") {
                Err(meta.error("unsupported query attribute `repeated`; Vec<T> fields are repeated automatically"))
            } else {
                // Serde has many unrelated behavioral attributes. They remain
                // Serde's concern, while query attributes should catch typos.
                if namespace == "serde" {
                    if meta.input.peek(Token![=]) {
                        let value = meta.value()?;
                        let _: Expr = value.parse()?;
                    } else if meta.input.peek(syn::token::Paren) {
                        let content;
                        parenthesized!(content in meta.input);
                        let _: TokenStream = content.parse()?;
                    }
                    Ok(())
                } else {
                    Err(meta.error("unsupported query attribute"))
                }
            }
        })?;
    }
    Ok(result)
}

fn set_string(slot: &mut Option<(String, Span)>, value: &LitStr, name: &str) -> syn::Result<()> {
    set_once(slot, (value.value(), value.span()), name, value.span())
}

fn set_once<T>(slot: &mut Option<T>, value: T, name: &str, span: Span) -> syn::Result<()> {
    if slot.is_some() {
        Err(syn::Error::new(span, format!("duplicate `{name}` attribute")))
    } else {
        *slot = Some(value);
        Ok(())
    }
}

fn set_flag(slot: &mut Option<Span>, span: Span, name: &str) -> syn::Result<()> {
    set_once(slot, span, name, span)
}

fn merge_attrs(query: RawAttrs, serde: RawAttrs) -> syn::Result<RawAttrs> {
    let rename = merge_value(query.rename, serde.rename, "rename")?;
    let rename_all = merge_value(query.rename_all, serde.rename_all, "rename_all")?;
    let mut aliases = serde.aliases;
    for alias in query.aliases {
        if !aliases.iter().any(|(value, _)| value == &alias.0) {
            aliases.push(alias);
        }
    }
    Ok(RawAttrs {
        rename,
        rename_all,
        aliases,
        default: merge_flag(query.default, serde.default),
        flatten: merge_flag(query.flatten, serde.flatten),
        skip: merge_flag(query.skip, serde.skip),
        deny_unknown_fields: query.deny_unknown_fields,
    })
}

fn merge_value<T: PartialEq>(query: Option<(T, Span)>, serde: Option<(T, Span)>, name: &str) -> syn::Result<Option<(T, Span)>> {
    match (query, serde) {
        (Some(query), Some(serde)) if query.0 != serde.0 => {
            Err(syn::Error::new(query.1, format!("conflicting query and serde `{name}` attributes")))
        }
        (Some(query), _) => Ok(Some(query)),
        (None, serde) => Ok(serde),
    }
}

fn merge_flag(query: Option<Span>, serde: Option<Span>) -> Option<Span> {
    query.or(serde)
}

#[derive(Clone, Copy)]
enum ValueKind<'a> {
    Str,
    Cow,
    String,
    Other(&'a Type),
}

enum FieldKind<'a> {
    Scalar(ValueKind<'a>),
    Optional(ValueKind<'a>, &'a Type),
    Repeated(ValueKind<'a>, &'a Type),
    Flatten,
    Skip,
}

struct QueryField<'a> {
    field: &'a Field,
    ident: &'a Ident,
    name: String,
    aliases: Vec<String>,
    attrs: RawAttrs,
    kind: FieldKind<'a>,
}

struct QueryInput<'a> {
    fields: Vec<QueryField<'a>>,
    deny_unknown_fields: bool,
    query_lifetime: Option<syn::Lifetime>,
}

impl<'a> QueryInput<'a> {
    fn parse(input: &'a DeriveInput, decoding: bool) -> syn::Result<Self> {
        let named = match &input.data {
            Data::Struct(data) => match &data.fields {
                Fields::Named(named) => named,
                _ => {
                    return Err(syn::Error::new(
                        data.fields.span(),
                        "query derives support only structs with named fields",
                    ));
                }
            },
            _ => {
                return Err(syn::Error::new(
                    input.span(),
                    "query derives support only structs with named fields",
                ));
            }
        };
        let container = merge_attrs(parse_attrs(&input.attrs, "query")?, parse_attrs(&input.attrs, "serde")?)?;
        if container.rename.is_some()
            || !container.aliases.is_empty()
            || container.default.is_some()
            || container.flatten.is_some()
            || container.skip.is_some()
        {
            return Err(syn::Error::new(
                input.span(),
                "this query attribute is not supported on a container",
            ));
        }
        let rename_all = container.rename_all.map(|value| value.0);
        let mut fields = Vec::with_capacity(named.named.len());
        let mut parameter_names: HashMap<String, Span> = HashMap::new();
        let generic_types = input
            .generics
            .type_params()
            .map(|parameter| parameter.ident.to_string())
            .collect::<HashSet<_>>();
        for field in &named.named {
            let ident = field.ident.as_ref().expect("named fields have identifiers");
            let attrs = merge_attrs(parse_attrs(&field.attrs, "query")?, parse_attrs(&field.attrs, "serde")?)?;
            if attrs.rename_all.is_some() || attrs.deny_unknown_fields.is_some() {
                return Err(syn::Error::new(
                    field.span(),
                    "rename_all and deny_unknown_fields are container attributes",
                ));
            }
            let rust_name = ident.unraw().to_string();
            let name = attrs.rename.as_ref().map_or_else(
                || rename_all.map_or_else(|| rust_name.clone(), |case| case.apply(&rust_name)),
                |value| value.0.clone(),
            );
            let kind = classify(field, &attrs, &generic_types)?;
            let aliases = attrs.aliases.iter().map(|alias| alias.0.clone()).collect::<Vec<_>>();
            if !matches!(kind, FieldKind::Flatten | FieldKind::Skip) {
                for (value, span) in std::iter::once((&name, field.span())).chain(attrs.aliases.iter().map(|alias| (&alias.0, alias.1))) {
                    if parameter_names.insert(value.clone(), span).is_some() {
                        return Err(syn::Error::new(span, format!("duplicate query parameter name `{value}`")));
                    }
                }
            }
            fields.push(QueryField {
                field,
                ident,
                name,
                aliases,
                attrs,
                kind,
            });
        }
        let query_lifetime = if decoding { query_lifetime(&fields)? } else { None };
        Ok(Self {
            fields,
            deny_unknown_fields: container.deny_unknown_fields.is_some(),
            query_lifetime,
        })
    }
}

fn classify<'a>(field: &'a Field, attrs: &RawAttrs, generic_types: &HashSet<String>) -> syn::Result<FieldKind<'a>> {
    let has_name = attrs.rename.is_some() || !attrs.aliases.is_empty();
    if attrs.skip.is_some() {
        if attrs.flatten.is_some() || attrs.default.is_some() || has_name {
            return Err(syn::Error::new(
                field.span(),
                "`skip` cannot be combined with other query field attributes",
            ));
        }
        return Ok(FieldKind::Skip);
    }
    if attrs.flatten.is_some() {
        if attrs.default.is_some() || has_name {
            return Err(syn::Error::new(
                field.span(),
                "`flatten` cannot be combined with default, rename, or alias",
            ));
        }
        return Ok(FieldKind::Flatten);
    }
    if let Some(inner) = container_inner(&field.ty, "Option", generic_types) {
        return Ok(FieldKind::Optional(value_kind(inner, generic_types), inner));
    }
    if let Some(inner) = container_inner(&field.ty, "Vec", generic_types) {
        return Ok(FieldKind::Repeated(value_kind(inner, generic_types), inner));
    }
    Ok(FieldKind::Scalar(value_kind(&field.ty, generic_types)))
}

fn container_inner<'a>(ty: &'a Type, expected: &str, generic_types: &HashSet<String>) -> Option<&'a Type> {
    let Type::Path(path) = ty else { return None };
    if !standard_path(&path.path, expected, generic_types) {
        return None;
    }
    let segment = path.path.segments.last()?;
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    if arguments.args.len() != 1 {
        return None;
    }
    match arguments.args.first()? {
        GenericArgument::Type(ty) => Some(ty),
        _ => None,
    }
}

fn value_kind<'a>(ty: &'a Type, generic_types: &HashSet<String>) -> ValueKind<'a> {
    if matches!(ty, Type::Reference(reference) if reference.mutability.is_none() && matches!(&*reference.elem, Type::Path(path) if path.path.is_ident("str")))
    {
        ValueKind::Str
    } else if is_cow_str(ty, generic_types) {
        ValueKind::Cow
    } else if matches!(ty, Type::Path(path) if standard_path(&path.path, "String", generic_types)) {
        ValueKind::String
    } else {
        ValueKind::Other(ty)
    }
}

fn is_cow_str(ty: &Type, generic_types: &HashSet<String>) -> bool {
    let Type::Path(path) = ty else { return false };
    if !standard_path(&path.path, "Cow", generic_types) {
        return false;
    }
    let segment = path
        .path
        .segments
        .last()
        .expect("standard_path accepts only nonempty paths ending in Cow");
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    let mut arguments = arguments.args.iter();
    matches!(arguments.next(), Some(GenericArgument::Lifetime(_)))
        && matches!(
            arguments.next(),
            Some(GenericArgument::Type(Type::Path(path))) if path.path.is_ident("str")
        )
        && arguments.next().is_none()
}

fn borrowed_lifetime(kind: &FieldKind<'_>, field_ty: &Type, span: Span) -> syn::Result<Option<syn::Lifetime>> {
    let (value_kind, ty) = match kind {
        FieldKind::Scalar(value_kind) => (*value_kind, field_ty),
        FieldKind::Optional(value_kind, ty) | FieldKind::Repeated(value_kind, ty) => (*value_kind, *ty),
        FieldKind::Flatten | FieldKind::Skip => return Ok(None),
    };
    if !matches!(value_kind, ValueKind::Str | ValueKind::Cow) {
        return Ok(None);
    }
    let declared_lifetime = match (value_kind, ty) {
        (ValueKind::Str, Type::Reference(reference)) => reference.lifetime.as_ref(),
        (ValueKind::Cow, Type::Path(path)) => path.path.segments.last().and_then(|segment| {
            let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
                return None;
            };
            arguments.args.iter().find_map(|argument| match argument {
                GenericArgument::Lifetime(lifetime) => Some(lifetime),
                _ => None,
            })
        }),
        _ => None,
    };
    declared_lifetime
        .cloned()
        .map(Some)
        .ok_or_else(|| syn::Error::new(span, "borrowed query fields must declare an explicit lifetime"))
}

fn query_lifetime(fields: &[QueryField<'_>]) -> syn::Result<Option<syn::Lifetime>> {
    let mut selected = None;
    for field in fields {
        let Some(lifetime) = borrowed_lifetime(&field.kind, &field.field.ty, field.field.span())? else {
            continue;
        };
        if selected.as_ref().is_some_and(|selected| selected != &lifetime) {
            return Err(syn::Error::new(
                field.field.span(),
                "query data cannot be borrowed through more than one distinct lifetime",
            ));
        }
        selected = Some(lifetime);
    }
    Ok(selected)
}

fn standard_path(path: &syn::Path, name: &str, generic_types: &HashSet<String>) -> bool {
    let segments = path.segments.iter().map(|segment| segment.ident.to_string()).collect::<Vec<_>>();
    match segments.as_slice() {
        [single] => single == name && !generic_types.contains(single),
        [root, module, item] if item == name => match name {
            "Option" => (root == "core" || root == "std") && module == "option",
            "Vec" => (root == "alloc" || root == "std") && module == "vec",
            "String" => (root == "alloc" || root == "std") && module == "string",
            "Cow" => (root == "alloc" || root == "std") && module == "borrow",
            _ => false,
        },
        _ => false,
    }
}

fn parse_value(kind: ValueKind<'_>, runtime: &TokenStream, name: &str) -> TokenStream {
    match kind {
        ValueKind::Str => quote! { #runtime::query::parse_borrowed(&value, #name, pair_offset)? },
        ValueKind::Cow => quote! { #runtime::query::parse_cow(value) },
        ValueKind::String => quote! { #runtime::query::parse_owned(value) },
        ValueKind::Other(ty) => quote! { #runtime::query::parse_value::<#ty>(&value, #name, pair_offset)? },
    }
}

fn primitive_encoder(ty: &Type) -> Option<Ident> {
    const PRIMITIVES: &[&str] = &[
        "bool", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
    ];
    let Type::Path(path) = ty else { return None };
    if path.qself.is_some() || path.path.segments.len() != 1 {
        return None;
    }
    let name = path.path.segments[0].ident.to_string();
    PRIMITIVES.contains(&name.as_str()).then(|| format_ident!("pair_{name}"))
}

fn encode_other(ty: &Type, parameter: &str, encoded_parameter: &str, value: &TokenStream) -> TokenStream {
    if let Some(method) = primitive_encoder(ty) {
        quote! { encoder.#method(#parameter, #encoded_parameter, *#value)?; }
    } else {
        quote! { encoder.pair_display(#parameter, #encoded_parameter, #value)?; }
    }
}

fn encode_parameter(parameter: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(parameter.len());
    for &byte in parameter.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'*' | b'-' | b'.' | b'_') {
            encoded.push(char::from(byte));
        } else if byte == b' ' {
            encoded.push('+');
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[(byte >> 4) as usize]));
            encoded.push(char::from(HEX[(byte & 0x0F) as usize]));
        }
    }
    encoded
}

// Reversing the collision predicate makes uniqueness search non-terminating.
#[cfg_attr(test, mutants::skip)]
fn unique_lifetime_marker(parsed: &QueryInput<'_>) -> Ident {
    let mut candidate = "__routerama_query_lifetime".to_owned();
    while parsed.fields.iter().any(|field| field.ident == candidate.as_str()) {
        candidate.push('_');
    }
    format_ident!("{candidate}")
}

#[expect(
    clippy::too_many_lines,
    reason = "the decoder expansion assembles state, dispatch, flattening, and finishing fragments that share one parsed field model"
)]
pub(crate) fn expand_from_query(input: &DeriveInput) -> syn::Result<TokenStream> {
    let parsed = QueryInput::parse(input, true)?;
    let runtime = runtime_path();
    let name = &input.ident;
    let original_generics = &input.generics;
    let (_, type_generics, _) = original_generics.split_for_impl();
    let outer_type: Type = syn::parse_quote!(#name #type_generics);
    let (mut generated_generics, query_lifetime) = decoding_generics(original_generics, parsed.query_lifetime.as_ref());
    add_from_query_bounds(&mut generated_generics, &parsed, &runtime, &query_lifetime);
    let (impl_generics, _, generated_where_clause) = generated_generics.split_for_impl();
    let deny_unknown_fields = parsed.deny_unknown_fields;
    let flattened_types = parsed
        .fields
        .iter()
        .filter(|field| matches!(field.kind, FieldKind::Flatten))
        .map(|field| &field.field.ty)
        .collect::<Vec<_>>();
    let marker = unique_lifetime_marker(&parsed);
    let mut generated_names = identifier_names(input);
    let state_name = unique_generic_ident("__RouteramaQueryState", &mut generated_names);
    let flatten_decoders = parsed
        .fields
        .iter()
        .filter(|field| matches!(field.kind, FieldKind::Flatten))
        .enumerate()
        .map(|(index, field)| {
            let decoder = unique_generic_ident(&format!("__RouteramaFlattenDecoder{index}"), &mut generated_names);
            (decoder, &field.field.ty)
        })
        .collect::<Vec<_>>();
    let flatten_decoder_names = flatten_decoders.iter().map(|(decoder, _)| decoder).collect::<Vec<_>>();

    let mut state_declaration_generics = original_generics.clone();
    for parameter in &mut state_declaration_generics.params {
        match parameter {
            GenericParam::Type(parameter) => parameter.default = None,
            GenericParam::Const(parameter) => parameter.default = None,
            GenericParam::Lifetime(_) => {}
        }
    }
    replace_self_in_generics(&mut state_declaration_generics, &outer_type);
    let (_, _, state_where_clause) = state_declaration_generics.split_for_impl();
    let mut state_parameters = state_declaration_generics
        .params
        .iter()
        .map(|parameter| quote! { #parameter })
        .collect::<Vec<_>>();
    state_parameters.extend(flatten_decoder_names.iter().map(|decoder| quote! { #decoder }));
    let state_generics = generic_list(&state_parameters);

    let mut state_arguments = generic_arguments(original_generics);
    state_arguments.extend(flatten_decoder_names.iter().map(|decoder| quote! { #decoder }));
    let state_type_generics = generic_list(&state_arguments);

    let mut decoder_generics = generated_generics.clone();
    for decoder in &flatten_decoder_names {
        decoder_generics.params.push(syn::parse_quote!(#decoder));
    }
    for (decoder, ty) in &flatten_decoders {
        decoder_generics.make_where_clause().predicates.push(syn::parse_quote! {
            #decoder: #runtime::query::QueryDecoder<#query_lifetime, Output = #ty>
        });
    }
    replace_self_in_generics(&mut decoder_generics, &outer_type);
    let (decoder_impl_generics, _, decoder_where) = decoder_generics.split_for_impl();

    let mut flatten_index = 0;
    let state_fields = parsed.fields.iter().filter_map(|field| {
        let ident = field.ident;
        match &field.kind {
            FieldKind::Scalar(_) => {
                let ty = &field.field.ty;
                Some(quote! { #ident: ::core::option::Option<#ty> })
            }
            FieldKind::Optional(_, inner) => Some(quote! { #ident: ::core::option::Option<#inner> }),
            FieldKind::Repeated(_, inner) => Some(quote! { #ident: #runtime::query::Repeated<#inner> }),
            FieldKind::Flatten => {
                let decoder = flatten_decoder_names[flatten_index];
                flatten_index += 1;
                Some(quote! { #ident: #decoder })
            }
            FieldKind::Skip => None,
        }
    });
    let state_initializers = parsed.fields.iter().filter_map(|field| {
        let ident = field.ident;
        match &field.kind {
            FieldKind::Scalar(_) | FieldKind::Optional(_, _) => Some(quote! { #ident: ::core::option::Option::None }),
            FieldKind::Repeated(_, _) => Some(quote! { #ident: #runtime::query::Repeated::new() }),
            FieldKind::Flatten => {
                let ty = &field.field.ty;
                Some(quote! { #ident: <#ty as #runtime::query::DecodeFields<#query_lifetime>>::decoder() })
            }
            FieldKind::Skip => None,
        }
    });
    let flatten_idents = parsed
        .fields
        .iter()
        .filter(|field| matches!(field.kind, FieldKind::Flatten))
        .map(|field| field.ident)
        .collect::<Vec<_>>();

    let match_arms = parsed.fields.iter().filter_map(|field| {
        let ident = field.ident;
        let name = &field.name;
        let patterns = std::iter::once(name)
            .chain(field.aliases.iter())
            .map(|value| LitStr::new(value, field.field.span()));
        let body = match field.kind {
            FieldKind::Scalar(kind) | FieldKind::Optional(kind, _) => {
                let parse = parse_value(kind, &runtime, name);
                quote! {
                    if false #( || #runtime::query::QueryDecoder::claims_field(&self.#flatten_idents, key) )* {
                        return ::core::result::Result::Err(#runtime::query::Error::ambiguous(pair_offset));
                    }
                    if self.#ident.is_some() {
                        return ::core::result::Result::Err(#runtime::query::Error::duplicate(#name, pair_offset));
                    }
                    self.#ident = ::core::option::Option::Some(#parse);
                }
            }
            FieldKind::Repeated(kind, inner) => {
                let parse = parse_value(kind, &runtime, name);
                quote! {
                    if false #( || #runtime::query::QueryDecoder::claims_field(&self.#flatten_idents, key) )* {
                        return ::core::result::Result::Err(#runtime::query::Error::ambiguous(pair_offset));
                    }
                    let repeated_len = self.#ident.len();
                    // Generated state starts empty and grows only through this guard.
                    if repeated_len == limits.max_repeated_values {
                        return ::core::result::Result::Err(#runtime::query::Error::too_many_values(#name, pair_offset));
                    }
                    let value = #parse;
                    if repeated_len != 0 {
                        self.#ident.push(value);
                    } else {
                        let element_size = ::core::mem::size_of::<#inner>();
                        let initial_capacity = if element_size == 1 {
                            8
                        } else if element_size <= 1024 {
                            4
                        } else {
                            1
                        };
                        let mut values = #runtime::query::Repeated::with_capacity(initial_capacity);
                        values.push(value);
                        self.#ident = values;
                    }
                }
            }
            FieldKind::Flatten | FieldKind::Skip => return None,
        };
        Some(quote! { #( #patterns )|* => { #body ::core::result::Result::Ok(true) } })
    });
    let claim_arms = parsed.fields.iter().filter_map(|field| {
        if matches!(field.kind, FieldKind::Flatten | FieldKind::Skip) {
            return None;
        }
        let patterns = std::iter::once(&field.name)
            .chain(field.aliases.iter())
            .map(|value| LitStr::new(value, field.field.span()));
        Some(quote! { #( #patterns )|* => true })
    });
    let flatten_claims = parsed
        .fields
        .iter()
        .filter(|field| matches!(field.kind, FieldKind::Flatten))
        .enumerate()
        .map(|(index, field)| {
            let ident = field.ident;
            quote! {
                if #runtime::query::QueryDecoder::claims_field(&self.#ident, key) {
                    if __routerama_claimant.is_some() {
                        return ::core::result::Result::Err(
                            #runtime::query::Error::ambiguous(pair_offset),
                        );
                    }
                    __routerama_claimant = ::core::option::Option::Some(#index);
                }
            }
        });
    let flatten_dispatch = parsed
        .fields
        .iter()
        .filter(|field| matches!(field.kind, FieldKind::Flatten))
        .enumerate()
        .map(|(index, field)| {
            let ident = field.ident;
            quote! {
                ::core::option::Option::Some(#index) => #runtime::query::QueryDecoder::decode_field(
                    &mut self.#ident,
                    key,
                    value,
                    pair_offset,
                    limits,
                ),
            }
        });
    let finishes = parsed.fields.iter().map(|field| {
        let ident = field.ident;
        let field_name = &field.name;
        match &field.kind {
            FieldKind::Scalar(_) if field.attrs.default.is_some() => {
                quote! { #ident: self.#ident.unwrap_or_default() }
            }
            FieldKind::Scalar(_) => quote! {
                #ident: self.#ident.ok_or_else(|| #runtime::query::Error::missing(#field_name, end_offset))?
            },
            FieldKind::Optional(_, _) | FieldKind::Repeated(_, _) => quote! { #ident: self.#ident },
            FieldKind::Flatten => {
                quote! {
                    #ident: #runtime::query::QueryDecoder::finish(self.#ident, end_offset)?
                }
            }
            FieldKind::Skip => quote! { #ident: ::core::default::Default::default() },
        }
    });

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics #runtime::query::DecodeFields<#query_lifetime> for #name #type_generics #generated_where_clause {
            const DENY_UNKNOWN_FIELDS: bool = #deny_unknown_fields
                #( || <#flattened_types as #runtime::query::DecodeFields<#query_lifetime>>::DENY_UNKNOWN_FIELDS )*;

            fn decoder() -> impl #runtime::query::QueryDecoder<#query_lifetime, Output = Self> {
                struct #state_name #state_generics #state_where_clause {
                    #( #state_fields, )*
                    #marker: ::core::marker::PhantomData<fn() -> #name #type_generics>,
                }

                impl #decoder_impl_generics #runtime::query::QueryDecoder<#query_lifetime>
                    for #state_name #state_type_generics
                    #decoder_where
                {
                    type Output = #outer_type;

                    fn claims_field(&self, key: &str) -> bool {
                        match key {
                            #( #claim_arms, )*
                            _ => false #( || #runtime::query::QueryDecoder::claims_field(&self.#flatten_idents, key) )*,
                        }
                    }

                    #[expect(
                        clippy::inline_always,
                        reason = "Callgrind shows decoder inlining removes repeated Result ABI overhead"
                    )]
                    #[inline(always)]
                    fn decode_field(
                        &mut self,
                        key: &str,
                        value: #runtime::query::Decoded<#query_lifetime>,
                        pair_offset: usize,
                        limits: #runtime::query::QueryLimits,
                    ) -> ::core::result::Result<bool, #runtime::query::Error> {
                        match key {
                            #( #match_arms, )*
                            _ => {
                                let mut __routerama_claimant: ::core::option::Option<usize> =
                                    ::core::option::Option::None;
                                #( #flatten_claims )*
                                match __routerama_claimant {
                                    #( #flatten_dispatch )*
                                    ::core::option::Option::None => ::core::result::Result::Ok(false),
                                    _ => ::core::unreachable!(),
                                }
                            }
                        }
                    }

                    fn finish(
                        self,
                        end_offset: usize,
                    ) -> ::core::result::Result<Self::Output, #runtime::query::Error> {
                        ::core::result::Result::Ok(#name { #( #finishes, )* })
                    }
                }

                #state_name {
                    #( #state_initializers, )*
                    #marker: ::core::marker::PhantomData,
                }
            }
        }
    })
}

fn decoding_generics(original: &Generics, lifetime: Option<&syn::Lifetime>) -> (Generics, syn::Lifetime) {
    if let Some(lifetime) = lifetime {
        (original.clone(), lifetime.clone())
    } else {
        let used = lifetime_names(original);
        let mut name = "__routerama_q".to_owned();
        while used.contains(&name) {
            name.push('_');
        }
        let lifetime = syn::Lifetime::new(&format!("'{name}"), Span::call_site());
        let mut generics = original.clone();
        generics.params.insert(0, syn::parse_quote!(#lifetime));
        (generics, lifetime)
    }
}

fn lifetime_names(generics: &Generics) -> HashSet<String> {
    struct Collector {
        names: HashSet<String>,
    }

    impl<'ast> syn::visit::Visit<'ast> for Collector {
        fn visit_lifetime(&mut self, i: &'ast syn::Lifetime) {
            self.names.insert(i.ident.to_string());
        }
    }

    let mut collector = Collector { names: HashSet::new() };
    syn::visit::Visit::visit_generics(&mut collector, generics);
    collector.names
}

// Reversing the collision predicate makes uniqueness search non-terminating.
#[cfg_attr(test, mutants::skip)]
fn unique_generic_ident(base: &str, used: &mut HashSet<String>) -> Ident {
    let mut name = base.to_owned();
    while !used.insert(name.clone()) {
        name.push('_');
    }
    format_ident!("{name}")
}

fn identifier_names(input: &DeriveInput) -> HashSet<String> {
    struct Collector {
        names: HashSet<String>,
    }

    impl<'ast> syn::visit::Visit<'ast> for Collector {
        fn visit_ident(&mut self, i: &'ast Ident) {
            self.names.insert(i.to_string());
        }
    }

    let mut collector = Collector { names: HashSet::new() };
    syn::visit::Visit::visit_derive_input(&mut collector, input);
    collector.names
}

fn replace_self_in_generics(generics: &mut Generics, outer_type: &Type) {
    let Some(where_clause) = &mut generics.where_clause else {
        return;
    };
    for predicate in &mut where_clause.predicates {
        let tokens = replace_self_tokens(quote! { #predicate }, &quote! { #outer_type });
        *predicate = syn::parse2(tokens).expect("replacing `Self` with the derived type preserves a valid where predicate");
    }
}

fn replace_self_tokens(tokens: TokenStream, outer_type: &TokenStream) -> TokenStream {
    tokens
        .into_iter()
        .flat_map(|token| match token {
            TokenTree::Ident(ident) if ident == "Self" => outer_type.clone(),
            TokenTree::Group(group) => {
                let mut replaced = Group::new(group.delimiter(), replace_self_tokens(group.stream(), outer_type));
                replaced.set_span(group.span());
                TokenStream::from(TokenTree::Group(replaced))
            }
            token => TokenStream::from(token),
        })
        .collect()
}

fn generic_arguments(generics: &Generics) -> Vec<TokenStream> {
    generics
        .params
        .iter()
        .map(|parameter| match parameter {
            GenericParam::Lifetime(parameter) => {
                let lifetime = &parameter.lifetime;
                quote! { #lifetime }
            }
            GenericParam::Type(parameter) => {
                let ident = &parameter.ident;
                quote! { #ident }
            }
            GenericParam::Const(parameter) => {
                let ident = &parameter.ident;
                quote! { #ident }
            }
        })
        .collect()
}

fn add_from_query_bounds(generics: &mut Generics, parsed: &QueryInput<'_>, runtime: &TokenStream, lifetime: &syn::Lifetime) {
    let generic_parameters = generics
        .params
        .iter()
        .map(|parameter| match parameter {
            GenericParam::Lifetime(parameter) => parameter.lifetime.ident.to_string(),
            GenericParam::Type(parameter) => parameter.ident.to_string(),
            GenericParam::Const(parameter) => parameter.ident.to_string(),
        })
        .collect::<HashSet<_>>();
    for field in &parsed.fields {
        match &field.kind {
            FieldKind::Scalar(ValueKind::Other(ty))
            | FieldKind::Optional(ValueKind::Other(ty), _)
            | FieldKind::Repeated(ValueKind::Other(ty), _) => {
                generics
                    .make_where_clause()
                    .predicates
                    .push(syn::parse_quote!(#ty: ::core::str::FromStr));
            }
            FieldKind::Flatten => {
                let ty = &field.field.ty;
                generics
                    .make_where_clause()
                    .predicates
                    .push(syn::parse_quote!(#ty: #runtime::query::DecodeFields<#lifetime>));
            }
            FieldKind::Skip
            | FieldKind::Scalar(ValueKind::Str | ValueKind::Cow | ValueKind::String)
            | FieldKind::Optional(ValueKind::Str | ValueKind::Cow | ValueKind::String, _)
            | FieldKind::Repeated(ValueKind::Str | ValueKind::Cow | ValueKind::String, _) => {}
        }
        if (matches!(field.kind, FieldKind::Skip) || matches!(field.kind, FieldKind::Scalar(_) if field.attrs.default.is_some()))
            && type_depends_on_generic_parameter(&field.field.ty, &generic_parameters)
            && !has_unconditional_default(&field.field.ty, &generic_parameters)
        {
            let ty = &field.field.ty;
            generics
                .make_where_clause()
                .predicates
                .push(syn::parse_quote!(#ty: ::core::default::Default));
        }
    }
}

fn has_unconditional_default(ty: &Type, generic_types: &HashSet<String>) -> bool {
    let Type::Path(path) = ty else { return false };
    if ["Option", "String", "Vec"]
        .iter()
        .any(|name| standard_path(&path.path, name, generic_types))
    {
        return true;
    }

    let segments = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    match segments.as_slice() {
        [name] => name == "PhantomData" && !generic_types.contains(name),
        [root, module, name] => (root == "core" || root == "std") && module == "marker" && name == "PhantomData",
        _ => false,
    }
}

fn type_depends_on_generic_parameter(ty: &Type, parameters: &HashSet<String>) -> bool {
    struct Finder<'a> {
        parameters: &'a HashSet<String>,
        found: bool,
    }

    impl<'ast> syn::visit::Visit<'ast> for Finder<'_> {
        fn visit_path(&mut self, i: &'ast syn::Path) {
            if i.segments
                .first()
                .is_some_and(|segment| self.parameters.contains(&segment.ident.to_string()))
            {
                self.found = true;
                return;
            }
            syn::visit::visit_path(self, i);
        }

        fn visit_lifetime(&mut self, i: &'ast syn::Lifetime) {
            self.found |= self.parameters.contains(&i.ident.to_string());
        }
    }

    let mut finder = Finder { parameters, found: false };
    syn::visit::Visit::visit_type(&mut finder, ty);
    finder.found
}

fn generic_list(parameters: &[TokenStream]) -> TokenStream {
    if parameters.is_empty() {
        TokenStream::new()
    } else {
        quote! { <#(#parameters),*> }
    }
}

pub(crate) fn expand_to_query(input: &DeriveInput) -> syn::Result<TokenStream> {
    let parsed = QueryInput::parse(input, false)?;
    let runtime = runtime_path();
    let name = &input.ident;
    let mut generated_generics = input.generics.clone();
    add_to_query_bounds(&mut generated_generics, &parsed, &runtime);
    let (impl_generics, _, where_clause) = generated_generics.split_for_impl();
    let (_, type_generics, _) = input.generics.split_for_impl();
    let mut generated_names = identifier_names(input);
    let writer = unique_generic_ident("__RouteramaQueryWriter", &mut generated_names);
    let encoders = parsed.fields.iter().map(|field| {
        let ident = field.ident;
        let parameter = &field.name;
        let encoded_parameter = encode_parameter(parameter);
        match field.kind {
            FieldKind::Scalar(ValueKind::Str | ValueKind::Cow | ValueKind::String) => {
                quote! { encoder.pair_str(#parameter, #encoded_parameter, &self.#ident)?; }
            }
            FieldKind::Scalar(ValueKind::Other(ty)) => encode_other(ty, parameter, &encoded_parameter, &quote! { &self.#ident }),
            FieldKind::Optional(ValueKind::Str | ValueKind::Cow | ValueKind::String, _) => quote! {
                if let ::core::option::Option::Some(value) = &self.#ident {
                    encoder.pair_str(#parameter, #encoded_parameter, value)?;
                }
            },
            FieldKind::Optional(ValueKind::Other(ty), _) => {
                let encode = encode_other(ty, parameter, &encoded_parameter, &quote! { value });
                quote! {
                    if let ::core::option::Option::Some(value) = &self.#ident {
                        #encode
                    }
                }
            }
            FieldKind::Repeated(ValueKind::Str | ValueKind::Cow | ValueKind::String, _) => quote! {
                for value in &self.#ident {
                    encoder.pair_str(#parameter, #encoded_parameter, value)?;
                }
            },
            FieldKind::Repeated(ValueKind::Other(ty), _) => {
                let encode = encode_other(ty, parameter, &encoded_parameter, &quote! { value });
                quote! {
                    for value in &self.#ident {
                        #encode
                    }
                }
            }
            FieldKind::Flatten => quote! {
                #runtime::query::EncodeFields::encode_fields(&self.#ident, encoder)?;
            },
            FieldKind::Skip => TokenStream::new(),
        }
    });
    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics #runtime::query::EncodeFields for #name #type_generics #where_clause {
            fn encode_fields<#writer: ::core::fmt::Write>(
                &self,
                encoder: &mut #runtime::query::Encoder<'_, #writer>,
            ) -> ::core::result::Result<(), #runtime::query::Error> {
                #( #encoders )*
                ::core::result::Result::Ok(())
            }
        }
    })
}

fn add_to_query_bounds(generics: &mut Generics, parsed: &QueryInput<'_>, runtime: &TokenStream) {
    for field in &parsed.fields {
        match &field.kind {
            FieldKind::Scalar(ValueKind::Other(ty))
            | FieldKind::Optional(ValueKind::Other(ty), _)
            | FieldKind::Repeated(ValueKind::Other(ty), _) => {
                generics
                    .make_where_clause()
                    .predicates
                    .push(syn::parse_quote!(#ty: ::core::fmt::Display));
            }
            FieldKind::Flatten => {
                let ty = &field.field.ty;
                generics
                    .make_where_clause()
                    .predicates
                    .push(syn::parse_quote!(#ty: #runtime::query::EncodeFields));
            }
            FieldKind::Skip
            | FieldKind::Scalar(ValueKind::Str | ValueKind::Cow | ValueKind::String)
            | FieldKind::Optional(ValueKind::Str | ValueKind::Cow | ValueKind::String, _)
            | FieldKind::Repeated(ValueKind::Str | ValueKind::Cow | ValueKind::String, _) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;

    use quote::quote;
    use syn::parse_quote;

    use super::*;

    fn input(tokens: TokenStream) -> DeriveInput {
        syn::parse2(tokens).expect("valid derive input")
    }

    #[test]
    fn decoding_expansion_keeps_state_local_and_composes_flattening() {
        let expanded = expand_from_query(&input(quote! {
            #[query(rename_all = "camelCase", deny_unknown_fields)]
            struct Request<'a> {
                user_name: &'a str,
                #[query(alias = "n")] count: Option<u32>,
                tags: Vec<String>,
                #[query(default)] page: u32,
                #[query(flatten)] nested: Nested,
                #[query(skip)] cache: Cache,
            }
        }))
        .expect("expands");
        let file: syn::File = syn::parse2(expanded.clone()).expect("expansion is a valid Rust file");
        assert!(
            file.items
                .iter()
                .all(|item| !matches!(item, syn::Item::Struct(_) | syn::Item::Type(_)))
        );

        let expanded = expanded.to_string();
        assert!(!expanded.contains("__RequestQueryState"));
        assert!(expanded.contains("fn decoder"));
        assert!(expanded.contains("struct __RouteramaQueryState"));
        assert!(expanded.contains("QueryDecoder"));
        assert!(expanded.contains("\"userName\""));
        assert!(expanded.contains("\"n\""));
        assert!(expanded.contains("max_repeated_values"));
        assert!(expanded.contains("DENY_UNKNOWN_FIELDS"));
        assert!(expanded.contains("unwrap_or_default"));
        assert!(expanded.contains("DecodeFields"));
    }

    #[test]
    fn generic_lists_are_emitted_only_when_populated() {
        assert!(generic_list(&[]).is_empty());
        assert_eq!(generic_list(&[quote!('a), quote!(T)]).to_string(), "< 'a , T >");
    }

    #[test]
    fn encoding_uses_canonical_names_and_value_shapes() {
        let expanded = expand_to_query(&input(quote! {
            struct Request {
                #[query(rename = "q", alias = "query")] text: String,
                count: Option<u32>,
                tags: Vec<String>,
                #[query(flatten)] nested: Nested,
                #[query(skip)] ignored: bool,
            }
        }))
        .expect("expands")
        .to_string();
        assert!(expanded.contains("pair_str (\"q\""));
        assert!(!expanded.contains("\"query\""));
        assert!(expanded.contains("pair_u32 (\"count\""));
        assert!(expanded.contains("for value in & self . tags"));
        assert!(expanded.contains("EncodeFields :: encode_fields"));
        assert!(!expanded.contains("self . ignored"));
    }

    #[test]
    fn serde_structural_attributes_are_merged() {
        let derive_input = input(quote! {
            #[serde(rename_all = "kebab-case")]
            struct Request {
                #[serde(rename = "user", alias = "u", default, with = "codec", borrow)] user_name: String,
            }
        });
        let parsed = QueryInput::parse(&derive_input, true).expect("attributes parse");
        assert_eq!(parsed.fields[0].name, "user");
        assert_eq!(parsed.fields[0].aliases, ["u"]);
        assert!(parsed.fields[0].attrs.default.is_some());
    }

    #[test]
    fn conflicting_and_duplicate_names_are_rejected() {
        let conflict_input = input(quote! {
            struct Request {
                #[query(rename = "q")]
                #[serde(rename = "query")]
                value: String,
            }
        });
        let conflict = QueryInput::parse(&conflict_input, true).err().expect("conflict");
        assert!(conflict.to_string().contains("conflicting"));

        let duplicate_input = input(quote! {
            struct Request {
                #[query(alias = "other")] first: String,
                other: String,
            }
        });
        let duplicate = QueryInput::parse(&duplicate_input, true).err().expect("duplicate");
        assert!(duplicate.to_string().contains("duplicate query parameter"));

        let matching = input(quote! {
            struct Request {
                #[query(rename = "same", alias = "alias")]
                #[serde(rename = "same", alias = "alias")]
                value: String,
            }
        });
        let parsed = QueryInput::parse(&matching, true).expect("matching attributes merge");
        assert_eq!(parsed.fields[0].name, "same");
        assert_eq!(parsed.fields[0].aliases, ["alias"]);
    }

    #[test]
    fn unsupported_shapes_and_misplaced_attributes_are_rejected() {
        for item in [
            quote!(
                struct Tuple(String);
            ),
            quote!(
                enum Choice {
                    A,
                }
            ),
            quote!(
                struct Bad {
                    #[query(repeated)]
                    value: String,
                }
            ),
            quote!(
                struct Bad {
                    #[query(flatten, rename = "x")]
                    value: Nested,
                }
            ),
            quote!(
                struct Bad<'a> {
                    value: &'static str,
                    marker: &'a str,
                }
            ),
            quote!(
                #[query(rename = "bad")]
                struct Bad {
                    value: String,
                }
            ),
            quote!(
                struct Bad {
                    #[query(rename_all = "snake_case")]
                    value: String,
                }
            ),
            quote!(
                struct Bad {
                    #[query(skip, rename = "bad")]
                    value: String,
                }
            ),
        ] {
            assert!(QueryInput::parse(&input(item), true).is_err());
        }
    }

    #[test]
    fn default_is_accepted_on_optional_and_repeated_fields() {
        QueryInput::parse(
            &input(quote! {
                struct Accepted {
                    #[query(default)]
                    optional: Option<String>,
                    #[serde(default)]
                    repeated: Vec<String>,
                }
            }),
            true,
        )
        .expect("missing Option and Vec fields already use their default values");
    }

    #[test]
    fn obsolete_repeated_is_rejected_only_in_the_query_namespace() {
        let query = input(quote! {
            struct Rejected {
                #[query(repeated)]
                value: Vec<String>,
            }
        });
        assert!(matches!(
            QueryInput::parse(&query, true),
            Err(error) if error.to_string().contains("Vec<T> fields are repeated automatically")
        ));

        let serde = input(quote! {
            struct Accepted {
                #[serde(repeated)]
                value: Vec<String>,
            }
        });
        QueryInput::parse(&serde, true).expect("unrelated Serde attributes are ignored");
    }

    #[test]
    fn every_container_only_attribute_is_rejected_on_containers() {
        for attribute in [
            quote!(#[query(alias = "bad")]),
            quote!(#[query(default)]),
            quote!(#[query(flatten)]),
            quote!(#[query(skip)]),
        ] {
            assert!(
                QueryInput::parse(
                    &input(quote! {
                        #attribute
                        struct Bad {
                            value: String,
                        }
                    }),
                    true
                )
                .is_err()
            );
        }
    }

    #[test]
    fn incompatible_skip_and_flatten_combinations_are_rejected() {
        for attribute in [
            quote!(#[query(skip, flatten)]),
            quote!(#[query(skip, default)]),
            quote!(#[query(flatten, default)]),
        ] {
            assert!(
                QueryInput::parse(
                    &input(quote! {
                        struct Bad {
                            #attribute
                            value: String,
                        }
                    }),
                    true
                )
                .is_err()
            );
        }
    }

    #[test]
    fn unrelated_lifetimes_are_accepted_but_distinct_query_lifetimes_are_rejected() {
        QueryInput::parse(
            &input(quote! {
                struct Accepted<'a, 'b> {
                    #[query(skip)]
                    first: Marker<'a>,
                    #[query(skip)]
                    second: Marker<'b>,
                }
            }),
            true,
        )
        .expect("unrelated lifetimes are valid");

        let error = QueryInput::parse(
            &input(quote! {
                struct Bad<'a, 'b> {
                    first: &'a str,
                    second: &'b str,
                }
            }),
            true,
        )
        .err()
        .expect("distinct borrowing lifetimes are rejected");
        assert!(error.to_string().contains("more than one distinct lifetime"));
    }

    #[test]
    fn case_rules_cover_every_supported_form() {
        let literal = |value| LitStr::new(value, Span::call_site());
        assert_eq!(Case::parse(&literal("snake_case")).expect("snake").apply("twoWords"), "two_words");
        assert_eq!(Case::parse(&literal("kebab-case")).expect("kebab").apply("two_words"), "two-words");
        assert_eq!(
            Case::parse(&literal("SCREAMING_SNAKE_CASE"))
                .expect("screaming snake")
                .apply("twoWords"),
            "TWO_WORDS"
        );
        assert!(Case::parse(&literal("PascalCase")).is_err());
        assert_eq!(Case::Camel.apply(""), "");
    }

    #[test]
    fn attribute_parser_rejects_query_typos_and_duplicates() {
        for item in [
            quote!(
                #[query(unknown)]
                struct Bad {
                    value: String,
                }
            ),
            quote!(
                struct Bad {
                    #[query(default, default)]
                    value: String,
                }
            ),
        ] {
            assert!(QueryInput::parse(&input(item), true).is_err());
        }

        let serde_input = input(quote! {
            #[serde(bound(serialize = "Self: Sized"), serialize_with = "serialize")]
            struct Accepted {
                value: String,
            }
        });
        QueryInput::parse(&serde_input, true).expect("unrelated Serde attributes are ignored");
    }

    #[test]
    fn type_shape_helpers_reject_near_misses() {
        let generic_types = HashSet::new();
        let reference: Type = parse_quote!(&str);
        let string: Type = parse_quote!(String);
        let bare_option: Type = parse_quote!(Option);
        let multi_option: Type = parse_quote!(Option<u8, u16>);
        let lifetime_option: Type = parse_quote!(Option<'a>);
        assert!(container_inner(&reference, "Option", &generic_types).is_none());
        assert!(container_inner(&string, "Option", &generic_types).is_none());
        assert!(container_inner(&bare_option, "Option", &generic_types).is_none());
        assert!(container_inner(&multi_option, "Option", &generic_types).is_none());
        assert!(container_inner(&lifetime_option, "Option", &generic_types).is_none());

        let empty_path = Type::Path(syn::TypePath {
            qself: None,
            path: syn::Path {
                leading_colon: None,
                segments: syn::punctuated::Punctuated::default(),
            },
        });
        for ty in [
            reference,
            empty_path,
            parse_quote!(Cow),
            parse_quote!(Cow<String>),
            parse_quote!(Cow<'a, String>),
        ] {
            assert!(!is_cow_str(&ty, &generic_types));
        }

        for ty in [
            parse_quote!(&u8),
            parse_quote!(std::primitive::u8),
            parse_quote!(<T as Trait>::Value),
        ] {
            assert!(primitive_encoder(&ty).is_none());
        }

        let cow: Type = parse_quote!(Cow<'a, str>);
        assert!(is_cow_str(&cow, &generic_types));
        assert!(matches!(value_kind(&cow, &generic_types), ValueKind::Cow));

        let custom_option: Type = parse_quote!(custom::Option<u32>);
        assert!(container_inner(&custom_option, "Option", &generic_types).is_none());
        let custom_string: Type = parse_quote!(custom::String);
        assert!(matches!(value_kind(&custom_string, &generic_types), ValueKind::Other(_)));
        assert!(standard_path(&parse_quote!(alloc::borrow::Cow), "Cow", &generic_types));
        assert!(!standard_path(&parse_quote!(custom::module::Other), "Other", &generic_types));
        assert!(has_unconditional_default(&parse_quote!(Option<u32>), &generic_types));
        assert!(!has_unconditional_default(&parse_quote!(custom::Marker), &generic_types));

        let generic_types = HashSet::from(["String".to_owned()]);
        let generic_string: Type = parse_quote!(String);
        assert!(matches!(value_kind(&generic_string, &generic_types), ValueKind::Other(_)));

        let qualified_primitive: Type = parse_quote!(u8::Value);
        assert!(primitive_encoder(&qualified_primitive).is_none());
        let qualified_self = Type::Path(syn::TypePath {
            qself: Some(syn::QSelf {
                lt_token: syn::token::Lt::default(),
                ty: Box::new(parse_quote!(u8)),
                position: 0,
                as_token: None,
                gt_token: syn::token::Gt::default(),
            }),
            path: parse_quote!(u8),
        });
        assert!(primitive_encoder(&qualified_self).is_none());
    }

    #[test]
    fn standard_paths_require_exact_roots_modules_and_items() {
        let generic_types = HashSet::new();
        for (path, name) in [
            (parse_quote!(core::option::Option), "Option"),
            (parse_quote!(std::option::Option), "Option"),
            (parse_quote!(alloc::vec::Vec), "Vec"),
            (parse_quote!(std::vec::Vec), "Vec"),
            (parse_quote!(alloc::string::String), "String"),
            (parse_quote!(std::string::String), "String"),
            (parse_quote!(alloc::borrow::Cow), "Cow"),
            (parse_quote!(std::borrow::Cow), "Cow"),
        ] {
            assert!(standard_path(&path, name, &generic_types), "canonical path must be standard {name}");
        }
        for (path, name) in [
            (parse_quote!(custom::option::Option), "Option"),
            (parse_quote!(core::wrong::Option), "Option"),
            (parse_quote!(core::option::Different), "Option"),
            (parse_quote!(custom::vec::Vec), "Vec"),
            (parse_quote!(alloc::wrong::Vec), "Vec"),
            (parse_quote!(alloc::vec::Different), "Vec"),
            (parse_quote!(custom::string::String), "String"),
            (parse_quote!(alloc::wrong::String), "String"),
            (parse_quote!(alloc::string::Different), "String"),
            (parse_quote!(custom::borrow::Cow), "Cow"),
            (parse_quote!(alloc::wrong::Cow), "Cow"),
            (parse_quote!(alloc::borrow::Different), "Cow"),
        ] {
            assert!(!standard_path(&path, name, &generic_types), "near miss must not be standard {name}");
        }
    }

    #[test]
    fn lifetime_name_collection_includes_nested_binders_and_references() {
        let item = input(quote! {
            struct Probe<'outer, T>
            where
                T: for<'__routerama_q> Trait<&'__routerama_q (), &'nested ()>,
            {
                marker: &'outer T,
            }
        });
        assert_eq!(
            lifetime_names(&item.generics),
            HashSet::from(["outer".to_owned(), "__routerama_q".to_owned(), "nested".to_owned(),])
        );
    }

    #[test]
    fn identifier_collection_visits_the_complete_derive_input() {
        let item = input(quote! {
            struct IdentifierProbe<Generic: Bound> {
                first_field: Outer<Generic>,
                second_field: Inner,
            }
        });
        assert_eq!(
            identifier_names(&item),
            HashSet::from([
                "IdentifierProbe".to_owned(),
                "Generic".to_owned(),
                "Bound".to_owned(),
                "first_field".to_owned(),
                "Outer".to_owned(),
                "second_field".to_owned(),
                "Inner".to_owned(),
            ])
        );
    }

    #[test]
    fn self_replacement_is_recursive_and_limited_to_self() {
        fn compact(tokens: &TokenStream) -> String {
            tokens.to_string().split_whitespace().collect()
        }

        let outer = quote!(Outer<T>);
        let replaced: Type =
            syn::parse2(replace_self_tokens(quote!((Self, Nested<Self>, Selfish)), &outer)).expect("replacement preserves a valid type");
        assert_eq!(compact(&quote!(#replaced)), compact(&quote!((Outer<T>, Nested<Outer<T>>, Selfish))));

        let item = input(quote! {
            struct Probe<T>
            where
                Self: Trait<(Self, Nested<Self>)>,
            {
                value: T,
            }
        });
        let mut generics = item.generics;
        replace_self_in_generics(&mut generics, &parse_quote!(Outer<T>));
        let expected = input(quote! {
            struct Expected<T>
            where
                Outer<T>: Trait<(Outer<T>, Nested<Outer<T>>)>,
            {
                value: T,
            }
        })
        .generics;
        let actual_where = generics.where_clause;
        let expected_where = expected.where_clause;
        assert_eq!(compact(&quote!(#actual_where)), compact(&quote!(#expected_where)));
    }

    #[test]
    fn default_bounds_are_added_only_when_generation_requires_them() {
        fn bounds(tokens: TokenStream) -> String {
            let item = input(tokens);
            let parsed = QueryInput::parse(&item, true).expect("valid query input");
            let mut generics = item.generics.clone();
            add_from_query_bounds(&mut generics, &parsed, &quote!(routerama), &parse_quote!('__routerama_q));
            let where_clause = generics.where_clause;
            quote!(#where_clause).to_string()
        }

        assert!(
            bounds(quote! {
                struct Skipped<T> {
                    #[query(skip)]
                    value: T,
                }
            })
            .contains("default :: Default")
        );
        assert!(
            bounds(quote! {
                struct Defaulted<T> {
                    #[query(default)]
                    value: T,
                }
            })
            .contains("default :: Default")
        );
        assert!(
            !bounds(quote! {
                struct Parsed<T> {
                    value: T,
                }
            })
            .contains("default :: Default")
        );
        assert!(
            !bounds(quote! {
                struct Concrete {
                    #[query(skip)]
                    value: ConcreteValue,
                }
            })
            .contains("default :: Default")
        );
        assert!(
            !bounds(quote! {
                struct Unconditional<T> {
                    #[query(skip)]
                    value: Option<T>,
                }
            })
            .contains("default :: Default")
        );
    }

    #[test]
    fn unconditional_defaults_recognize_only_standard_types() {
        let generic_types = HashSet::from(["T".to_owned(), "PhantomData".to_owned()]);
        for ty in [
            parse_quote!(Option<T>),
            parse_quote!(alloc::vec::Vec<T>),
            parse_quote!(std::string::String),
            parse_quote!(core::marker::PhantomData<T>),
            parse_quote!(std::marker::PhantomData<T>),
        ] {
            assert!(has_unconditional_default(&ty, &generic_types));
        }

        let no_generics = HashSet::new();
        assert!(has_unconditional_default(&parse_quote!(PhantomData<T>), &no_generics));
        for ty in [
            parse_quote!(PhantomData<T>),
            parse_quote!(custom::marker::PhantomData<T>),
            parse_quote!(core::wrong::PhantomData<T>),
            parse_quote!(custom::marker::Different<T>),
            parse_quote!(Custom<T>),
            parse_quote!(&'static str),
        ] {
            assert!(!has_unconditional_default(&ty, &generic_types));
        }
    }

    #[test]
    fn generic_dependency_detects_types_consts_and_lifetimes() {
        let parameters = HashSet::from(["T".to_owned(), "N".to_owned(), "a".to_owned()]);
        for ty in [
            parse_quote!(T),
            parse_quote!(Wrapper<T>),
            parse_quote!([u8; N]),
            parse_quote!(&'a str),
        ] {
            assert!(type_depends_on_generic_parameter(&ty, &parameters));
        }
        for ty in [parse_quote!(u32), parse_quote!(Wrapper<u32>), parse_quote!(&'static str)] {
            assert!(!type_depends_on_generic_parameter(&ty, &parameters));
        }
    }

    #[test]
    fn borrowed_validation_rejects_malformed_cow_shapes() {
        for ty in [parse_quote!(Cow), parse_quote!(Cow<String>), parse_quote!(&str)] {
            let malformed = borrowed_lifetime(&FieldKind::Scalar(ValueKind::Cow), &ty, Span::call_site()).is_err();
            assert!(malformed, "malformed Cow must be rejected");
        }

        let cow: Type = parse_quote!(Cow<'a, str>);
        let lifetime = borrowed_lifetime(&FieldKind::Scalar(ValueKind::Cow), &cow, Span::call_site())
            .expect("valid Cow is accepted")
            .expect("Cow borrows");
        assert_eq!(lifetime.ident, "a");
    }

    #[test]
    fn value_parsing_emits_each_specialized_path() {
        let runtime = quote!(routerama);
        let other: Type = parse_quote!(u32);
        let outputs = [
            parse_value(ValueKind::Str, &runtime, "value").to_string(),
            parse_value(ValueKind::Cow, &runtime, "value").to_string(),
            parse_value(ValueKind::String, &runtime, "value").to_string(),
            parse_value(ValueKind::Other(&other), &runtime, "value").to_string(),
        ];
        assert!(outputs[0].contains("parse_borrowed"));
        assert!(outputs[1].contains("parse_cow"));
        assert!(outputs[2].contains("parse_owned"));
        assert!(outputs[3].contains("parse_value"));
    }

    #[test]
    fn expansions_cover_encoded_names_and_remaining_field_shapes() {
        let encoded = expand_to_query(&input(quote! {
            struct Encoded {
                #[query(rename = "a b/%")]
                value: Option<String>,
                page: u32,
                score: f32,
                numbers: Vec<u32>,
            }
        }))
        .expect("encodes")
        .to_string();
        assert!(encoded.contains("a+b%2F%25"));
        assert!(encoded.contains("pair_str"));
        assert!(encoded.contains("pair_u32"));
        assert!(encoded.contains("pair_display"));

        let decoded = expand_from_query(&input(quote! {
            struct Outer {
                first: First,
                #[query(flatten)]
                second: Second,
                #[query(flatten)]
                third: Third,
            }
        }))
        .expect("decodes")
        .to_string();
        assert!(!decoded.contains("value . clone"));
        assert!(decoded.contains("__routerama_q"));
        assert!(decoded.contains("second : __RouteramaFlattenDecoder0"));
        assert!(decoded.contains("third : __RouteramaFlattenDecoder1"));
        assert!(decoded.contains("claims_field (& self . second , key)"));
        assert!(decoded.contains("self . second , key , value ,"));
        assert!(decoded.contains("self . third , key , value ,"));

        let defaults = expand_from_query(&input(quote! {
            struct Defaults {
                required: String,
                #[query(default)]
                page: u32,
            }
        }))
        .expect("decodes defaults")
        .to_string();
        assert_eq!(defaults.matches("unwrap_or_default").count(), 1);
        assert!(defaults.contains("Error :: missing (\"required\""));
    }

    #[test]
    fn lifetime_marker_avoids_field_name_collisions() {
        let expanded = expand_from_query(&input(quote! {
            struct Borrowed<'a> {
                __routerama_query_lifetime: &'a str,
            }
        }))
        .expect("expands")
        .to_string();
        assert!(expanded.contains("__routerama_query_lifetime_"));
    }
}
