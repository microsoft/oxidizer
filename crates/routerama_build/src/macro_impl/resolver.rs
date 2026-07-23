// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementation of `#[resolver]`.
//!
//! A route enum can contain:
//!
//! - **static** variants carry `#[route(METHOD, "path")]`; their paths are known
//!   at compile time and lowered by [`Generator`]. Their fields may borrow the
//!   path (`&'p str`).
//! - **dynamic** variants carry no `#[route]`; their paths are registered at run
//!   time through the generated builder. Their fields must be owned.
//!
//! The generated resolver checks static routes before dynamic routes.

use alloc::format;
use alloc::string::{String, ToString as _};
use alloc::vec::Vec;

use http_path_template::Segment;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{ToTokens as _, quote};
use syn::spanned::Spanned as _;
use syn::{Error, Fields, ItemEnum};

use super::field::{FieldKind, field_kind, is_capture_cow, is_capture_str_reference, is_option, is_str_reference, uses_capture_lifetime};
use super::{declared_fields, has_capture_lifetime, has_route_attr, routes_for_variant};
use crate::{Generator, Route};

/// A classified field of a resolver variant.
struct TypedField {
    ident: Ident,
    kind: FieldKind,
}

pub(crate) fn runtime_path() -> TokenStream2 {
    runtime_path_for(crate_name("routerama").ok())
}

fn runtime_path_for(found: Option<FoundCrate>) -> TokenStream2 {
    match found {
        Some(FoundCrate::Name(name)) => {
            let name = name.replace('-', "_");
            let ident = syn::parse_str::<Ident>(&name)
                .or_else(|_| syn::parse_str::<Ident>(&format!("r#{name}")))
                .expect("Cargo dependency aliases are valid Rust identifiers, possibly requiring raw syntax");
            quote! { ::#ident }
        }
        Some(FoundCrate::Itself) | None => quote! { ::routerama },
    }
}

/// A resolver variant: its name and (possibly empty) classified fields.
struct TypedVariant {
    ident: Ident,
    fields: Vec<TypedField>,
}

#[cfg(test)]
pub(crate) fn expand(item: ItemEnum) -> syn::Result<TokenStream2> {
    expand_named(item, None)
}

#[expect(
    clippy::too_many_lines,
    reason = "the codegen assembles many independent token fragments in one place; splitting it would obscure the static/dynamic branching it orchestrates"
)]
pub(crate) fn expand_named(mut item: ItemEnum, explicit_resolver_name: Option<Ident>) -> syn::Result<TokenStream2> {
    let runtime = runtime_path();
    let has_lifetime = has_capture_lifetime(&item.generics)?;
    if item.ident.to_string().starts_with("r#") {
        return Err(Error::new(
            item.ident.span(),
            "`#[resolver]` does not support a raw identifier as the route enum name",
        ));
    }
    for variant in &item.variants {
        if variant.ident.to_string().starts_with("r#") {
            return Err(Error::new(
                variant.ident.span(),
                "`#[resolver]` does not support raw route variant identifiers",
            ));
        }
        if let Some(attribute) = variant
            .attrs
            .iter()
            .find(|attribute| attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr"))
        {
            return Err(Error::new(
                attribute.span(),
                "`#[resolver]` does not support conditionally compiled variants because every generated router item must have the same route set",
            ));
        }
        for field in &variant.fields {
            if let Some(attribute) = field
                .attrs
                .iter()
                .find(|attribute| attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr"))
            {
                return Err(Error::new(
                    attribute.span(),
                    "`#[resolver]` does not support conditionally compiled fields because generated extractors must match the route enum",
                ));
            }
        }
    }

    let mut static_routes: Vec<Route> = Vec::new();
    let mut static_variants: Vec<TypedVariant> = Vec::new();
    let mut dynamic_variants: Vec<TypedVariant> = Vec::new();
    for variant in &item.variants {
        if has_route_attr(variant) {
            static_routes.extend(routes_for_variant(variant)?);
            static_variants.push(classify_static(variant)?);
        } else {
            dynamic_variants.push(classify_dynamic(variant)?);
        }
    }
    let mut generated_dynamic_names: Vec<(String, Ident)> = Vec::new();
    for variant in &dynamic_variants {
        let generated = to_snake_case(&variant.ident.to_string());
        if let Some((_, previous)) = generated_dynamic_names.iter().find(|(name, _)| *name == generated) {
            return Err(Error::new(
                variant.ident.span(),
                format!(
                    "dynamic variants `{previous}` and `{}` both generate `add_{generated}`; rename one variant",
                    variant.ident
                ),
            ));
        }
        generated_dynamic_names.push((generated, variant.ident.clone()));
    }
    let enum_name = item.ident.clone();
    let resolver_name = explicit_resolver_name.unwrap_or_else(|| Ident::new(&format!("{enum_name}Resolver"), enum_name.span()));
    if resolver_name == enum_name {
        return Err(Error::new(
            resolver_name.span(),
            "the resolver type name must differ from the route enum name",
        ));
    }
    let builder_name = Ident::new(&format!("{resolver_name}Builder"), resolver_name.span());
    let schema_ty = if has_lifetime {
        quote! { #enum_name<'static> }
    } else {
        quote! { #enum_name }
    };
    let typed_ty = if has_lifetime {
        quote! { #enum_name<'p> }
    } else {
        quote! { #enum_name }
    };
    let extractor_ty = quote! {
        fn(&#runtime::__rt::Captures<'_, '_, '_>)
            -> ::core::result::Result<#schema_ty, #runtime::ResolveError<'static>>
    };
    let visibility = item.vis.to_token_stream();

    let has_static = !static_routes.is_empty();
    let has_dynamic = !dynamic_variants.is_empty();
    let associated_function = if has_dynamic { "builder" } else { "resolver" };
    for variant in &item.variants {
        let name = variant.ident.to_string();
        if name == associated_function {
            return Err(Error::new(
                variant.ident.span(),
                format!("route variant `{name}` collides with a generated associated function; rename the route"),
            ));
        }
    }
    let builder_method_doc = format!(
        "Creates a [`{builder_name}`] for this route enum.\n\nThe builder compiles all run-time route registrations and returns a resolver from its `build` method."
    );
    let resolver_method_doc =
        format!("Constructs the zero-sized [`{resolver_name}`].\n\nAll routes are compiled statically, so construction is infallible.");
    let dynamic_builder_doc = format!(
        "Builds a resolver for [`{enum_name}`].\n\nRegister every dynamic variant with its generated `add_<variant>` method, then call [`build`](Self::build). Static routes are already compiled into the resolver and need no registration."
    );
    let dynamic_build_doc = format!(
        "Validates the dynamic registrations and builds a [`{resolver_name}`].\n\n# Errors\n\nReturns `routerama::ConfigurationError` containing every invalid or missing dynamic route registration."
    );
    let resolver_type_doc = format!("The generated resolver for [`{enum_name}`].");
    let (static_any_verb, static_has_captures) = static_routes.iter().fold((false, false), |state, route| {
        let template = route.template();
        (
            state.0 || template.verb().is_some(),
            state.1
                || template
                    .segments()
                    .iter()
                    .any(|segment| matches!(segment, Segment::Variable(_) | Segment::Affix { .. })),
        )
    });

    let raw_enum = Ident::new(&format!("__{enum_name}Raw"), enum_name.span());
    let static_resolve_body = if has_static {
        let mut generator = Generator::new(raw_enum.to_string(), false);
        generator.full_api(false);
        generator.runtime_path(quote! { #runtime::codegen_helpers });
        generator.add_all(static_routes);
        let raw_impls = generator.generate();

        let raw_ty = if static_has_captures {
            quote! { #raw_enum<'p> }
        } else {
            quote! { #raw_enum }
        };
        let arms = static_variants
            .iter()
            .map(|variant| convert_arm(variant, &raw_enum, &enum_name, &runtime));
        quote! {
            #raw_impls

            fn __convert_static<'p>(__raw: #raw_ty) -> ::core::result::Result<#typed_ty, #runtime::ResolveError<'p>> {
                ::core::result::Result::Ok(match __raw { #(#arms)* })
            }
            match #raw_enum::__resolve_checked(__method, __path) {
                ::core::result::Result::Ok(::core::option::Option::Some(__raw)) => __convert_static(__raw),
                ::core::result::Result::Ok(::core::option::Option::None) => {
                    ::core::result::Result::Err(#runtime::ResolveError::NotFound(__path))
                }
                ::core::result::Result::Err(_) => {
                    ::core::result::Result::Err(#runtime::ResolveError::InvalidPath(__path))
                }
            }
        }
    } else {
        quote! { ::core::result::Result::Err(#runtime::ResolveError::NotFound(__path)) }
    };

    let add_methods: Vec<TokenStream2> = dynamic_variants
        .iter()
        .map(|variant| add_method(variant, &enum_name, &schema_ty, &extractor_ty, &visibility, &runtime))
        .collect();
    let dynamic_conversion_arms: Vec<TokenStream2> = dynamic_variants
        .iter()
        .map(|variant| dynamic_conversion_arm(variant, &enum_name))
        .collect();
    let dynamic_unreachable_arm = has_static.then(|| {
        quote! {
            _ => ::core::unreachable!(
                "dynamic extractors only construct dynamic route variants"
            ),
        }
    });
    let seen_decls: Vec<TokenStream2> = dynamic_variants
        .iter()
        .map(|variant| {
            let field = seen_ident(&variant.ident);
            quote! { #field: bool }
        })
        .collect();
    let seen_inits: Vec<TokenStream2> = dynamic_variants
        .iter()
        .map(|variant| {
            let field = seen_ident(&variant.ident);
            quote! { #field: false }
        })
        .collect();
    let requires: Vec<TokenStream2> = dynamic_variants
        .iter()
        .map(|variant| {
            let field = seen_ident(&variant.ident);
            let add = format!("add_{}", to_snake_case(&variant.ident.to_string()));
            let name = variant.ident.to_string();
            quote! { __builder.require(self.#field, #add, #name); }
        })
        .collect();

    let (state_ty, dynamic_resolve_fn) = if has_dynamic {
        (
            quote! {
                #runtime::__rt::RawResolver<#runtime::__rt::DynRoute<#extractor_ty>>
            },
            quote! {
                fn __convert_dynamic<'p>(__route: #schema_ty) -> #typed_ty {
                    match __route {
                        #( #dynamic_conversion_arms )*
                        #dynamic_unreachable_arm
                    }
                }

                fn __dynamic_resolve<'p>(
                    __dyn: &#runtime::__rt::RawResolver<#runtime::__rt::DynRoute<#extractor_ty>>,
                    __method: &str,
                    __path: &'p str,
                ) -> ::core::result::Result<#typed_ty, #runtime::ResolveError<'p>> {
                    match #runtime::__rt::RawResolver::resolve_scanned_checked(
                        __dyn,
                        __method,
                        __path,
                        |__leaf, __route, __scanned| {
                            let __caps = #runtime::__rt::Captures::new(
                                __leaf,
                                __scanned,
                                #runtime::__rt::DynRoute::capture_order(__route),
                            );
                            match (*#runtime::__rt::DynRoute::extractor(__route))(&__caps) {
                                ::core::result::Result::Ok(__r) => {
                                    ::core::result::Result::Ok(__convert_dynamic(__r))
                                }
                                ::core::result::Result::Err(__err) => {
                                    ::core::result::Result::Err(__err)
                                }
                            }
                        },
                    ) {
                        ::core::result::Result::Ok(::core::option::Option::Some(__result)) => {
                            __result
                        }
                        ::core::result::Result::Ok(::core::option::Option::None) => {
                            ::core::result::Result::Err(#runtime::ResolveError::NotFound(__path))
                        }
                        ::core::result::Result::Err(_) => {
                            ::core::result::Result::Err(#runtime::ResolveError::InvalidPath(__path))
                        }
                    }
                }
            },
        )
    } else {
        (quote! { () }, quote! {})
    };
    let static_resolve_fn = if has_static || !has_dynamic {
        quote! {
            fn __static_resolve<'p>(
                __method: &str,
                __path: &'p str,
            ) -> ::core::result::Result<#typed_ty, #runtime::ResolveError<'p>> {
                #static_resolve_body
            }
        }
    } else {
        quote! {}
    };

    let builder_def = if has_dynamic {
        quote! {
            #[doc = #dynamic_builder_doc]
            #[derive(Debug)]
            #visibility struct #builder_name {
                __dyn: #runtime::__rt::DynBuilder<#extractor_ty>,
                #( #seen_decls, )*
            }

            #[automatically_derived]
            impl #builder_name {
                #( #add_methods )*

                #[doc = #dynamic_build_doc]
                #visibility fn build(
                    self,
                ) -> ::core::result::Result<#resolver_name, #runtime::ConfigurationError> {
                    let mut __builder = self.__dyn;
                    #( #requires )*
                    let mut __resolver = #runtime::__rt::DynBuilder::finish(__builder)?;
                    // All routes must apply the same verb-splitting rule.
                    #runtime::__rt::RawResolver::force_verb_split(&mut __resolver, #static_any_verb);
                    ::core::result::Result::Ok(#resolver_name { __state: __resolver })
                }
            }
        }
    } else {
        quote! {}
    };

    let resolve_body = if has_dynamic && has_static && !static_any_verb {
        // A dynamic `:verb` route must not be consumed as a static capture.
        quote! {
            let __matched = if #runtime::__rt::RawResolver::splits_verbs(__state)
                && #runtime::codegen_helpers::split_verb(__path).1.is_some()
            {
                ::core::result::Result::Err(#runtime::ResolveError::NotFound(__path))
            } else {
                __static_resolve(__method, __path)
            };
            match __matched {
                ::core::result::Result::Err(#runtime::ResolveError::NotFound(_)) => {
                    __dynamic_resolve(__state, __method, __path)
                }
                __matched => __matched,
            }
        }
    } else if has_dynamic && has_static {
        quote! {
            match __static_resolve(__method, __path) {
                ::core::result::Result::Err(#runtime::ResolveError::NotFound(_)) => {
                    __dynamic_resolve(__state, __method, __path)
                }
                __matched => __matched,
            }
        }
    } else if has_dynamic {
        quote! { __dynamic_resolve(__state, __method, __path) }
    } else {
        quote! { __static_resolve(__method, __path) }
    };
    let enum_inherent = if has_dynamic {
        quote! {
            #[automatically_derived]
            impl #schema_ty {
                #[doc = #builder_method_doc]
                #[must_use]
                #visibility fn builder() -> #builder_name {
                    #builder_name {
                        __dyn: #runtime::__rt::DynBuilder::new(),
                        #( #seen_inits, )*
                    }
                }
            }
        }
    } else {
        quote! {
            #[automatically_derived]
            impl #schema_ty {
                #[doc = #resolver_method_doc]
                #[must_use]
                #visibility fn resolver() -> #resolver_name {
                    #resolver_name { __state: () }
                }
            }
        }
    };

    let resolver_def = quote! {
        #[doc = #resolver_type_doc]
        #[derive(Debug)]
        #visibility struct #resolver_name {
            __state: #state_ty,
        }

        #[automatically_derived]
        impl #resolver_name {
            /// Resolves an HTTP method and path into the corresponding route enum.
            ///
            /// # Errors
            ///
            /// Returns `routerama::ResolveError` when no route matches or a
            /// matched route's capture cannot be decoded or converted.
            #[inline]
            #visibility fn resolve<'p, P>(
                &self,
                __method: impl ::core::convert::AsRef<str>,
                __path: &'p P,
            ) -> ::core::result::Result<#typed_ty, #runtime::ResolveError<'p>>
            where
                P: ::core::convert::AsRef<str> + ?Sized,
            {
                #runtime::Resolver::resolve(self, __method, __path)
            }
        }

        #[automatically_derived]
        impl #runtime::Resolver for #resolver_name {
            type Route<'p> = #typed_ty;

            #[inline]
            fn resolve<'p, P>(
                &self,
                __method: impl ::core::convert::AsRef<str>,
                __path: &'p P,
            ) -> ::core::result::Result<#typed_ty, #runtime::ResolveError<'p>>
            where
                P: ::core::convert::AsRef<str> + ?Sized,
            {
                let __state = &self.__state;
                let __method = __method.as_ref();
                let __path = __path.as_ref();
                #static_resolve_fn
                #dynamic_resolve_fn
                #resolve_body
            }
        }
    };

    for variant in &mut item.variants {
        variant.attrs.retain(|attr| !attr.path().is_ident("route"));
    }
    Ok(quote! {
        #item
        #resolver_def
        #enum_inherent
        #builder_def
    })
}

/// Classifies a static (`#[route]`) variant's named fields, rejecting `Option`.
/// Field/capture agreement was already checked by [`routes_for_variant`].
fn classify_static(variant: &syn::Variant) -> syn::Result<TypedVariant> {
    let fields = match &variant.fields {
        Fields::Named(named) => {
            let mut fields = Vec::with_capacity(named.named.len());
            for field in &named.named {
                let ident = field.ident.clone().expect("named field has an identifier");
                if is_option(&field.ty) {
                    return Err(Error::new(
                        field.ty.span(),
                        "`#[resolver]` fields cannot be `Option`: a matched route always has its captures",
                    ));
                }
                let kind = field_kind(&field.ty);
                if is_str_reference(&field.ty) && !is_capture_str_reference(&field.ty) {
                    return Err(Error::new(
                        field.ty.span(),
                        "a borrowed string capture must be written as `&'p str`",
                    ));
                }
                if matches!(kind, FieldKind::Cow) && !is_capture_cow(&field.ty) {
                    return Err(Error::new(
                        field.ty.span(),
                        "a borrowing `Cow` capture must be written as `Cow<'p, str>`",
                    ));
                }
                fields.push(TypedField { ident, kind });
            }
            fields
        }
        // Tuple/empty-brace variants are rejected by `routes_for_variant`.
        Fields::Unit | Fields::Unnamed(_) => Vec::new(),
    };
    Ok(TypedVariant {
        ident: variant.ident.clone(),
        fields,
    })
}

/// Classifies an unannotated variant, whose fields must be owned.
fn classify_dynamic(variant: &syn::Variant) -> syn::Result<TypedVariant> {
    declared_fields(variant)?;

    let fields = match &variant.fields {
        Fields::Named(named) => {
            let mut fields = Vec::with_capacity(named.named.len());
            for field in &named.named {
                let ident = field.ident.clone().expect("named field has an identifier");
                if is_option(&field.ty) {
                    return Err(Error::new(
                        field.ty.span(),
                        "`#[resolver]` fields cannot be `Option`: a matched route always has its captures",
                    ));
                }
                if is_str_reference(&field.ty) {
                    return Err(Error::new(
                        field.ty.span(),
                        format!(
                            "dynamic route variant `{}` cannot borrow the path (`&str`); dynamic route fields must be owned (`String` or any `FromStr` type). Only static `#[route(...)]` variants may borrow.",
                            variant.ident
                        ),
                    ));
                }
                let kind = field_kind(&field.ty);
                if matches!(kind, FieldKind::Cow) {
                    return Err(Error::new(
                        field.ty.span(),
                        format!(
                            "dynamic route variant `{}` cannot use `Cow` (it borrows the path); use `String` for an owned, decoded value.",
                            variant.ident
                        ),
                    ));
                }
                if uses_capture_lifetime(&field.ty) {
                    return Err(Error::new(
                        field.ty.span(),
                        format!(
                            "dynamic route variant `{}` cannot carry the request lifetime `'p`; dynamic route fields must be fully owned",
                            variant.ident
                        ),
                    ));
                }
                fields.push(TypedField { ident, kind });
            }
            fields
        }
        Fields::Unit | Fields::Unnamed(_) => Vec::new(),
    };
    Ok(TypedVariant {
        ident: variant.ident.clone(),
        fields,
    })
}

/// Emits a raw-to-typed static variant conversion arm.
fn convert_arm(variant: &TypedVariant, raw_enum: &Ident, typed_enum: &Ident, runtime: &TokenStream2) -> TokenStream2 {
    let name = &variant.ident;
    if variant.fields.is_empty() {
        return quote! { #raw_enum::#name => #typed_enum::#name, };
    }
    let binds = variant.fields.iter().map(|field| &field.ident);
    let inits = variant.fields.iter().map(|field| {
        let id = &field.ident;
        let key = id.to_string();
        let value = match field.kind {
            FieldKind::Raw => quote! { #id },
            FieldKind::Owned => quote! { #runtime::__rt::coerce_owned(#id, #key)? },
            FieldKind::Cow => quote! { #runtime::__rt::coerce_cow(#id, #key)? },
            FieldKind::Parse => quote! { #runtime::__rt::coerce_parse(#id, #key)? },
        };
        quote! { #id: #value }
    });
    quote! {
        #raw_enum::#name { #(#binds),* } => #typed_enum::#name { #(#inits),* },
    }
}

/// Emits the capture extractor for a dynamic variant.
fn dynamic_extractor(variant: &TypedVariant, typed_enum: &Ident, schema_ty: &TokenStream2, runtime: &TokenStream2) -> TokenStream2 {
    let name = &variant.ident;
    let body = if variant.fields.is_empty() {
        quote! { ::core::result::Result::Ok(#typed_enum::#name) }
    } else {
        let inits = variant.fields.iter().enumerate().map(|(index, field)| {
            let id = &field.ident;
            let key = unraw_ident(id);
            let helper = match field.kind {
                FieldKind::Parse => quote! { parse },
                FieldKind::Owned | FieldKind::Raw | FieldKind::Cow => quote! { owned },
            };
            quote! { #id: #runtime::__rt::#helper(__caps, #index, #key)? }
        });
        quote! { ::core::result::Result::Ok(#typed_enum::#name { #(#inits),* }) }
    };
    quote! {
        fn __extract(__caps: &#runtime::__rt::Captures<'_, '_, '_>)
            -> ::core::result::Result<#schema_ty, #runtime::ResolveError<'static>>
        {
            #body
        }
    }
}

fn dynamic_conversion_arm(variant: &TypedVariant, typed_enum: &Ident) -> TokenStream2 {
    let name = &variant.ident;
    if variant.fields.is_empty() {
        quote! { #typed_enum::#name => #typed_enum::#name, }
    } else {
        let fields: Vec<&Ident> = variant.fields.iter().map(|field| &field.ident).collect();
        quote! {
            #typed_enum::#name { #( #fields ),* } => #typed_enum::#name { #( #fields ),* },
        }
    }
}

/// Emits the fluent `add_<variant>` registration method for a dynamic variant.
fn add_method(
    variant: &TypedVariant,
    enum_name: &Ident,
    schema_ty: &TokenStream2,
    extractor_ty: &TokenStream2,
    visibility: &TokenStream2,
    runtime: &TokenStream2,
) -> TokenStream2 {
    let name = variant.ident.to_string();
    let method = Ident::new(&format!("add_{}", to_snake_case(&name)), variant.ident.span());
    let seen = seen_ident(&variant.ident);
    let extractor = dynamic_extractor(variant, enum_name, schema_ty, runtime);
    let field_keys = variant.fields.iter().map(|field| unraw_ident(&field.ident));
    let doc = format!(
        "Registers a path for the dynamic [`{enum_name}::{name}`] variant.\n\n\
         `method` is matched exactly against the incoming HTTP method. `path` is \
         parsed as a Routerama path template, and its capture names must be \
         exactly the fields of `{enum_name}::{name}`.\n\n\
         Validation errors are accumulated and returned by [`build`](Self::build). \
         Call this method more than once to register aliases for the same variant."
    );
    quote! {
        #[doc = #doc]
        #[must_use]
        #visibility fn #method(mut self, method: #runtime::HttpMethod, path: impl ::core::convert::AsRef<str>) -> Self {
            #extractor
            let __extractor: #extractor_ty = __extract;
            self.__dyn.add(
                method,
                ::core::convert::AsRef::<str>::as_ref(&path),
                &[ #( #field_keys ),* ],
                #name,
                __extractor,
            );
            self.#seen = true;
            self
        }
    }
}

/// The registration flag for a dynamic variant.
fn seen_ident(variant: &Ident) -> Ident {
    Ident::new(&format!("__seen_{}", to_snake_case(&variant.to_string())), variant.span())
}

fn unraw_ident(ident: &Ident) -> String {
    let ident = ident.to_string();
    String::from(ident.strip_prefix("r#").unwrap_or(&ident))
}

/// Converts an `UpperCamelCase` variant name into `snake_case`.
pub(crate) fn to_snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use alloc::borrow::ToOwned as _;

    use syn::parse_quote;

    use super::*;

    fn expand_str(item: ItemEnum) -> Result<String, String> {
        expand(item).map(|t| t.to_string()).map_err(|e| e.to_string())
    }

    #[test]
    fn expands_a_static_resolver() {
        let item: ItemEnum = parse_quote! {
            enum Route<'p> {
                #[route(GET, "/books/{book}/reviews/{review}/{title}/{slug}")]
                Get {
                    book: &'p str,
                    review: u32,
                    title: String,
                    slug: std::borrow::Cow<'p, str>,
                },
            }
        };
        let code = expand_str(item).expect("valid");
        assert!(code.contains("fn resolve"), "{code}");
        assert!(code.contains("struct RouteResolver"), "{code}");
        assert!(code.contains("impl :: routerama :: Resolver for RouteResolver"), "{code}");
        assert!(code.contains("coerce_owned"), "{code}");
        assert!(code.contains("coerce_cow"), "{code}");
    }

    #[test]
    fn expands_a_dynamic_resolver() {
        let item: ItemEnum = parse_quote! {
            enum Route {
                Plugin { name: String, priority: u32 },
            }
        };
        let code = expand_str(item).expect("valid");
        assert!(code.contains("add_plugin"), "{code}");
        assert!(code.contains("RouteResolverBuilder"), "{code}");
        assert!(code.contains("__rt :: parse"), "{code}");
    }

    #[test]
    fn uses_an_explicit_resolver_name_as_the_builder_root() {
        let item: ItemEnum = parse_quote! {
            enum Route {
                Plugin,
            }
        };
        let code = expand_named(item, Some(parse_quote!(ApiResolver))).expect("valid").to_string();
        assert!(code.contains("struct ApiResolver"), "{code}");
        assert!(code.contains("struct ApiResolverBuilder"), "{code}");
        assert!(code.contains(":: core :: result :: Result < ApiResolver"), "{code}");
    }

    #[test]
    fn resolver_name_must_differ_from_the_route_enum() {
        let item: ItemEnum = parse_quote! {
            enum Route {
                #[route(GET, "/")]
                Home,
            }
        };
        let error = expand_named(item, Some(parse_quote!(Route))).expect_err("resolver and route names would collide");
        assert!(error.to_string().contains("must differ from the route enum name"), "{error}");
    }

    #[test]
    fn resolution_errors_are_not_injected_into_the_route_enum() {
        let item: ItemEnum = parse_quote! {
            enum Route {
                #[route(GET, "/a")]
                A,
            }
        };
        let code = expand_str(item).expect("valid");
        assert!(code.contains("enum Route { A , }"), "{code}");
        assert!(code.contains("ResolveError :: NotFound"), "{code}");
    }

    #[test]
    fn generated_dynamic_api_is_documented() {
        let item: ItemEnum = parse_quote! {
            enum Route {
                Plugin { name: String },
            }
        };
        let code = expand_str(item).expect("valid");
        for expected in [
            "Creates a [`RouteResolverBuilder`]",
            "Builds a resolver for [`Route`]",
            "Registers a path for the dynamic [`Route::Plugin`] variant",
            "Validation errors are accumulated",
        ] {
            assert!(code.contains(expected), "missing generated API documentation `{expected}`: {code}");
        }
    }

    #[test]
    fn generated_static_resolver_is_documented() {
        let item: ItemEnum = parse_quote! {
            enum Route {
                #[route(GET, "/health")]
                Health,
            }
        };
        let code = expand_str(item).expect("valid");
        for expected in ["All routes are compiled statically", "construction is infallible", "fn resolver"] {
            assert!(code.contains(expected), "missing generated API documentation `{expected}`: {code}");
        }
    }

    #[test]
    fn resolution_error_names_are_available_to_routes() {
        for available in ["NotFound", "MissingCapture", "InvalidCapture", "UndecodableCapture"] {
            let variant: proc_macro2::TokenStream = available.parse().expect("ident");
            let item: ItemEnum = syn::parse2(quote! {
                enum Route {
                    #[route(GET, "/x")]
                    #variant,
                }
            })
            .expect("valid enum");
            expand_str(item).expect("resolution errors do not reserve route names");
        }
    }

    #[test]
    fn route_variants_cannot_collide_with_builder() {
        let item = parse_quote! {
            enum Route {
                builder,
            }
        };
        let error = expand_str(item).expect_err("the generated builder function must not collide");
        assert!(error.contains("generated associated function"), "{error}");
    }

    #[test]
    fn route_variants_cannot_collide_with_static_resolver() {
        let item = parse_quote! {
            enum Route {
                #[route(GET, "/resolver")]
                resolver,
            }
        };
        let error = expand_str(item).expect_err("the generated resolver function must not collide");
        assert!(error.contains("generated associated function"), "{error}");
    }

    #[test]
    fn localized_helper_names_do_not_reserve_variant_names() {
        let item = parse_quote! {
            enum Route {
                Plugin,
                #[route(GET, "/static")]
                __static_resolve,
                #[route(GET, "/dynamic")]
                __dynamic_resolve,
                #[route(GET, "/extractor")]
                __dyn_plugin,
            }
        };
        expand_str(item).expect("localized helpers cannot collide with variants");
    }

    #[test]
    fn captureless_resolver_does_not_get_an_injected_lifetime() {
        let item: ItemEnum = parse_quote! {
            enum Route {
                #[route(GET, "/a")]
                A,
            }
        };
        let code = expand_str(item).expect("valid");
        assert!(!code.contains("enum Route < 'p >"), "an injected `'p`: {code}");
        assert!(code.contains("type Route < 'p > = Route"), "{code}");
    }

    #[test]
    fn dynamic_variant_borrowing_is_rejected() {
        let item: ItemEnum = parse_quote! {
            enum Route<'p> {
                Plugin { name: &'p str },
            }
        };
        let error = expand_str(item).expect_err("a borrowing dynamic field must be rejected");
        assert!(error.contains("owned"), "{error}");
    }

    #[test]
    fn dynamic_variant_cow_is_rejected() {
        let item: ItemEnum = parse_quote! {
            enum Route<'p> {
                Plugin { name: std::borrow::Cow<'p, str> },
            }
        };
        let _ = expand_str(item).expect_err("a `Cow` dynamic field must be rejected");
    }

    #[test]
    fn dynamic_variant_types_cannot_carry_the_request_lifetime() {
        let item: ItemEnum = parse_quote! {
            enum Route<'p> {
                Plugin { value: Invariant<'p> },
            }
        };
        let error = expand_str(item).expect_err("dynamic extractor results must not depend on the request lifetime");
        assert!(error.contains("fully owned"), "{error}");
    }

    #[test]
    fn dynamic_variant_option_field_is_rejected() {
        let item: ItemEnum = parse_quote! {
            enum Route {
                Plugin { name: Option<String> },
            }
        };
        let error = expand_str(item).expect_err("an `Option` dynamic field must be rejected");
        assert!(error.contains("Option"), "{error}");
    }

    #[test]
    fn option_field_is_rejected() {
        let item: ItemEnum = parse_quote! {
            enum Route {
                #[route(GET, "/b/{book}")]
                B { book: Option<String> },
            }
        };
        expand_str(item).unwrap_err();
    }

    #[test]
    fn tuple_variant_is_rejected() {
        let item: ItemEnum = parse_quote! {
            enum Route<'p> {
                #[route(GET, "/b/{book}")]
                B(&'p str),
            }
        };
        expand_str(item).unwrap_err();
    }

    #[test]
    fn mixed_static_and_dynamic_expands() {
        let item: ItemEnum = parse_quote! {
            enum Route<'p> {
                #[route(GET, "/books/{book}")]
                GetBook { book: &'p str },
                Plugin { name: String },
            }
        };
        let code = expand_str(item).expect("valid");
        assert!(code.contains("add_plugin"), "{code}");
        assert!(code.contains("fn __static_resolve"), "{code}");
        assert!(code.contains("__dynamic_resolve"), "{code}");
        assert!(code.contains("fn __convert_dynamic < 'p >"), "{code}");
        assert!(code.contains("Route :: Plugin { name } => Route :: Plugin { name }"), "{code}");
        assert!(code.contains("__convert_dynamic (__r)"), "{code}");
    }

    #[test]
    fn raw_dynamic_field_names_are_registered_without_the_raw_prefix() {
        let item: ItemEnum = parse_quote! {
            enum Route {
                Dynamic { r#type: String },
            }
        };
        let code = expand_str(item).expect("raw fields are valid Rust identifiers");
        assert!(code.contains("& [\"type\"]"), "{code}");
        assert!(!code.contains("\"r#type\""), "{code}");
    }

    #[test]
    fn bound_p_lifetimes_in_dynamic_fields_do_not_borrow_from_the_request() {
        let item: ItemEnum = parse_quote! {
            enum Route<'p> {
                #[route(GET, "/static/{value}")]
                Static { value: &'p str },
                Dynamic { callback: for<'p> fn(&'p str) },
            }
        };
        expand_str(item).expect("the inner HRTB lifetime shadows the request lifetime");
    }

    #[test]
    fn dynamic_variants_must_not_generate_the_same_method_name() {
        let item: ItemEnum = parse_quote! {
            enum Route {
                Foo,
                foo,
            }
        };
        let error = expand_str(item).expect_err("the generated methods would collide");
        assert!(error.contains("both generate `add_foo`"), "{error}");
    }

    #[test]
    fn conditionally_compiled_variants_and_fields_are_rejected() {
        let variant: ItemEnum = parse_quote! {
            enum Route {
                #[cfg(feature = "x")]
                #[route(GET, "/x")]
                X,
            }
        };
        let error = expand_str(variant).expect_err("cfg-gated variants break generated references");
        assert!(error.contains("conditionally compiled variants"), "{error}");

        let field: ItemEnum = parse_quote! {
            enum Route<'p> {
                #[route(GET, "/x/{x}")]
                X {
                    #[cfg(feature = "x")]
                    x: &'p str,
                },
            }
        };
        let error = expand_str(field).expect_err("cfg-gated fields break generated extractors");
        assert!(error.contains("conditionally compiled fields"), "{error}");
    }

    #[test]
    fn raw_identifiers_are_rejected_instead_of_panicking_name_generation() {
        let item: ItemEnum = parse_quote! {
            enum r#type {
                Route,
            }
        };
        let error = expand_str(item).expect_err("the generated builder name cannot contain `r#`");
        assert!(error.contains("raw identifier"), "{error}");

        let item: ItemEnum = parse_quote! {
            enum Route {
                r#type,
            }
        };
        let error = expand_str(item).expect_err("generated add/extractor names cannot contain `r#`");
        assert!(error.contains("raw route variant"), "{error}");
    }

    #[test]
    fn runtime_path_uses_a_renamed_dependency() {
        assert!(!runtime_path().is_empty());
        assert_eq!(runtime_path_for(Some(FoundCrate::Name("rr".to_owned()))).to_string(), ":: rr");
        assert_eq!(runtime_path_for(Some(FoundCrate::Itself)).to_string(), ":: routerama");
        assert_eq!(runtime_path_for(None).to_string(), ":: routerama");
    }

    #[test]
    fn runtime_path_supports_a_keyword_dependency_alias() {
        assert_eq!(runtime_path_for(Some(FoundCrate::Name("type".to_owned()))).to_string(), ":: r#type");
    }

    #[test]
    fn static_borrowing_fields_must_use_the_capture_lifetime() {
        let static_str: ItemEnum = parse_quote! {
            enum Route<'p> {
                #[route(GET, "/x/{x}")]
                X { x: &'static str },
            }
        };
        let error = expand_str(static_str).expect_err("the generated value only lives for `'p`");
        assert!(error.contains("&'p str"), "{error}");

        let static_cow: ItemEnum = parse_quote! {
            enum Route<'p> {
                #[route(GET, "/x/{x}")]
                X { x: std::borrow::Cow<'static, str> },
            }
        };
        let error = expand_str(static_cow).expect_err("the generated Cow only lives for `'p`");
        assert!(error.contains("Cow<'p, str>"), "{error}");
    }

    #[test]
    fn a_captureless_route_enum_is_re_emitted_without_a_lifetime() {
        let item: ItemEnum = parse_quote! {
            enum Solo {
                #[route(GET, "/a")]
                A,
            }
        };
        let code = expand_str(item).expect("valid");
        assert!(code.contains("enum Solo"), "{code}");
        assert!(!code.contains("enum Solo < 'p >"), "{code}");
    }

    #[test]
    fn a_pure_static_resolver_keeps_the_raw_enum_local_and_emits_no_dynamic_path() {
        let item: ItemEnum = parse_quote! {
            enum Stat<'p> {
                #[route(GET, "/s/{n}")]
                S { n: u32 },
            }
        };
        let code = expand_str(item).expect("valid");
        assert!(code.contains("__StatRaw"), "{code}");
        assert!(code.contains("coerce_parse"), "{code}");
        assert!(!code.contains("splits_verbs"), "{code}");
        assert!(!code.contains("__dynamic_resolve"), "{code}");
        assert!(!code.contains("route (GET"), "{code}");
        assert!(code.contains("__raw : __StatRaw < 'p >"), "{code}");
    }

    #[test]
    fn helper_types_do_not_escape_into_the_user_module() {
        let item: ItemEnum = parse_quote! {
            enum Scoped<'p> {
                #[route(GET, "/static/{value}")]
                Static { value: &'p str },
                Dynamic { value: String },
            }
        };
        let expanded = expand(item).expect("valid");
        let file: syn::File = syn::parse2(expanded).expect("expansion is a valid Rust file");
        assert!(file.items.iter().all(|item| !matches!(item, syn::Item::Type(_))));
        let type_names: Vec<String> = file
            .items
            .iter()
            .filter_map(|item| match item {
                syn::Item::Enum(item) => Some(item.ident.to_string()),
                syn::Item::Struct(item) => Some(item.ident.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(type_names, ["Scoped", "ScopedResolver", "ScopedResolverBuilder"]);

        let associated_functions: Vec<String> = file
            .items
            .iter()
            .filter_map(|item| match item {
                syn::Item::Impl(item) if item.trait_.is_none() => Some(item),
                _ => None,
            })
            .filter(|item| {
                matches!(
                    &*item.self_ty,
                    syn::Type::Path(path)
                        if path.path.segments.last().is_some_and(|segment| segment.ident == "Scoped")
                )
            })
            .flat_map(|item| item.items.iter())
            .map(quote::ToTokens::to_token_stream)
            .map(|item| item.to_string())
            .collect();
        assert_eq!(associated_functions.len(), 1);
        assert!(associated_functions[0].contains("fn builder"), "{associated_functions:?}");
    }

    #[test]
    fn static_resolver_emits_only_the_route_enum() {
        let item: ItemEnum = parse_quote! {
            enum Static {
                #[route(GET, "/health")]
                Health,
            }
        };
        let expanded = expand(item).expect("valid");
        let file: syn::File = syn::parse2(expanded).expect("expansion is a valid Rust file");
        let type_names: Vec<String> = file
            .items
            .iter()
            .filter_map(|item| match item {
                syn::Item::Enum(item) => Some(item.ident.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(type_names, ["Static"]);
        let code = file.to_token_stream().to_string();
        assert!(code.contains("struct StaticResolver"), "{code}");
        assert!(code.contains("fn resolver"), "{code}");
        assert!(!code.contains("fn builder"), "{code}");
        assert!(!code.contains("StaticResolverBuilder"), "{code}");
    }

    #[test]
    fn a_mixed_non_verb_resolver_takes_the_verb_split_branch() {
        let item: ItemEnum = parse_quote! {
            enum Mix<'p> {
                #[route(GET, "/a/{x}")]
                A { x: &'p str },
                GetBook { book: String },
            }
        };
        let code = expand_str(item).expect("valid");
        assert!(code.contains("splits_verbs"), "{code}");
        assert!(code.contains("__dynamic_resolve"), "{code}");
        assert!(code.contains("fn __extract"), "{code}");
        assert!(code.contains("fn add_get_book"), "{code}");
    }

    #[test]
    fn a_static_verb_route_forces_dynamic_verb_splitting() {
        let item: ItemEnum = parse_quote! {
            enum Mix<'p> {
                #[route(GET, "/a/{x}")]
                A { x: &'p str },
                #[route(GET, "/b/{x}:archive")]
                Archive { x: &'p str },
                Dynamic { x: String },
            }
        };
        let code = expand_str(item).expect("valid");
        assert!(
            !code.contains("splits_verbs"),
            "a static verb route must select the direct static-first branch: {code}"
        );
    }

    #[test]
    fn a_pure_dynamic_resolver_never_verb_splits() {
        let item: ItemEnum = parse_quote! {
            enum Dyn {
                GetBook { book: String },
            }
        };
        let code = expand_str(item).expect("valid");
        assert!(!code.contains("splits_verbs"), "{code}");
        assert!(!code.contains("fn __static_resolve"), "{code}");
        assert!(code.contains("fn add_get_book"), "{code}");
        assert!(code.contains("fn __extract"), "{code}");
    }
}
