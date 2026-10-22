// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Route contract construction, lifetime rewriting, and generated identifiers.

use alloc::format;
use alloc::string::{String, ToString as _};
use alloc::vec::Vec;

use proc_macro2::{Ident, Span, TokenStream as TokenStream2, TokenTree};
use quote::quote;
use syn::spanned::Spanned as _;
use syn::visit_mut::VisitMut as _;
use syn::{Attribute, Error, ImplItem, Lifetime, PathArguments, TraitBound, Type};

use super::binding::catcher_for;
use super::model::{Argument, ExtractionKind, GeneratedIdents, Handler, RouteContract, RoutingPolicy, SharedState};

pub(super) struct ParentModulePaths;

impl syn::visit_mut::VisitMut for ParentModulePaths {
    fn visit_path_mut(&mut self, i: &mut syn::Path) {
        if i.leading_colon.is_none()
            && let Some(first) = i.segments.first_mut()
        {
            if first.ident == "self" {
                first.ident = Ident::new("super", first.ident.span());
            } else if first.ident == "super" {
                i.segments.insert(0, syn::parse_quote!(super));
            }
        }
        syn::visit_mut::visit_path_mut(self, i);
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the where-clause contract enumerates every parts, body, transform, and interceptor bound in one place"
)]
pub(super) fn route_contract(
    handlers: &[Handler],
    policy: &RoutingPolicy,
    body_type: &Ident,
    shared_state: &SharedState,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> syn::Result<RouteContract> {
    let state_type = &shared_state.ty;
    let mut bounds = Vec::new();
    let mut rendered = Vec::new();
    for handler in handlers {
        for argument in &handler.arguments {
            let bound = match argument {
                Argument::Parts(_, ty) => {
                    let lifetime = Lifetime::new("'__routerama_request", Span::call_site());
                    let mut bound_type = ty.clone();
                    RequestPartsLifetime::new(&lifetime).rewrite(&mut bound_type)?;
                    if let Some(catcher) = catcher_for(policy, ExtractionKind::Parts, ty) {
                        let rejection = &catcher.rejection_type;
                        quote! {
                            for<#lifetime> #bound_type:
                                #runtime::FromRequestParts<
                                    #lifetime,
                                    #state_type,
                                    Rejection = #rejection,
                                >
                        }
                    } else if shared_state.generic.is_some() {
                        // Erase this higher-ranked rejection body so its private
                        // type does not enter the route signature.
                        let extraction = quote! {
                            for<#lifetime> #bound_type:
                                #runtime::FromRequestParts<#lifetime, #state_type>
                        };
                        let text = extraction.to_string();
                        if !rendered.contains(&text) {
                            rendered.push(text);
                            bounds.push(extraction);
                        }
                        let data_bound = if heterogeneous_data {
                            TokenStream2::new()
                        } else {
                            quote! { Data = #runtime::bytes::Bytes, }
                        };
                        quote! {
                            for<#lifetime> <#bound_type as #runtime::FromRequestParts<
                                #lifetime,
                                #state_type,
                            >>::Rejection: #response_runtime::IntoResponse<
                                Body: #runtime::http_body::Body<
                                    #data_bound
                                    Error: ::core::error::Error + ::core::marker::Send + ::core::marker::Sync + 'static,
                                > + ::core::marker::Send + 'static,
                            >
                        }
                    } else {
                        // Fixed-state validation proves this concrete extractor
                        // contract at the service definition.
                        continue;
                    }
                }
                Argument::Body(_, ty) => {
                    let body_input = policy.body_input_for(&handler.method, body_type);
                    if let Some(catcher) = catcher_for(policy, ExtractionKind::Body, ty) {
                        let rejection = &catcher.rejection_type;
                        quote! {
                            #ty: #runtime::FromRequestBody<
                                #state_type,
                                #body_input,
                                Rejection = #rejection,
                            >
                        }
                    } else {
                        let data_bound = if heterogeneous_data {
                            TokenStream2::new()
                        } else {
                            quote! { Data = #runtime::bytes::Bytes, }
                        };
                        let rejection_bound = quote! {
                            <<#ty as #runtime::FromRequestBody<
                                #state_type,
                                #body_input,
                            >>::Rejection as #response_runtime::IntoResponse>::Body:
                                #runtime::http_body::Body<
                                    #data_bound
                                    Error: ::core::error::Error + 'static,
                                >
                        };
                        push_unique_bound(&mut bounds, &mut rendered, rejection_bound);
                        quote! { #ty: #runtime::FromRequestBody<#state_type, #body_input> }
                    }
                }
                Argument::Capture(_) => continue,
            };
            let text = bound.to_string();
            if !rendered.contains(&text) {
                rendered.push(text);
                bounds.push(bound);
            }
        }
        let response_type = &handler.response_type;
        let bound = quote! { #response_type: #response_runtime::IntoResponse };
        let text = bound.to_string();
        if !rendered.contains(&text) {
            rendered.push(text);
            bounds.push(bound);
        }
        push_response_data_bound(
            response_type,
            heterogeneous_data,
            runtime,
            response_runtime,
            &mut bounds,
            &mut rendered,
        );
    }
    if let Some(fallback) = &policy.fallback {
        let response_type = &fallback.response_type;
        let bound = quote! { #response_type: #response_runtime::IntoResponse };
        let text = bound.to_string();
        if !rendered.contains(&text) {
            rendered.push(text);
            bounds.push(bound);
        }
        push_response_data_bound(
            response_type,
            heterogeneous_data,
            runtime,
            response_runtime,
            &mut bounds,
            &mut rendered,
        );
    }
    for catcher in &policy.catchers {
        let response_type = &catcher.response_type;
        let bound = quote! { #response_type: #response_runtime::IntoResponse };
        let text = bound.to_string();
        if !rendered.contains(&text) {
            rendered.push(text);
            bounds.push(bound);
        }
        push_response_data_bound(
            response_type,
            heterogeneous_data,
            runtime,
            response_runtime,
            &mut bounds,
            &mut rendered,
        );
    }
    for response_type in policy
        .befores
        .iter()
        .map(|before| &before.response_type)
        .chain(policy.transforms.iter().map(|transform| &transform.response_type))
    {
        let bound = quote! { #response_type: #response_runtime::IntoResponse };
        let text = bound.to_string();
        if !rendered.contains(&text) {
            rendered.push(text);
            bounds.push(bound);
        }
        push_response_data_bound(
            response_type,
            heterogeneous_data,
            runtime,
            response_runtime,
            &mut bounds,
            &mut rendered,
        );
    }
    for predicate in policy.transforms.iter().flat_map(|transform| &transform.body_bounds) {
        let bound = quote! { #predicate };
        let text = bound.to_string();
        if !rendered.contains(&text) {
            rendered.push(text);
            bounds.push(bound);
        }
    }
    if !policy.transforms.is_empty() {
        let bound = quote! { #body_type: #runtime::http_body::Body<Data = #runtime::bytes::Bytes> };
        let text = bound.to_string();
        if !rendered.contains(&text) {
            rendered.push(text);
            bounds.push(bound);
        }
    }
    Ok(RouteContract { bounds })
}

fn push_response_data_bound(
    response_type: &Type,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
    bounds: &mut Vec<TokenStream2>,
    rendered: &mut Vec<String>,
) {
    let data_bound = if heterogeneous_data {
        TokenStream2::new()
    } else {
        quote! { Data = #runtime::bytes::Bytes, }
    };
    push_unique_bound(
        bounds,
        rendered,
        quote! {
            <#response_type as #response_runtime::IntoResponse>::Body:
                #runtime::http_body::Body<
                    #data_bound
                    Error: ::core::error::Error + 'static,
                >
        },
    );
}

fn push_unique_bound(bounds: &mut Vec<TokenStream2>, rendered: &mut Vec<String>, bound: TokenStream2) {
    let text = bound.to_string();
    if !rendered.contains(&text) {
        rendered.push(text);
        bounds.push(bound);
    }
}

pub(super) fn token_stream_contains_ident(tokens: TokenStream2, name: &str) -> bool {
    tokens.into_iter().any(|token| match token {
        TokenTree::Ident(ident) => ident.to_string().trim_start_matches("r#") == name,
        TokenTree::Group(group) => token_stream_contains_ident(group.stream(), name),
        TokenTree::Punct(_) | TokenTree::Literal(_) => false,
    })
}

pub(super) struct RequestPartsLifetime<'a> {
    lifetime: &'a Lifetime,
    error: Option<Error>,
    /// The lifetimes the enclosing higher-ranked binders (`for<'a>`) declare.
    ///
    /// They are bound by the type itself and are not free in the handler.
    shadowed: ShadowedLifetimes,
}

impl<'a> RequestPartsLifetime<'a> {
    pub(super) fn new(lifetime: &'a Lifetime) -> Self {
        Self {
            lifetime,
            error: None,
            shadowed: ShadowedLifetimes::default(),
        }
    }

    /// Ties every elided extractor lifetime to the request-parts borrow.
    ///
    /// The outermost reference *is* the request-parts borrow, so an explicit
    /// lifetime there — including `'static` — can never be honored. A nested
    /// `'static` names an owned, request-independent type instead, so it is
    /// preserved verbatim; every other named lifetime is rejected because it
    /// cannot be tied unambiguously to the borrow.
    pub(super) fn rewrite(mut self, ty: &mut Type) -> syn::Result<()> {
        if let Type::Reference(reference) = &*ty
            && let Some(lifetime) = &reference.lifetime
            && lifetime.ident != "_"
        {
            return Err(Self::rejection(lifetime));
        }
        self.visit_type_mut(ty);
        match self.error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn rejection(lifetime: &Lifetime) -> Error {
        Error::new(
            lifetime.span(),
            "request-parts extractor lifetimes must be elided or use `'_`; an explicit lifetime cannot be tied unambiguously to the request-parts borrow",
        )
    }

    fn rewrite_lifetime(&mut self, lifetime: &mut Lifetime) {
        if lifetime.ident == "_" {
            *lifetime = self.lifetime.clone();
        } else if lifetime.ident != "static" && !self.shadowed.binds(lifetime) && self.error.is_none() {
            self.error = Some(Self::rejection(lifetime));
        }
    }
}

/// The lifetimes declared by the higher-ranked binders a type visitor is inside.
///
/// The binders are the enclosing `for<'a>` forms.
///
/// A lifetime named by such a binder is bound by the type — `Box<dyn for<'a>
/// Trait<'a>>` names no lifetime of the surrounding item — so it must neither be
/// rewritten nor reported as a free lifetime.
#[derive(Default)]
struct ShadowedLifetimes {
    names: Vec<Ident>,
}

impl ShadowedLifetimes {
    /// Records the binder's lifetimes, returning the depth to restore afterwards.
    fn enter(&mut self, lifetimes: Option<&syn::BoundLifetimes>) -> usize {
        let depth = self.names.len();
        if let Some(lifetimes) = lifetimes {
            self.names
                .extend(lifetimes.lifetimes.iter().filter_map(|parameter| match parameter {
                    syn::GenericParam::Lifetime(parameter) => Some(parameter.lifetime.ident.clone()),
                    syn::GenericParam::Type(_) | syn::GenericParam::Const(_) => None,
                }));
        }
        depth
    }

    fn leave(&mut self, depth: usize) {
        self.names.truncate(depth);
    }

    fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    fn binds(&self, lifetime: &Lifetime) -> bool {
        self.names.contains(&lifetime.ident)
    }
}

impl syn::visit_mut::VisitMut for RequestPartsLifetime<'_> {
    fn visit_type_fn_ptr_mut(&mut self, _i: &mut syn::TypeFnPtr) {
        // Bare function parameters bind their own elided lifetimes.
    }

    fn visit_trait_bound_mut(&mut self, i: &mut TraitBound) {
        let depth = self.shadowed.enter(i.lifetimes.as_ref());
        syn::visit_mut::visit_trait_bound_mut(self, i);
        self.shadowed.leave(depth);
    }

    fn visit_path_arguments_mut(&mut self, i: &mut PathArguments) {
        if matches!(i, PathArguments::Parenthesized(_)) {
            // `Fn(&T)`-style parameters bind their own elided lifetimes.
            return;
        }
        syn::visit_mut::visit_path_arguments_mut(self, i);
    }

    fn visit_type_reference_mut(&mut self, i: &mut syn::TypeReference) {
        match &mut i.lifetime {
            Some(lifetime) => self.rewrite_lifetime(lifetime),
            None => i.lifetime = Some(self.lifetime.clone()),
        }
        self.visit_type_mut(&mut i.elem);
    }

    fn visit_lifetime_mut(&mut self, i: &mut Lifetime) {
        self.rewrite_lifetime(i);
    }
}

#[derive(Default)]
pub(super) struct StaticAnonymousLifetimes {
    shadowed: ShadowedLifetimes,
}

impl syn::visit_mut::VisitMut for StaticAnonymousLifetimes {
    fn visit_type_fn_ptr_mut(&mut self, _i: &mut syn::TypeFnPtr) {
        // Bare function parameters bind their own elided lifetimes.
    }

    fn visit_trait_bound_mut(&mut self, i: &mut TraitBound) {
        let depth = self.shadowed.enter(i.lifetimes.as_ref());
        syn::visit_mut::visit_trait_bound_mut(self, i);
        self.shadowed.leave(depth);
    }

    fn visit_path_arguments_mut(&mut self, i: &mut PathArguments) {
        if matches!(i, PathArguments::Parenthesized(_)) {
            // `Fn(&T)`-style parameters bind their own elided lifetimes.
            return;
        }
        syn::visit_mut::visit_path_arguments_mut(self, i);
    }

    fn visit_type_reference_mut(&mut self, i: &mut syn::TypeReference) {
        // Inside a higher-ranked binder an elided lifetime is bound by that
        // binder, so naming `'static` there would change the type.
        if self.shadowed.is_empty() {
            match &mut i.lifetime {
                Some(lifetime) if lifetime.ident == "_" => {
                    *lifetime = Lifetime::new("'static", lifetime.apostrophe);
                }
                Some(_) => {}
                None => {
                    i.lifetime = Some(Lifetime::new("'static", i.and_token.span()));
                }
            }
        }
        self.visit_type_mut(&mut i.elem);
    }

    fn visit_lifetime_mut(&mut self, i: &mut Lifetime) {
        if self.shadowed.is_empty() && i.ident == "_" {
            *i = Lifetime::new("'static", i.apostrophe);
        }
    }
}

pub(super) fn generated_idents(handlers: &[Handler]) -> GeneratedIdents {
    GeneratedIdents {
        request: generated_ident("__routerama_request", handlers),
        state: generated_ident("__routerama_state", handlers),
        parts: generated_ident("__routerama_parts", handlers),
        body: generated_ident("__routerama_body", handlers),
        route: generated_ident("__routerama_route", handlers),
        response: generated_ident("__routerama_response", handlers),
        failure: generated_ident("__routerama_failure_stage", handlers),
    }
}

pub(super) fn generated_ident(base: &str, handlers: &[Handler]) -> Ident {
    let mut name = base.to_string();
    while handlers
        .iter()
        .flat_map(|handler| &handler.arguments)
        .any(|argument| match argument {
            Argument::Capture(ident) | Argument::Parts(ident, _) | Argument::Body(ident, _) => ident == &name,
        })
    {
        name.insert(0, '_');
    }
    Ident::new(&name, Span::call_site())
}

pub(super) fn variant_name(method: &Ident) -> syn::Result<Ident> {
    let spelling = method.to_string();
    if spelling.starts_with("r#") {
        return Err(Error::new(method.span(), "service handler names cannot be raw identifiers"));
    }
    let mut name = String::new();
    for part in spelling.split('_').filter(|part| !part.is_empty()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            name.extend(first.to_uppercase());
            name.extend(chars);
        }
    }
    if name.is_empty() {
        return Err(Error::new(method.span(), "service handler names must generate a route variant"));
    }
    Ok(Ident::new(&name, method.span()))
}

pub(super) fn fmt_names(names: &[String]) -> String {
    if names.is_empty() {
        "{}".to_string()
    } else {
        format!("{{{}}}", names.join(", "))
    }
}

pub(super) fn has_route_attr(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| attribute.path().is_ident("route"))
}

pub(super) fn has_policy_attr(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("fallback")
            || attribute.path().is_ident("catch")
            || attribute.path().is_ident("before")
            || attribute.path().is_ident("after")
            || attribute.path().is_ident("transform")
    })
}

pub(super) trait ImplItemAttributes {
    fn attrs(&self) -> &[Attribute];
}

impl ImplItemAttributes for ImplItem {
    /// Returns the attributes of an `ImplItem`.
    ///
    /// Its only call site (`expand`'s item-scan guard) invokes this on `other`
    /// after two preceding match arms already exhaustively consumed every
    /// `ImplItem::Fn(..)`, so `other` can never be `Self::Fn(..)` here; that
    /// arm is kept only so this stays total over every `ImplItem` variant.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn attrs(&self) -> &[Attribute] {
        match self {
            Self::Const(item) => &item.attrs,
            Self::Fn(item) => &item.attrs,
            Self::Type(item) => &item.attrs,
            Self::Macro(item) => &item.attrs,
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use quote::quote;
    use syn::parse_quote;
    use syn::visit_mut::VisitMut as _;

    use super::*;
    use crate::macro_impl::router::test_support::{expand_fixed_mounted_router, expand_fixed_router, expand_router};

    #[test]
    fn generated_module_rebases_relative_capture_type_paths() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/{local}/{outer}/{plain}/{absolute}")]
                async fn get(
                    &self,
                    local: self::LocalId,
                    outer: super::OuterId,
                    plain: PlainId,
                    absolute: ::std::string::String,
                ) -> Response {
                    response(local, outer, plain, absolute)
                }
            }
        })
        .expect("relative capture type paths are valid");
        let code = generated.to_string();
        assert!(code.contains("local : super :: LocalId"), "{code}");
        assert!(code.contains("outer : super :: super :: OuterId"), "{code}");
        assert!(
            code.contains(": :: std :: string :: String") || code.contains(":: std :: string :: String"),
            "an absolute (leading-colon) capture path is left untouched: {code}"
        );
        assert!(
            code.contains("plain : PlainId"),
            "a capture path whose first segment is neither `self` nor `super` is left untouched: {code}"
        );
    }

    #[test]
    fn anonymous_lifetimes_in_nested_parts_extractors_are_tied_to_the_request() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn inspect(
                    &self,
                    headers: &HeaderMap,
                    user_agent: Wrapper<UserAgent<'_>>,
                ) -> Response {
                    response(headers, user_agent)
                }
            }
        })
        .expect("anonymous extractor lifetimes are supported");
        let code = generated.to_string();

        assert!(code.contains("& '__routerama_request HeaderMap"), "{code}");
        assert!(code.contains("Wrapper < UserAgent < '__routerama_request > >"), "{code}");
        assert!(code.contains("FromRequestParts < '_ , __RouteramaState >"), "{code}");
    }

    #[test]
    fn nested_static_lifetimes_in_parts_extractors_are_preserved() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn inspect(
                    &self,
                    banner: Wrapper<&'static str>,
                    version: Version<'static>,
                ) -> Response {
                    response(banner, version)
                }
            }
        })
        .expect("a nested `'static` names an owned, request-independent type");
        let code = generated.to_string();

        assert!(code.contains("Wrapper < & 'static str >"), "{code}");
        assert!(code.contains("Version < 'static >"), "{code}");
        assert!(!code.contains("'__routerama_request str"), "{code}");
    }

    #[test]
    fn static_anonymous_lifetimes_normalizes_elided_and_underscore_references() {
        let mut ty: Type = syn::parse_quote! {
            (
                fn(&'_ i32) -> &i32,
                Box<dyn Fn(&i32) -> &i32>,
                Box<dyn for<'a> Trait<'a>>,
                &'_ i32,
                &'a i32,
                &i32,
                Foo<'_>,
            )
        };

        StaticAnonymousLifetimes::default().visit_type_mut(&mut ty);

        let rendered = quote! { #ty }.to_string();
        // The bare `fn` pointer's own `'_` is left untouched: it binds its
        // own parameter, so it is not rewritten to `'static`.
        assert!(rendered.contains("fn (& '_ i32) -> & i32"), "{rendered}");
        // `Fn(&T)` sugar is parenthesized path arguments, which bind their
        // own elided lifetime the same way a bare `fn` pointer does.
        assert!(rendered.contains("Fn (& i32) -> & i32"), "{rendered}");
        // Inside the higher-ranked `for<'a>` binder, `'a` is bound by the
        // bound itself and must not be renamed.
        assert!(rendered.contains("for < 'a > Trait < 'a >"), "{rendered}");
        // Outside any binder, an explicit `'_` reference becomes `'static`...
        assert!(rendered.contains("& 'static i32"), "{rendered}");
        // ...a differently-named free lifetime is left as-is...
        assert!(rendered.contains("& 'a i32"), "{rendered}");
        // ...and a fully elided reference gains an explicit `'static`.
        assert!(rendered.matches("& 'static i32").count() >= 2, "{rendered}");
        // A bare `'_` lifetime argument outside any binder is also renamed.
        assert!(rendered.contains("Foo < 'static >"), "{rendered}");
    }

    #[test]
    fn explicit_parts_extractor_lifetimes_that_cannot_be_tied_are_rejected() {
        for argument in [
            quote! { headers: &'static HeaderMap },
            quote! { headers: &'a HeaderMap },
            quote! { user_agent: UserAgent<'a> },
            quote! { user_agent: Wrapper<&'a str> },
        ] {
            let error = expand_router(quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn inspect(&self, #argument) -> Response {
                        response()
                    }
                }
            })
            .expect_err("an explicit request-tied lifetime cannot be honored");

            assert!(error.to_string().contains("must be elided or use `'_`"), "{argument}: {error}");
        }
    }

    #[test]
    fn route_contract_lifetime_errors_propagate_through_every_generated_entry_point() {
        let bad_handler = quote! {
            #[route(GET, "/")]
            async fn inspect(&self, headers: &'static HeaderMap) -> Response {
                response(headers)
            }
        };

        // Fixed state with erased mounts: exercises
        // `static_mounted_route_method`'s `route_contract` call and its
        // propagation back through `expand`.
        let error = expand_fixed_mounted_router(
            syn::parse_quote!(AppState),
            quote! {
                impl Api {
                    #bad_handler
                }
            },
        )
        .expect_err("an explicit request-tied lifetime cannot be honored");
        assert!(error.to_string().contains("must be elided or use `'_`"), "{error}");

        // A mixed static/dynamic service: exercises `dynamic_service_api`'s
        // own `route_contract` call and its propagation back through
        // `expand`.
        let error = expand_router(quote! {
            impl Api {
                #[route(dynamic)]
                async fn plugin(&self, #[capture] name: String) -> Response {
                    response(name)
                }

                #bad_handler
            }
        })
        .expect_err("an explicit request-tied lifetime cannot be honored");
        assert!(error.to_string().contains("must be elided or use `'_`"), "{error}");
    }

    #[test]
    fn callable_argument_lifetimes_remain_independently_bound() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn inspect(
                    &self,
                    callback: Callback<fn(&str)>,
                    callable: &dyn Fn(&str),
                ) -> Response {
                    response(callback, callable)
                }
            }
        })
        .expect("callable argument lifetimes do not borrow from request parts");
        let code = generated.to_string();

        assert!(code.contains("Callback < fn (& str) >"), "{code}");
        assert!(code.contains("& '__routerama_request dyn Fn (& str)"), "{code}");
        assert!(!code.contains("fn (& '__routerama_request str)"), "{code}");
        assert!(!code.contains("Fn (& '__routerama_request str)"), "{code}");
    }

    #[test]
    fn service_handler_names_cannot_be_raw_identifiers() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn r#move(&self) -> StatusCode {
                    response(())
                }
            }
        })
        .expect_err("a raw-identifier handler name cannot form a route variant");
        assert!(
            error.to_string().contains("service handler names cannot be raw identifiers"),
            "{error}"
        );
    }

    #[test]
    fn a_higher_ranked_bound_extractor_type_is_rewritten_under_fixed_state() {
        let generated = expand_fixed_router(
            parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self, value: Box<dyn for<'a, T> Fn(&'a T) -> bool>) -> StatusCode {
                        response(value)
                    }
                }
            },
        )
        .expect("a parts extractor whose type has a higher-ranked bound with a mixed generic parameter list is valid");
        let code = generated.to_string();
        assert!(code.contains("self . home (value)"), "{code}");
    }

    // --- StaticAnonymousLifetimes visiting a reference nested inside a body extractor type ---

    #[test]
    fn a_fixed_state_body_extractor_with_a_nested_elided_reference_expands() {
        let generated = expand_fixed_router(
            parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(POST, "/")]
                    async fn create(&self, #[body] value: Wrapper<&'_ str>) -> StatusCode {
                        response(value)
                    }
                }
            },
        )
        .expect("a body extractor type nesting an elided-lifetime reference is valid under fixed state");
        let code = generated.to_string();
        assert!(code.contains("self . create (value)"), "{code}");
    }

    // --- variant_name edge cases ---

    #[test]
    fn a_handler_name_of_only_underscores_generates_no_route_variant() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn ___(&self) -> StatusCode {
                    home()
                }
            }
        })
        .expect_err("a handler name made only of underscores generates an empty route variant");
        assert!(
            error.to_string().contains("service handler names must generate a route variant"),
            "{error}"
        );
    }

    // --- TransformAttr trailing-comma-with-no-handlers edge case ---

    #[test]
    fn a_handler_argument_named_like_a_generated_identifier_is_disambiguated() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, __routerama_request: Query) -> StatusCode {
                    response(__routerama_request)
                }
            }
        })
        .expect("a handler argument colliding with a generated identifier name is disambiguated");
        let code = generated.to_string();
        assert!(code.contains("self . home (__routerama_request)"), "{code}");
    }

    // --- kind_matches: an existing Direct dispatch compared against a multi-candidate overlap group ---

    #[test]
    fn a_body_extractor_wrapping_a_reference_type_is_rewritten_recursively() {
        let generated = expand_fixed_router(
            parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(POST, "/")]
                    async fn create(&self, #[body] value: Wrapper<&HeaderMap>) -> StatusCode {
                        response(value)
                    }
                }
            },
        )
        .expect("a body extractor generic over a reference type expands");
        let code = generated.to_string();
        assert!(code.contains("Wrapper < & 'static HeaderMap >"), "{code}");
    }

    #[test]
    fn a_body_extractor_referencing_a_higher_ranked_lifetime_is_left_unrewritten() {
        let generated = expand_fixed_router(
            parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(POST, "/")]
                    async fn create(&self, #[body] value: Box<dyn for<'a> SomeTrait<&'a HeaderMap>>) -> StatusCode {
                        response(value)
                    }
                }
            },
        )
        .expect("a body extractor bounded by a higher-ranked trait bound expands");
        let code = generated.to_string();
        assert!(code.contains("for < 'a > SomeTrait < & 'a HeaderMap >"), "{code}");
    }
}
