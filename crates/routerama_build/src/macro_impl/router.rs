// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementation of `#[router]`.

use alloc::string::{String, ToString as _};
use alloc::vec::Vec;
use alloc::{format, vec};

use http_path_template::{Grammar, PathTemplate};
use proc_macro2::{Ident, Span, TokenStream as TokenStream2, TokenTree};
use quote::{format_ident, quote};
use syn::spanned::Spanned as _;
use syn::visit::Visit as _;
use syn::visit_mut::VisitMut as _;
use syn::{
    Attribute, Error, FnArg, GenericArgument, ImplItem, ImplItemFn, ItemEnum, ItemImpl, Lifetime, LitByteStr, LitStr, Pat, PathArguments,
    ReturnType, Token, TraitBound, Type, WherePredicate,
};

use super::runtime::{RuntimeCapability, response_path, runtime_path};
use super::{
    RouteAttr, RouteDeclaration, RoutePredicates, RoutePriority, RouteTarget, StaticHeader, StaticHeaderOperation, differing_static_header,
    resolver, route_declaration, same_static_headers,
};
use crate::trie::{Node, VarPlan, build_trie, capture_field_names, depth_limit_error};
use crate::{Route, route_field_name};

struct Handler {
    method: Ident,
    variant: Ident,
    kind: HandlerKind,
    route_attrs: Vec<Attribute>,
    predicates: RoutePredicates,
    static_headers: Vec<StaticHeader>,
    captures: Vec<(Ident, Type)>,
    arguments: Vec<Argument>,
    response_type: Type,
    borrows_path: bool,
}

struct DispatchArm {
    variant: Ident,
    route_attrs: Vec<Attribute>,
    captures: Vec<(Ident, Type)>,
    kind: DispatchKind,
}

enum DispatchKind {
    Direct(usize),
    Overlap(Vec<usize>),
}

#[derive(Clone, Copy)]
enum DispatchResponse {
    Concrete,
    Mounted,
    MountedHeterogeneous,
}

/// How a generated stage leaves the entry once it owns a response.
///
/// Stages return directly unless a router-wide `#[after]` needs a common
/// response epilogue.
#[derive(Clone, Copy)]
enum DispatchExit {
    Return,
    Break,
}

/// The response shape and exit form shared by every stage of one entry.
#[derive(Clone, Copy)]
struct DispatchBoundary {
    response: DispatchResponse,
    exit: DispatchExit,
}

impl DispatchBoundary {
    /// Emits the statement one stage uses to leave the dispatch with `value`.
    ///
    /// The labeled form binds the response first so the `break` value can never
    /// be mistaken for a labeled expression, whatever the response tokens are.
    fn exit(self, value: &TokenStream2) -> TokenStream2 {
        match self.exit {
            DispatchExit::Return => quote! { return #value; },
            DispatchExit::Break => {
                let label = dispatch_label();
                quote! {
                    {
                        let __routerama_exit = #value;
                        break #label __routerama_exit;
                    }
                }
            }
        }
    }
}

/// The label of the block that wraps a dispatch whose responses are observed by
/// a generated-wide `#[after]` interceptor.
fn dispatch_label() -> Lifetime {
    Lifetime::new("'__routerama_dispatch", Span::call_site())
}

struct StaticRouteEntry {
    handler: usize,
    attribute: Attribute,
    route: Route,
    capture_keys: Vec<String>,
    priority: Option<RoutePriority>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExtractionKind {
    Parts,
    Body,
}

struct CatchBinding {
    kind: ExtractionKind,
    extractor_key: String,
    catcher: usize,
}

#[derive(Default)]
struct RoutingPolicy {
    fallback: Option<FallbackPolicy>,
    catchers: Vec<CatcherPolicy>,
    bindings: Vec<CatchBinding>,
    befores: Vec<BeforeInterceptor>,
    afters: Vec<AfterInterceptor>,
    transforms: Vec<TransformInterceptor>,
}

impl RoutingPolicy {
    /// Router-wide request interceptors run at every entry, before routing.
    fn router_wide_befores(&self) -> impl Iterator<Item = &BeforeInterceptor> {
        self.befores.iter().filter(|before| before.handlers.is_none())
    }

    /// Per-handler request interceptors run after route selection.
    fn handler_befores(&self, handler: &Ident) -> impl Iterator<Item = &BeforeInterceptor> {
        self.befores.iter().filter(move |before| {
            before
                .handlers
                .as_ref()
                .is_some_and(|names| names.iter().any(|name| name == handler))
        })
    }

    /// Response interceptors that observe every generated response. They run
    /// outermost, at the entry, after any per-handler ones.
    fn generated_wide_afters(&self) -> impl Iterator<Item = &AfterInterceptor> {
        self.afters.iter().filter(|after| after.handlers.is_none())
    }

    /// Per-handler response interceptors run innermost.
    fn handler_afters(&self, handler: &Ident) -> impl Iterator<Item = &AfterInterceptor> {
        self.afters.iter().filter(move |after| {
            after
                .handlers
                .as_ref()
                .is_some_and(|names| names.iter().any(|name| name == handler))
        })
    }

    /// The single terminal body transform selected for a handler, if any.
    fn transform_for(&self, handler: &Ident) -> Option<&TransformInterceptor> {
        self.transforms
            .iter()
            .find(|transform| transform.handlers.iter().any(|name| name == handler))
    }

    /// The concrete request body a handler's `#[body]` parameter extracts from:
    /// a transform replacement when one exists, otherwise the transport body.
    fn body_input_for(&self, handler: &Ident, body_type: &Ident) -> TokenStream2 {
        self.transform_for(handler)
            .and_then(|transform| transform.replacement_body.as_ref())
            .map_or_else(|| quote! { #body_type }, |replacement| quote! { #replacement })
    }

    /// True when a `#[before]` interceptor may mutate the request head, so the
    /// entry must bind it mutably. Transforms read the head immutably.
    fn mutates_request(&self) -> bool {
        !self.befores.is_empty()
    }
}

struct FallbackPolicy {
    method: Ident,
    response_type: Type,
}

struct CatcherPolicy {
    method: Ident,
    rejection_type: Type,
    extractor_type: Option<Type>,
    response_type: Type,
    span: Span,
}

/// A generated `#[before]` request interceptor.
struct BeforeInterceptor {
    method: Ident,
    response_type: Type,
    /// `None` is router-wide; `Some` lists the handlers it guards.
    handlers: Option<Vec<Ident>>,
}

/// A generated `#[after]` response interceptor.
struct AfterInterceptor {
    method: Ident,
    /// `None` observes every generated response; `Some` lists the handlers it
    /// post-processes.
    handlers: Option<Vec<Ident>>,
}

/// A generated `#[transform]` terminal request-body interceptor.
struct TransformInterceptor {
    method: Ident,
    mode: TransformMode,
    handlers: Vec<Ident>,
    /// `Some` produces a replacement body; `None` consumes the body.
    ///
    /// A streaming transform's replacement already names the generated
    /// transport body type, because the interceptor's generic body parameter is
    /// substituted when the signature is validated.
    replacement_body: Option<Type>,
    /// Streaming method predicates with its body parameter rewritten to the
    /// generated transport body type.
    body_bounds: Vec<WherePredicate>,
    response_type: Type,
    span: Span,
}

/// How a `#[transform]` interceptor takes ownership of the request body.
enum TransformMode {
    /// `#[transform(limit = N, ...)]` collects the body into bounded
    /// `bytes::Bytes` before calling the interceptor.
    Buffered { limit: syn::Expr },
    /// `#[transform(stream, ...)]` moves the transport body into an interceptor
    /// generic over it, so nothing is buffered by the framework.
    Streaming,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HandlerKind {
    Static,
    Dynamic,
}

enum Argument {
    Capture(Ident),
    Parts(Ident, Type),
    Body(Ident, Type),
}

struct GeneratedIdents {
    request: Ident,
    state: Ident,
    parts: Ident,
    body: Ident,
    route: Ident,
    response: Ident,
    failure: Ident,
}

struct RouteContract {
    bounds: Vec<TokenStream2>,
}

struct SharedState {
    ty: Type,
    generic: Option<Ident>,
}

struct ResponseBodyModel {
    body: Ident,
    projection: Ident,
    error: Ident,
    sources: Vec<ResponseSource>,
    heterogeneous_data: bool,
}

struct ResponseSource {
    key: String,
    variant: Ident,
    body_type: Ident,
    error_type: Ident,
    label: String,
}

struct ParentModulePaths;

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

#[cfg(test)]
pub(crate) fn expand(item: ItemImpl, fixed_state: Option<Type>, erased_mounts: bool, tower_adapter: bool) -> syn::Result<TokenStream2> {
    expand_with_data(item, fixed_state, erased_mounts, tower_adapter, false)
}

#[expect(
    clippy::too_many_lines,
    reason = "router expansion keeps signature validation and all generated service components in one ordered pipeline"
)]
pub(crate) fn expand_with_data(
    mut item: ItemImpl,
    fixed_state: Option<Type>,
    erased_mounts: bool,
    tower_adapter: bool,
    heterogeneous_data: bool,
) -> syn::Result<TokenStream2> {
    validate_impl(&item)?;
    #[cfg(not(feature = "tower"))]
    if tower_adapter {
        return Err(Error::new(
            item.impl_token.span(),
            "`tower` requires Routerama's `tower` Cargo feature",
        ));
    }
    if erased_mounts && fixed_state.is_none() {
        return Err(Error::new(
            item.impl_token.span(),
            "`erased_mounts` requires a fixed `state = StateType` router contract",
        ));
    }
    let service_name = service_name(&item.self_ty)?;
    let module_name = format_ident!("__routerama_{}", service_name, span = service_name.span());
    let route_name = format_ident!("{}Route", service_name, span = service_name.span());
    let route_path = quote! { #module_name::#route_name };

    let mut policy = parse_policy(&item)?;
    let mut handlers = Vec::new();
    for impl_item in &item.items {
        match impl_item {
            ImplItem::Fn(method) if has_route_attr(&method.attrs) => {
                if has_policy_attr(&method.attrs) {
                    return Err(Error::new(
                        method.sig.ident.span(),
                        "a method cannot be both a route handler and a routing policy method",
                    ));
                }
                handlers.push(parse_handler(method)?);
            }
            ImplItem::Fn(_) => {}
            other if has_route_attr(other.attrs()) || has_policy_attr(other.attrs()) => {
                return Err(Error::new(
                    other.span(),
                    "route and routing-policy attributes may annotate methods only",
                ));
            }
            _ => {}
        }
    }
    if handlers.is_empty() {
        return Err(Error::new(
            item.impl_token.span(),
            "`#[router]` requires at least one `#[route]` handler",
        ));
    }
    validate_handlers(&handlers)?;
    bind_catchers(&mut policy, &handlers)?;
    bind_interceptors(&policy, &handlers)?;
    let dispatches = build_dispatch_arms(&handlers)?;
    let has_dynamic = handlers.iter().any(|handler| handler.kind == HandlerKind::Dynamic);
    validate_generated_method_names(&item, has_dynamic, erased_mounts, tower_adapter)?;

    let has_path_lifetime = handlers.iter().any(|handler| handler.borrows_path);
    let route_generics = has_path_lifetime.then(|| quote! { <'p> });
    let variants = dispatches.iter().map(|dispatch| {
        let attrs = &dispatch.route_attrs;
        let variant = &dispatch.variant;
        let fields = dispatch.captures.iter().map(|(name, ty)| {
            let mut ty = ty.clone();
            ParentModulePaths.visit_type_mut(&mut ty);
            quote! { #name: #ty }
        });
        if dispatch.captures.is_empty() {
            quote! {
                #(#attrs)*
                #variant
            }
        } else {
            quote! {
                #(#attrs)*
                #variant { #(#fields),* }
            }
        }
    });
    let route_item: ItemEnum = syn::parse2(quote! {
        pub(super) enum #route_name #route_generics {
            #(#variants),*
        }
    })?;
    let generated_resolver = resolver::expand_named(route_item, None, RuntimeCapability::Route)?;

    let generated_idents = generated_idents(&handlers);
    let shared_state = shared_state(fixed_state);
    let runtime = runtime_path(RuntimeCapability::Route);
    let response_runtime = response_path();
    let response_body = response_body_model(&handlers, &dispatches, &policy, &service_name, heterogeneous_data);
    let generated_response_body = response_body_definition(&response_body, &runtime, &response_runtime);
    let generated_state_validation = fixed_state_validation(&item, &handlers, &policy, &shared_state, &runtime, &response_runtime)?;
    let resolver_module = encapsulated_resolver(&module_name, &generated_resolver, &generated_response_body);
    if let Some(validation) = generated_state_validation {
        item.items.push(validation);
    }

    for impl_item in &mut item.items {
        if let ImplItem::Fn(method) = impl_item {
            method.attrs.retain(|attribute| !attribute.path().is_ident("route"));
            method.attrs.retain(|attribute| {
                !attribute.path().is_ident("fallback")
                    && !attribute.path().is_ident("catch")
                    && !attribute.path().is_ident("before")
                    && !attribute.path().is_ident("after")
                    && !attribute.path().is_ident("transform")
            });
            for input in &mut method.sig.inputs {
                match input {
                    FnArg::Receiver(receiver) => {
                        receiver.attrs.retain(|attribute| !is_parameter_marker(attribute));
                    }
                    FnArg::Typed(input) => {
                        input.attrs.retain(|attribute| !is_parameter_marker(attribute));
                    }
                }
            }
        }
    }
    let service_api = if has_dynamic {
        dynamic_service_api(
            &mut item,
            &handlers,
            &dispatches,
            &policy,
            &service_name,
            &module_name,
            &route_name,
            &route_path,
            &generated_idents,
            &shared_state,
            &response_body,
            erased_mounts,
            tower_adapter,
            &runtime,
            &response_runtime,
        )?
    } else {
        item.items.push(static_route_method(
            &handlers,
            &dispatches,
            &policy,
            &module_name,
            &route_path,
            &generated_idents,
            &shared_state,
            &response_body,
            &runtime,
            &response_runtime,
        )?);
        #[cfg(feature = "tower")]
        if tower_adapter {
            item.items.push(static_tower_service_method(
                &handlers,
                &policy,
                &generated_idents,
                &shared_state,
                response_body.heterogeneous_data,
                &runtime,
                &response_runtime,
            )?);
        }
        if erased_mounts {
            item.items.push(static_mounted_route_method(
                &handlers,
                &dispatches,
                &policy,
                &module_name,
                &route_path,
                &generated_idents,
                &shared_state,
                &response_body,
                &runtime,
                &response_runtime,
            )?);
        }
        quote! {}
    };
    #[cfg(feature = "tower")]
    let tower_route_impl = if tower_adapter && !has_dynamic {
        static_tower_route_impl(
            &handlers,
            &policy,
            &item.self_ty,
            &shared_state,
            response_body.heterogeneous_data,
            &runtime,
            &response_runtime,
        )?
    } else {
        TokenStream2::new()
    };
    #[cfg(not(feature = "tower"))]
    let tower_route_impl = TokenStream2::new();
    Ok(quote! {
        #resolver_module
        #service_api
        #item
        #tower_route_impl
    })
}

fn encapsulated_resolver(module_name: &Ident, generated_resolver: &TokenStream2, generated_response_body: &TokenStream2) -> TokenStream2 {
    quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        mod #module_name {
            #[allow(unused_imports)]
            use super::*;

            #generated_resolver
            #generated_response_body
        }
    }
}

const ROUTING_RESPONSE_KEY: &str = "routerama:routing";
const PREDICATE_RESPONSE_KEY: &str = "routerama:predicate";
const FALLBACK_RESPONSE_KEY: &str = "routerama:fallback";

impl ResponseBodyModel {
    fn variant(&self, key: &str) -> &Ident {
        &self
            .sources
            .iter()
            .find(|source| source.key == key)
            .expect("every emitted response source is registered before dispatch generation")
            .variant
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the response source model enumerates every handler, catcher, interceptor, and transform body in one pass"
)]
fn response_body_model(
    handlers: &[Handler],
    dispatches: &[DispatchArm],
    policy: &RoutingPolicy,
    service_name: &Ident,
    heterogeneous_data: bool,
) -> ResponseBodyModel {
    let mut model = ResponseBodyModel {
        body: format_ident!("{}ResponseBody", service_name, span = service_name.span()),
        projection: format_ident!("{}ResponseBodyProjection", service_name, span = service_name.span()),
        error: format_ident!("{}ResponseBodyError", service_name, span = service_name.span()),
        sources: Vec::new(),
        heterogeneous_data,
    };
    if let Some(fallback) = &policy.fallback {
        let response_type = &fallback.response_type;
        add_response_source(
            &mut model,
            FALLBACK_RESPONSE_KEY.to_string(),
            format!("routing fallback response `{}`", quote! { #response_type }),
        );
    } else {
        add_response_source(&mut model, ROUTING_RESPONSE_KEY.to_string(), "routing failure".to_string());
    }
    if policy.fallback.is_none()
        && dispatches.iter().any(|dispatch| match &dispatch.kind {
            DispatchKind::Direct(index) => !handlers[*index].predicates.is_empty(),
            DispatchKind::Overlap(candidates) => candidates.last().is_some_and(|index| !handlers[*index].predicates.is_empty()),
        })
    {
        add_response_source(
            &mut model,
            PREDICATE_RESPONSE_KEY.to_string(),
            "route predicate rejection".to_string(),
        );
    }
    for handler in handlers {
        let response_type = &handler.response_type;
        add_response_source(
            &mut model,
            handler_response_key(response_type),
            format!("handler response `{}`", quote! { #response_type }),
        );
        for argument in &handler.arguments {
            match argument {
                Argument::Parts(name, ty) => {
                    if let Some(catcher) = catcher_for(policy, ExtractionKind::Parts, ty) {
                        add_catcher_response_source(&mut model, catcher);
                    } else {
                        add_response_source(
                            &mut model,
                            parts_rejection_key(ty),
                            format!(
                                "request-parts rejection `{}` first used by `{}.{name}`",
                                quote! { #ty },
                                handler.method
                            ),
                        );
                    }
                }
                Argument::Body(name, ty) => {
                    if let Some(catcher) = catcher_for(policy, ExtractionKind::Body, ty) {
                        add_catcher_response_source(&mut model, catcher);
                    } else {
                        let body_input = policy.body_input_for(&handler.method, &generated_body_type());
                        add_response_source(
                            &mut model,
                            body_rejection_key(ty, &body_input),
                            format!(
                                "request-body rejection `{}` from `{}` first used by `{}.{name}`",
                                quote! { #ty },
                                body_input,
                                handler.method
                            ),
                        );
                    }
                }
                Argument::Capture(_) => {}
            }
        }
    }

    for before in &policy.befores {
        let response_type = &before.response_type;
        add_response_source(
            &mut model,
            interceptor_response_key(&before.method),
            format!(
                "interceptor `{}` short-circuit response `{}`",
                before.method,
                quote! { #response_type }
            ),
        );
    }
    for transform in &policy.transforms {
        let response_type = &transform.response_type;
        add_response_source(
            &mut model,
            interceptor_response_key(&transform.method),
            format!(
                "transform `{}` short-circuit response `{}`",
                transform.method,
                quote! { #response_type }
            ),
        );
        // A streaming transform never buffers, so it has no buffering rejection
        // to add to the response body sum.
        if matches!(transform.mode, TransformMode::Buffered { .. }) {
            add_response_source(
                &mut model,
                transform_buffer_key(&transform.method),
                format!("transform `{}` request-body buffering rejection", transform.method),
            );
        }
    }

    model
}

/// The generic transport request-body parameter of every generated entry.
///
/// A streaming `#[transform]` substitutes this name for its own generic body
/// parameter, so the handler's `#[body]` extraction binds to the exact wrapper
/// type the interceptor returns.
fn generated_body_type() -> Ident {
    format_ident!("__RouteramaBody")
}

fn interceptor_response_key(method: &Ident) -> String {
    format!("interceptor:{method}")
}

fn transform_buffer_key(method: &Ident) -> String {
    format!("transform-buffer:{method}")
}

fn add_catcher_response_source(model: &mut ResponseBodyModel, catcher: &CatcherPolicy) {
    let response_type = &catcher.response_type;
    add_response_source(
        model,
        catcher_response_key(catcher),
        format!("extractor catcher response `{}`", quote! { #response_type }),
    );
}

fn add_response_source(model: &mut ResponseBodyModel, key: String, label: String) {
    if model.sources.iter().any(|source| source.key == key) {
        return;
    }
    let index = model.sources.len();
    model.sources.push(ResponseSource {
        key,
        variant: format_ident!("Source{index}"),
        body_type: format_ident!("__RouteramaResponseBody{index}"),
        error_type: format_ident!("__RouteramaResponseError{index}"),
        label,
    });
}

fn handler_response_key(response_type: &Type) -> String {
    format!("handler:{}", quote! { #response_type })
}

fn parts_rejection_key(extractor_type: &Type) -> String {
    format!("parts-rejection:{}", quote! { #extractor_type })
}

fn body_rejection_key(extractor_type: &Type, body_input: &TokenStream2) -> String {
    format!("body-rejection:{}@{}", quote! { #extractor_type }, body_input)
}

fn catcher_response_key(catcher: &CatcherPolicy) -> String {
    format!("catcher:{}", catcher.method)
}

fn shared_state(fixed_state: Option<Type>) -> SharedState {
    if let Some(ty) = fixed_state {
        return SharedState { ty, generic: None };
    }

    let generic = Ident::new("__RouteramaState", Span::call_site());
    let ty = syn::parse_quote!(#generic);
    SharedState {
        ty,
        generic: Some(generic),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the fixed-state alias, parts/body witnesses, and response assertions form one compile-only contract"
)]
fn fixed_state_validation(
    item: &ItemImpl,
    handlers: &[Handler],
    policy: &RoutingPolicy,
    shared_state: &SharedState,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> syn::Result<Option<ImplItem>> {
    if shared_state.generic.is_some() {
        return Ok(None);
    }

    let mut validation_method_name = "__routerama_validate_fixed_state".to_string();
    while item.items.iter().any(|impl_item| match impl_item {
        ImplItem::Const(constant) => constant.ident == validation_method_name,
        ImplItem::Fn(method) => method.sig.ident == validation_method_name,
        _ => false,
    }) {
        validation_method_name.insert(0, '_');
    }
    let validation_method = Ident::new(&validation_method_name, Span::call_site());
    let source_state_type = &shared_state.ty;
    let mut state_alias_name = "__RouteramaFixedStateContract".to_string();
    while token_stream_contains_ident(quote! { #source_state_type }, &state_alias_name)
        || handlers.iter().any(|handler| {
            handler
                .captures
                .iter()
                .map(|(_, ty)| ty)
                .chain(handler.arguments.iter().filter_map(|argument| match argument {
                    Argument::Parts(_, ty) | Argument::Body(_, ty) => Some(ty),
                    Argument::Capture(_) => None,
                }))
                .chain(core::iter::once(&handler.response_type))
                .any(|ty| token_stream_contains_ident(quote! { #ty }, &state_alias_name))
        })
        || policy
            .fallback
            .iter()
            .map(|fallback| &fallback.response_type)
            .chain(policy.catchers.iter().flat_map(|catcher| {
                core::iter::once(&catcher.rejection_type)
                    .chain(catcher.extractor_type.iter())
                    .chain(core::iter::once(&catcher.response_type))
            }))
            .any(|ty| token_stream_contains_ident(quote! { #ty }, &state_alias_name))
    {
        state_alias_name.insert(0, '_');
    }
    let state_alias = Ident::new(&state_alias_name, Span::call_site());
    let state_type = &shared_state.ty;
    let mut extractor_assertions = Vec::new();
    let mut rendered_extractors = Vec::new();
    for handler in handlers {
        for argument in &handler.arguments {
            let assertion = match argument {
                Argument::Parts(_, ty) => {
                    let lifetime = Lifetime::new("'__routerama_witness", Span::call_site());
                    let mut extractor_type = ty.clone();
                    RequestPartsLifetime::new(&lifetime).rewrite(&mut extractor_type)?;
                    let rejection = if let Some(catcher) = catcher_for(policy, ExtractionKind::Parts, ty) {
                        let rejection = &catcher.rejection_type;
                        quote! { #rejection }
                    } else {
                        quote! { _ }
                    };
                    quote! {
                        {
                            fn __routerama_assert_parts<__RouteramaRejection>()
                            where
                                __RouteramaRejection:
                                    #response_runtime::IntoResponse + 'static,
                                <__RouteramaRejection as #response_runtime::IntoResponse>::Body:
                                    'static,
                                for<#lifetime> #extractor_type:
                                    #runtime::FromRequestParts<
                                        #lifetime,
                                        #state_alias,
                                        Rejection = __RouteramaRejection,
                                    >,
                            {}

                            __routerama_assert_parts::<#rejection>();
                        }
                    }
                }
                Argument::Body(_, ty) => {
                    let transform = policy.transform_for(&handler.method);
                    if transform.is_some_and(|transform| matches!(transform.mode, TransformMode::Streaming)) {
                        // A streaming replacement names the generic transport
                        // body, so it cannot be checked eagerly here; the entry
                        // where-clause carries its `FromRequestBody` bound.
                        continue;
                    }
                    if let Some(replacement) = transform.and_then(|transform| transform.replacement_body.as_ref()) {
                        let mut extractor_type = ty.clone();
                        StaticAnonymousLifetimes::default().visit_type_mut(&mut extractor_type);
                        let mut replacement_type = replacement.clone();
                        StaticAnonymousLifetimes::default().visit_type_mut(&mut replacement_type);
                        quote! {
                            __routerama_assert_concrete_body::<#extractor_type, #state_alias, #replacement_type>();
                        }
                    } else {
                        let mut extractor_type = ty.clone();
                        StaticAnonymousLifetimes::default().visit_type_mut(&mut extractor_type);
                        let rejection = if let Some(catcher) = catcher_for(policy, ExtractionKind::Body, ty) {
                            let rejection = &catcher.rejection_type;
                            quote! { #rejection }
                        } else {
                            default_body_witness_rejection(ty, runtime)
                        };
                        quote! {
                            __routerama_assert_body::<#extractor_type, #state_alias, #rejection>();
                        }
                    }
                }
                Argument::Capture(_) => continue,
            };
            let text = assertion.to_string();
            if !rendered_extractors.contains(&text) {
                rendered_extractors.push(text);
                extractor_assertions.push(assertion);
            }
        }
    }

    let mut response_assertions = Vec::new();
    let mut rendered_responses = Vec::new();
    for response_type in handlers
        .iter()
        .map(|handler| &handler.response_type)
        .chain(policy.fallback.iter().map(|fallback| &fallback.response_type))
        .chain(policy.catchers.iter().map(|catcher| &catcher.response_type))
        .chain(policy.befores.iter().map(|before| &before.response_type))
        .chain(policy.transforms.iter().map(|transform| &transform.response_type))
    {
        let assertion = quote! {
            __routerama_assert_response::<#response_type>();
        };
        let text = assertion.to_string();
        if !rendered_responses.contains(&text) {
            rendered_responses.push(text);
            response_assertions.push(assertion);
        }
    }

    let validation: ImplItem = syn::parse2(quote! {
        #[allow(
            dead_code,
            private_bounds,
            reason = "the fixed-state alias and witnesses validate the complete handler contract when the service is defined"
        )]
        fn #validation_method() {
            type #state_alias = #state_type;

            fn __routerama_assert_body<
                __RouteramaExtractor,
                __RouteramaWitnessInput,
                __RouteramaRejection,
            >()
            where
                __RouteramaWitnessInput: ?::core::marker::Sized,
                __RouteramaRejection: #response_runtime::IntoResponse,
                __RouteramaExtractor:
                    #runtime::BodyStateWitness<
                        __RouteramaWitnessInput,
                        __RouteramaRejection,
                    >
                    + #runtime::FromRequestBody<
                        __RouteramaWitnessInput,
                        <__RouteramaExtractor as #runtime::BodyStateWitness<
                            __RouteramaWitnessInput,
                            __RouteramaRejection
                        >>::RequestBody,
                        Rejection = __RouteramaRejection,
                    >,
            {}

            fn __routerama_assert_response<__RouteramaResponse>()
            where
                __RouteramaResponse: #response_runtime::IntoResponse,
            {}

            fn __routerama_assert_concrete_body<
                __RouteramaExtractor,
                __RouteramaConcreteBodyState,
                __RouteramaReplacementBody,
            >()
            where
                __RouteramaConcreteBodyState: ?::core::marker::Sized,
                __RouteramaExtractor: #runtime::FromRequestBody<__RouteramaConcreteBodyState, __RouteramaReplacementBody>,
            {}

            #(#extractor_assertions)*
            #(#response_assertions)*
        }
    })?;

    Ok(Some(validation))
}

fn route_generics(body_type: &Ident, shared_state: &SharedState) -> TokenStream2 {
    let mut parameters = vec![quote! { #body_type }];
    if let Some(state_type) = &shared_state.generic {
        parameters.push(quote! { #state_type: ?::core::marker::Sized });
    }
    quote! { <#(#parameters),*> }
}

/// The generic parameters of a generated mounted entry.
///
/// This is [`route_generics`] plus the [`MountDelegate`] parameter, which the
/// entry is generic over so one method serves every mount table.
fn mounted_route_generics(body_type: &Ident, shared_state: &SharedState) -> TokenStream2 {
    let mounts_type = mount_delegate_type();
    let mut parameters = vec![quote! { #body_type }];
    if let Some(state_type) = &shared_state.generic {
        parameters.push(quote! { #state_type: ?::core::marker::Sized });
    }
    parameters.push(quote! { #mounts_type });
    quote! { <#(#parameters),*> }
}

fn response_return_type(
    body_type: &Ident,
    shared_state: &SharedState,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> TokenStream2 {
    let mut captures = vec![quote! { #body_type }];
    if let Some(state_type) = &shared_state.generic {
        captures.push(quote! { #state_type });
    }
    if heterogeneous_data {
        quote! {
            #response_runtime::Response<
                impl #runtime::http_body::Body<
                    Error = impl ::core::error::Error + use<#(#captures),*>,
                > + use<#(#captures),*>
            >
        }
    } else {
        quote! {
            #response_runtime::Response<
                impl #runtime::http_body::Body<
                    Data = #runtime::bytes::Bytes,
                    Error = impl ::core::error::Error + use<#(#captures),*>,
                > + use<#(#captures),*>
            >
        }
    }
}

#[cfg(feature = "tower")]
fn tower_service_return_type(
    body_type: &Ident,
    handles: &[&Ident],
    shared_state: &SharedState,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> TokenStream2 {
    let mut captures = vec![quote! { #body_type }];
    if let Some(state_type) = &shared_state.generic {
        captures.push(quote! { #state_type });
    }
    captures.extend(handles.iter().map(|handle| quote! { #handle }));
    let data = if heterogeneous_data {
        TokenStream2::new()
    } else {
        quote! { Data = #runtime::bytes::Bytes, }
    };
    quote! {
        impl #runtime::tower_service::Service<
                #runtime::http::Request<#body_type>,
                Response = #response_runtime::Response<
                    impl #runtime::http_body::Body<
                        #data
                        Error = impl ::core::error::Error
                            + ::core::marker::Send
                            + ::core::marker::Sync
                            + 'static
                            + use<#(#captures),*>,
                    > + ::core::marker::Send + 'static + use<#(#captures),*>
                >,
                Error = ::core::convert::Infallible,
                Future: ::core::marker::Send,
            > + ::core::clone::Clone
                + ::core::marker::Send
                + ::core::marker::Sync
                + 'static
                + use<#(#captures),*>
    }
}

#[cfg(feature = "tower")]
fn tower_route_response_type(heterogeneous_data: bool, runtime: &TokenStream2, response_runtime: &TokenStream2) -> TokenStream2 {
    let data = if heterogeneous_data {
        TokenStream2::new()
    } else {
        quote! { Data = #runtime::bytes::Bytes, }
    };
    quote! {
        #response_runtime::Response<
            impl #runtime::http_body::Body<
                #data
                Error = impl ::core::error::Error
                    + ::core::marker::Send
                    + ::core::marker::Sync
                    + 'static,
            > + ::core::marker::Send + 'static
        >
    }
}

/// The generic mount-delegate parameter of a generated mounted entry.
///
/// Making the entry generic over [`MountDelegate`] rather than naming
/// `ErasedMountRouter` is what lets one generated method serve both the local
/// and the `Send` mount routers: the delegate's response body is an associated
/// type, so the auto traits of the opaque return type follow whichever router
/// the caller passed.
fn mount_delegate_type() -> Ident {
    format_ident!("__RouteramaMounts")
}

/// The generated pieces a mounted entry needs to delegate through
/// [`MountDelegate`].
struct MountContract {
    /// The associated response body of the delegate.
    body: TokenStream2,
    /// The where-clause predicates the delegate must satisfy.
    bounds: Vec<TokenStream2>,
}

fn mount_contract(body_type: &Ident, shared_state: &SharedState, heterogeneous_data: bool, runtime: &TokenStream2) -> MountContract {
    let mounts_type = mount_delegate_type();
    let state_type = &shared_state.ty;
    let body = quote! {
        <#mounts_type as #runtime::MountDelegate<#body_type, #state_type>>::Body
    };
    let data_bound = if heterogeneous_data {
        TokenStream2::new()
    } else {
        quote! { Data = #runtime::bytes::Bytes, }
    };
    MountContract {
        bounds: vec![
            quote! { #mounts_type: #runtime::MountDelegate<#body_type, #state_type> },
            quote! {
                #body: #runtime::http_body::Body<
                    #data_bound
                    Error: ::core::error::Error + 'static,
                >
            },
        ],
        body,
    }
}

fn mounted_response_return_type(
    body_type: &Ident,
    shared_state: &SharedState,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> TokenStream2 {
    let mounts_type = mount_delegate_type();
    let state_type = &shared_state.ty;
    let mut captures = vec![quote! { #body_type }];
    if let Some(state_type) = &shared_state.generic {
        captures.push(quote! { #state_type });
    }
    captures.push(quote! { #mounts_type });
    let mount_body = quote! {
        <#mounts_type as #runtime::MountDelegate<#body_type, #state_type>>::Body
    };

    if heterogeneous_data {
        quote! {
            #response_runtime::Response<
                #response_runtime::DataEitherBody<
                    impl #runtime::http_body::Body<
                        Error = impl ::core::error::Error + use<#(#captures),*>,
                    > + use<#(#captures),*>,
                    #mount_body,
                >
            >
        }
    } else {
        quote! {
            #response_runtime::Response<
                #response_runtime::EitherBody<
                    impl #runtime::http_body::Body<
                        Data = #runtime::bytes::Bytes,
                        Error = impl ::core::error::Error + use<#(#captures),*>,
                    > + use<#(#captures),*>,
                    #mount_body,
                >
            >
        }
    }
}

#[cfg(feature = "tower")]
fn static_tower_route_impl(
    handlers: &[Handler],
    policy: &RoutingPolicy,
    service_type: &Type,
    shared_state: &SharedState,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> syn::Result<TokenStream2> {
    let body_type = generated_body_type();
    let service_handle = generated_ident("__RouteramaServiceHandle", handlers);
    let state_handle = generated_ident("__RouteramaSharedHandle", handlers);
    let state_type = &shared_state.ty;
    let service = generated_ident("__routerama_exact_service", handlers);
    let request = generated_ident("__routerama_exact_request", handlers);
    let state = generated_ident("__routerama_exact_state", handlers);
    let contract = route_contract(
        handlers,
        policy,
        &body_type,
        shared_state,
        heterogeneous_data,
        runtime,
        response_runtime,
    )?;
    let bounds = &contract.bounds;
    let mut parameters = vec![quote! { #body_type }, quote! { #service_handle }, quote! { #state_handle }];
    if let Some(generic_state) = &shared_state.generic {
        parameters.insert(1, quote! { #generic_state: ?::core::marker::Sized });
    }
    let response_type = tower_route_response_type(heterogeneous_data, runtime, response_runtime);
    let exact_route = if heterogeneous_data {
        format_ident!("GeneratedExactRouteData")
    } else {
        format_ident!("GeneratedExactRoute")
    };

    Ok(quote! {
        #[automatically_derived]
        impl<#(#parameters),*> #runtime::#exact_route<
                #body_type,
                #state_type,
                #service_handle,
                #state_handle,
            >
            for #service_type
        where
            #service_type: ::core::marker::Sync + 'static,
            #body_type: ::core::marker::Send + 'static,
            #state_type: ::core::marker::Sync + 'static,
            #service_handle:
                ::core::borrow::Borrow<#service_type>
                + ::core::marker::Send
                + 'static,
            #state_handle:
                ::core::borrow::Borrow<#state_type>
                + ::core::marker::Send
                + 'static,
            #(#bounds),*
        {
            fn route_exact(
                #service: #service_handle,
                #request: #runtime::http::Request<#body_type>,
                #state: #state_handle,
            ) -> impl ::core::future::Future<
                Output = #response_type,
            > + ::core::marker::Send + 'static
            {
                async move {
                    let #service = ::core::borrow::Borrow::<#service_type>::borrow(&#service);
                    let #state = ::core::borrow::Borrow::<#state_type>::borrow(&#state);
                    #service.route(#request, #state).await
                }
            }
        }
    })
}

#[cfg(feature = "tower")]
fn static_tower_service_method(
    handlers: &[Handler],
    _policy: &RoutingPolicy,
    generated: &GeneratedIdents,
    shared_state: &SharedState,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> syn::Result<ImplItem> {
    let body_type = generated_body_type();
    let service_handle = generated_ident("__RouteramaServiceHandle", handlers);
    let state_handle = generated_ident("__RouteramaSharedHandle", handlers);
    let state_type = &shared_state.ty;
    let request = &generated.request;
    let service = generated_ident("__routerama_service_handle", handlers);
    let state = generated_ident("__routerama_state_handle", handlers);
    let mut parameters = vec![quote! { #body_type }, quote! { #service_handle }, quote! { #state_handle }];
    if let Some(generic_state) = &shared_state.generic {
        parameters.insert(1, quote! { #generic_state: ?::core::marker::Sized });
    }
    let return_type = tower_service_return_type(
        &body_type,
        &[&service_handle, &state_handle],
        shared_state,
        heterogeneous_data,
        runtime,
        response_runtime,
    );
    let exact_route = if heterogeneous_data {
        format_ident!("GeneratedExactRouteData")
    } else {
        format_ident!("GeneratedExactRoute")
    };

    syn::parse2(quote! {
        /// Creates an allocation-free Tower adapter that preserves this
        /// service's exact generated response body.
        ///
        /// The service and state handles are cloned once per call. Owned
        /// values, shared references, and `Arc` handles are accepted when they
        /// implement the structural bounds below.
        #[must_use]
        #[allow(
            clippy::future_not_send,
            reason = "transport Send bounds remain structural behind the generated route contract"
        )]
        pub fn tower_service<#(#parameters),*>(
            #service: #service_handle,
            #state: #state_handle,
        ) -> #return_type
        where
            Self: #runtime::#exact_route<
                    #body_type,
                    #state_type,
                    #service_handle,
                    #state_handle,
                >
                + 'static,
            #body_type: ::core::marker::Send + 'static,
            #state_type: 'static,
            #service_handle:
                ::core::clone::Clone
                + ::core::marker::Send
                + ::core::marker::Sync
                + 'static,
            #state_handle:
                ::core::clone::Clone
                + ::core::marker::Send
                + ::core::marker::Sync
                + 'static,
        {
            #runtime::RouteService::new(
                #service,
                #state,
                |
                    #service: #service_handle,
                    #state: #state_handle,
                    #request: #runtime::http::Request<#body_type>,
                | {
                    <Self as #runtime::#exact_route<
                        #body_type,
                        #state_type,
                        #service_handle,
                        #state_handle,
                    >>::route_exact(
                        #service,
                        #request,
                        #state,
                    )
                },
            )
        }
    })
}

#[cfg(feature = "tower")]
#[expect(
    clippy::too_many_arguments,
    reason = "Tower generation needs the validated router, state, data-mode, and runtime paths"
)]
fn dynamic_tower_route_impl(
    handlers: &[Handler],
    policy: &RoutingPolicy,
    router_type: &Ident,
    service_type: &Type,
    shared_state: &SharedState,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> syn::Result<TokenStream2> {
    let body_type = generated_body_type();
    let router_handle = generated_ident("__RouteramaRouterHandle", handlers);
    let service_handle = generated_ident("__RouteramaServiceHandle", handlers);
    let state_handle = generated_ident("__RouteramaSharedHandle", handlers);
    let state_type = &shared_state.ty;
    let router = generated_ident("__routerama_exact_router", handlers);
    let service = generated_ident("__routerama_exact_service", handlers);
    let request = generated_ident("__routerama_exact_request", handlers);
    let state = generated_ident("__routerama_exact_state", handlers);
    let contract = route_contract(
        handlers,
        policy,
        &body_type,
        shared_state,
        heterogeneous_data,
        runtime,
        response_runtime,
    )?;
    let bounds = &contract.bounds;
    let mut parameters = vec![
        quote! { #body_type },
        quote! { #router_handle },
        quote! { #service_handle },
        quote! { #state_handle },
    ];
    if let Some(generic_state) = &shared_state.generic {
        parameters.insert(1, quote! { #generic_state: ?::core::marker::Sized });
    }
    let response_type = tower_route_response_type(heterogeneous_data, runtime, response_runtime);
    let exact_route = if heterogeneous_data {
        format_ident!("GeneratedExactConfiguredRouteData")
    } else {
        format_ident!("GeneratedExactConfiguredRoute")
    };

    Ok(quote! {
        #[automatically_derived]
        impl<#(#parameters),*>
            #runtime::#exact_route<
                #body_type,
                #service_type,
                #state_type,
                #router_handle,
                #service_handle,
                #state_handle,
            >
            for #router_type
        where
            #router_type: ::core::marker::Sync + 'static,
            #service_type: ::core::marker::Sync + 'static,
            #body_type: ::core::marker::Send + 'static,
            #state_type: ::core::marker::Sync + 'static,
            #router_handle:
                ::core::borrow::Borrow<#router_type>
                + ::core::marker::Send
                + 'static,
            #service_handle:
                ::core::borrow::Borrow<#service_type>
                + ::core::marker::Send
                + 'static,
            #state_handle:
                ::core::borrow::Borrow<#state_type>
                + ::core::marker::Send
                + 'static,
            #(#bounds),*
        {
            fn route_exact(
                #router: #router_handle,
                #service: #service_handle,
                #request: #runtime::http::Request<#body_type>,
                #state: #state_handle,
            ) -> impl ::core::future::Future<
                Output = #response_type,
            > + ::core::marker::Send + 'static
            {
                async move {
                    let #router = ::core::borrow::Borrow::<#router_type>::borrow(&#router);
                    let #service = ::core::borrow::Borrow::<#service_type>::borrow(&#service);
                    let #state = ::core::borrow::Borrow::<#state_type>::borrow(&#state);
                    #router.route(#service, #request, #state).await
                }
            }
        }
    })
}

#[cfg(feature = "tower")]
fn dynamic_tower_service_method(
    handlers: &[Handler],
    _policy: &RoutingPolicy,
    service_type: &Type,
    shared_state: &SharedState,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> TokenStream2 {
    let body_type = generated_body_type();
    let router_handle = generated_ident("__RouteramaRouterHandle", handlers);
    let service_handle = generated_ident("__RouteramaServiceHandle", handlers);
    let state_handle = generated_ident("__RouteramaSharedHandle", handlers);
    let state_type = &shared_state.ty;
    let router = generated_ident("__routerama_router_handle", handlers);
    let service = generated_ident("__routerama_service_handle", handlers);
    let state = generated_ident("__routerama_state_handle", handlers);
    let request = generated_ident("__routerama_tower_request", handlers);
    let mut parameters = vec![
        quote! { #body_type },
        quote! { #router_handle },
        quote! { #service_handle },
        quote! { #state_handle },
    ];
    if let Some(generic_state) = &shared_state.generic {
        parameters.insert(1, quote! { #generic_state: ?::core::marker::Sized });
    }
    let return_type = tower_service_return_type(
        &body_type,
        &[&router_handle, &service_handle, &state_handle],
        shared_state,
        heterogeneous_data,
        runtime,
        response_runtime,
    );
    let exact_route = if heterogeneous_data {
        format_ident!("GeneratedExactConfiguredRouteData")
    } else {
        format_ident!("GeneratedExactConfiguredRoute")
    };

    quote! {
        /// Creates an allocation-free Tower adapter that preserves this
        /// configured service's exact generated response body.
        ///
        /// The router, service, and state handles are cloned once per call.
        /// Owned values, shared references, and `Arc` handles are accepted
        /// when they implement the structural bounds below.
        #[must_use]
        #[allow(
            clippy::future_not_send,
            reason = "transport Send bounds remain structural behind the generated route contract"
        )]
        pub fn tower_service<#(#parameters),*>(
            #router: #router_handle,
            #service: #service_handle,
            #state: #state_handle,
        ) -> #return_type
        where
            Self: #runtime::#exact_route<
                    #body_type,
                    #service_type,
                    #state_type,
                    #router_handle,
                    #service_handle,
                    #state_handle,
                >
                + 'static,
            #service_type: ::core::marker::Sync + 'static,
            #body_type: ::core::marker::Send + 'static,
            #state_type: 'static,
            #router_handle:
                ::core::clone::Clone
                + ::core::marker::Send
                + ::core::marker::Sync
                + 'static,
            #service_handle:
                ::core::clone::Clone
                + ::core::marker::Send
                + ::core::marker::Sync
                + 'static,
            #state_handle:
                ::core::clone::Clone
                + ::core::marker::Send
                + ::core::marker::Sync
                + 'static,
        {
            #runtime::RouteService::new(
                (#router, #service),
                #state,
                |
                    (#router, #service): (#router_handle, #service_handle),
                    #state: #state_handle,
                    #request: #runtime::http::Request<#body_type>,
                | {
                    <Self as #runtime::#exact_route<
                        #body_type,
                        #service_type,
                        #state_type,
                        #router_handle,
                        #service_handle,
                        #state_handle,
                    >>::route_exact(
                        #router,
                        #service,
                        #request,
                        #state,
                    )
                },
            )
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the complete generated body and error sums are kept together so their variants cannot drift"
)]
fn response_body_definition(model: &ResponseBodyModel, runtime: &TokenStream2, response_runtime: &TokenStream2) -> TokenStream2 {
    let body = &model.body;
    let projection = &model.projection;
    let error = &model.error;
    let body_types: Vec<_> = model.sources.iter().map(|source| &source.body_type).collect();
    let error_types: Vec<_> = model.sources.iter().map(|source| &source.error_type).collect();

    let body_variants = model.sources.iter().map(|source| {
        let variant = &source.variant;
        let body_type = &source.body_type;
        quote! {
            #variant {
                #[pin]
                body: #body_type
            }
        }
    });
    let error_variants = model.sources.iter().map(|source| {
        let variant = &source.variant;
        let error_type = &source.error_type;
        quote! { #variant(#error_type) }
    });
    let debug_arms = model.sources.iter().map(|source| {
        let variant = &source.variant;
        let label = &source.label;
        quote! {
            Self::#variant(_) => f
                .debug_struct(stringify!(#error))
                .field("source", &#label)
                .finish_non_exhaustive()
        }
    });
    let display_arms = model.sources.iter().map(|source| {
        let variant = &source.variant;
        let label = format!("response body from {} failed", source.label);
        quote! { Self::#variant(_) => f.write_str(#label) }
    });
    let source_arms = model.sources.iter().map(|source| {
        let variant = &source.variant;
        quote! { Self::#variant(error) => ::core::option::Option::Some(error) }
    });
    let poll_arms = model.sources.iter().enumerate().map(|(index, source)| {
        let variant = &source.variant;
        let successful_frame = if model.heterogeneous_data && model.sources.len() > 1 {
            let mut mapped = if index + 1 == model.sources.len() {
                quote! { data }
            } else {
                quote! { #response_runtime::EitherData::Left(data) }
            };
            for _ in 0..index {
                mapped = quote! { #response_runtime::EitherData::Right(#mapped) };
            }
            quote! {
                frame.map_data(|data| #mapped)
            }
        } else {
            quote! { frame }
        };
        quote! {
            #projection::#variant { body } => {
                match #runtime::http_body::Body::poll_frame(body, cx) {
                    ::core::task::Poll::Ready(::core::option::Option::Some(::core::result::Result::Ok(frame))) => {
                        ::core::task::Poll::Ready(::core::option::Option::Some(::core::result::Result::Ok(
                            #successful_frame
                        )))
                    }
                    ::core::task::Poll::Ready(::core::option::Option::Some(::core::result::Result::Err(error))) => {
                        ::core::task::Poll::Ready(::core::option::Option::Some(::core::result::Result::Err(
                            #error::#variant(error)
                        )))
                    }
                    ::core::task::Poll::Ready(::core::option::Option::None) => {
                        ::core::task::Poll::Ready(::core::option::Option::None)
                    }
                    ::core::task::Poll::Pending => ::core::task::Poll::Pending,
                }
            }
        }
    });
    let end_stream_arms = model.sources.iter().map(|source| {
        let variant = &source.variant;
        quote! {
            Self::#variant { body } => #runtime::http_body::Body::is_end_stream(body)
        }
    });
    let size_hint_arms = model.sources.iter().map(|source| {
        let variant = &source.variant;
        quote! {
            Self::#variant { body } => #runtime::http_body::Body::size_hint(body)
        }
    });
    let body_errors = body_types.iter().map(|body_type| {
        quote! { <#body_type as #runtime::http_body::Body>::Error }
    });
    let body_bounds = if model.heterogeneous_data {
        quote! { #(#body_types: #runtime::http_body::Body),* }
    } else {
        quote! { #(#body_types: #runtime::http_body::Body<Data = #runtime::bytes::Bytes>),* }
    };
    let body_data = if model.heterogeneous_data {
        let mut types = body_types.iter().rev();
        let last = types.next().expect("every generated response body has at least one source");
        let mut data_type = quote! { <#last as #runtime::http_body::Body>::Data };
        for body_type in types {
            data_type = quote! {
                #response_runtime::EitherData<
                    <#body_type as #runtime::http_body::Body>::Data,
                    #data_type
                >
            };
        }
        data_type
    } else {
        quote! { #runtime::bytes::Bytes }
    };

    quote! {

        #runtime::pin_project! {
            #[project = #projection]
            #[allow(
                clippy::large_enum_variant,
                reason = "the sum intentionally stores each concrete body inline to avoid mandatory boxing"
            )]
            pub(super) enum #body<#(#body_types),*> {
                #(#body_variants),*
            }
        }

        #[allow(
            dead_code,
            reason = "error payloads are retained for ownership and auto-traits even when a caller never walks the source chain"
        )]
        pub(super) enum #error<#(#error_types),*> {
            #(#error_variants),*
        }

        impl<#(#error_types),*> ::core::fmt::Debug for #error<#(#error_types),*> {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self {
                    #(#debug_arms),*
                }
            }
        }

        impl<#(#error_types),*> ::core::fmt::Display for #error<#(#error_types),*> {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self {
                    #(#display_arms),*
                }
            }
        }

        impl<#(#error_types),*> ::core::error::Error for #error<#(#error_types),*>
        where
            #(#error_types: ::core::error::Error + 'static),*
        {
            fn source(&self) -> ::core::option::Option<&(dyn ::core::error::Error + 'static)> {
                match self {
                    #(#source_arms),*
                }
            }
        }

        impl<#(#body_types),*> #runtime::http_body::Body for #body<#(#body_types),*>
        where
            #body_bounds
        {
            type Data = #body_data;
            type Error = #error<#(#body_errors),*>;

            fn poll_frame(
                self: ::core::pin::Pin<&mut Self>,
                cx: &mut ::core::task::Context<'_>,
            ) -> ::core::task::Poll<
                ::core::option::Option<
                    ::core::result::Result<
                        #runtime::http_body::Frame<Self::Data>,
                        Self::Error,
                    >
                >
            > {
                match self.project() {
                    #(#poll_arms),*
                }
            }

            fn is_end_stream(&self) -> bool {
                match self {
                    #(#end_stream_arms),*
                }
            }

            fn size_hint(&self) -> #runtime::http_body::SizeHint {
                match self {
                    #(#size_hint_arms),*
                }
            }
        }
    }
}

fn validate_generated_method_names(item: &ItemImpl, has_dynamic: bool, erased_mounts: bool, tower_adapter: bool) -> syn::Result<()> {
    #[cfg(not(feature = "tower"))]
    let _ = tower_adapter;
    let mut generated = if has_dynamic { vec!["router_builder"] } else { vec!["route"] };
    #[cfg(feature = "tower")]
    if tower_adapter && !has_dynamic {
        generated.push("tower_service");
    }
    if !has_dynamic && erased_mounts {
        generated.push("route_with_erased_mounts");
    }
    for generated in generated {
        if item
            .items
            .iter()
            .any(|impl_item| matches!(impl_item, ImplItem::Fn(method) if method.sig.ident == generated))
        {
            return Err(Error::new(
                item.impl_token.span(),
                format!("`#[router]` cannot generate `{generated}` because that method already exists"),
            ));
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "the static entry keeps its generated route, policy, response, and runtime models explicit"
)]
#[expect(
    clippy::too_many_lines,
    reason = "the generated static route keeps dispatch, policy, response, and runtime contracts together"
)]
fn static_route_method(
    handlers: &[Handler],
    dispatches: &[DispatchArm],
    policy: &RoutingPolicy,
    module_name: &Ident,
    route_path: &TokenStream2,
    generated: &GeneratedIdents,
    shared_state: &SharedState,
    response_body: &ResponseBodyModel,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> syn::Result<ImplItem> {
    let GeneratedIdents {
        request,
        state,
        parts,
        body,
        route,
        response,
        failure,
    } = generated;
    let body_type = format_ident!("__RouteramaBody");
    let state_type = &shared_state.ty;
    let contract = route_contract(
        handlers,
        policy,
        &body_type,
        shared_state,
        response_body.heterogeneous_data,
        runtime,
        response_runtime,
    )?;
    let bounds = &contract.bounds;
    let route_generics = route_generics(&body_type, shared_state);
    let scaffold = entry_scaffold(
        policy,
        &quote! { self },
        parts,
        module_name,
        response_body,
        DispatchResponse::Concrete,
        runtime,
        response_runtime,
    );
    let arms = dispatch_arms(
        handlers,
        dispatches,
        policy,
        route_path,
        &quote! { self },
        parts,
        body,
        response,
        failure,
        state,
        &body_type,
        shared_state,
        module_name,
        response_body,
        scaffold.boundary,
        runtime,
        response_runtime,
    );
    let response_type = response_return_type(
        &body_type,
        shared_state,
        response_body.heterogeneous_data,
        runtime,
        response_runtime,
    );
    let routing_error = resolve_failure_response(
        policy,
        &quote! { self },
        &quote! { error },
        module_name,
        response_body,
        runtime,
        response_runtime,
    );
    let parts_binding = &scaffold.parts_binding;
    let routing_failure = scaffold.boundary.exit(&routing_error);
    let dispatch = scaffold.body(
        &quote! {
            let #route = match #route_path::resolver().resolve(
                #parts.method.as_str(),
                #parts.uri.path(),
            ) {
                ::core::result::Result::Ok(route) => route,
                ::core::result::Result::Err(error) => {
                    #routing_failure
                }
            };
            match #route {
                #(#arms),*
            }
        },
        runtime,
    );
    syn::parse2(quote! {
        /// Routes an HTTP request and returns a composed response with an
        /// allocation-free, service-specific opaque body sum.
        #[allow(
            clippy::future_not_send,
            private_bounds,
            reason = "the base HTTP boundary intentionally leaves transport Send bounds to adapters"
        )]
        pub async fn route #route_generics(
            &self,
            #request: #runtime::http::Request<#body_type>,
            #state: &#state_type,
        ) -> #response_type
        where
            #(#bounds),*
        {
            let (#parts_binding, #body) = #request.into_parts();
            #dispatch
        }
    })
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the explicit mount entry keeps its generated route, policy, response, interceptor, and runtime models visible"
)]
fn static_mounted_route_method(
    handlers: &[Handler],
    dispatches: &[DispatchArm],
    policy: &RoutingPolicy,
    module_name: &Ident,
    route_path: &TokenStream2,
    generated: &GeneratedIdents,
    shared_state: &SharedState,
    response_body: &ResponseBodyModel,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> syn::Result<ImplItem> {
    let GeneratedIdents {
        request,
        state,
        parts,
        body,
        route,
        response,
        failure,
    } = generated;
    let mounts = generated_ident("__routerama_mounts", handlers);
    let body_type = format_ident!("__RouteramaBody");
    let state_type = &shared_state.ty;
    let mut contract = route_contract(
        handlers,
        policy,
        &body_type,
        shared_state,
        response_body.heterogeneous_data,
        runtime,
        response_runtime,
    )?;
    let mounts_type = mount_delegate_type();
    let mount = mount_contract(&body_type, shared_state, response_body.heterogeneous_data, runtime);
    contract.bounds.extend(mount.bounds.iter().cloned());
    let bounds = &contract.bounds;
    let route_generics = mounted_route_generics(&body_type, shared_state);
    let mounted_boundary = if response_body.heterogeneous_data {
        DispatchResponse::MountedHeterogeneous
    } else {
        DispatchResponse::Mounted
    };
    let scaffold = entry_scaffold(
        policy,
        &quote! { self },
        parts,
        module_name,
        response_body,
        mounted_boundary,
        runtime,
        response_runtime,
    );
    let arms = dispatch_arms(
        handlers,
        dispatches,
        policy,
        route_path,
        &quote! { self },
        parts,
        body,
        response,
        failure,
        state,
        &body_type,
        shared_state,
        module_name,
        response_body,
        scaffold.boundary,
        runtime,
        response_runtime,
    );
    let response_type = mounted_response_return_type(
        &body_type,
        shared_state,
        response_body.heterogeneous_data,
        runtime,
        response_runtime,
    );
    let routing_error = resolve_failure_response(
        policy,
        &quote! { self },
        &quote! { error },
        module_name,
        response_body,
        runtime,
        response_runtime,
    );
    let parts_binding = &scaffold.parts_binding;
    let mount_body = &mount.body;
    let routing_failure_body = if response_body.heterogeneous_data {
        quote! { #routing_error.map(|body| #response_runtime::DataEitherBody::Left { body }) }
    } else {
        quote! {
            #routing_error.map(
                |body| #response_runtime::EitherBody::<_, #mount_body>::Left { body }
            )
        }
    };
    let routing_failure = scaffold.boundary.exit(&routing_failure_body);
    let mounted_response = if response_body.heterogeneous_data {
        quote! { |body| #response_runtime::DataEitherBody::Right { body } }
    } else {
        quote! { |body| #response_runtime::EitherBody::<_, #mount_body>::Right { body } }
    };
    let mount_router_type = quote! { #mounts_type };
    let dispatch = scaffold.body(
        &quote! {
            let #route = match #route_path::resolver().resolve(
                #parts.method.as_str(),
                #parts.uri.path(),
            ) {
                ::core::result::Result::Ok(route) => route,
                ::core::result::Result::Err(#runtime::ResolveError::NotFound(_)) => {
                    return #mounts
                        .route(#runtime::http::Request::from_parts(#parts, #body), #state)
                        .await
                        .map(#mounted_response);
                }
                ::core::result::Result::Err(error) => {
                    #routing_failure
                }
            };
            match #route {
                #(#arms),*
            }
        },
        runtime,
    );
    syn::parse2(quote! {
        /// Routes generated handlers before delegating a complete miss to the
        /// mounted-service router. Generated response bodies remain concrete.
        #[allow(
            clippy::future_not_send,
            private_bounds,
            reason = "the base HTTP boundary intentionally leaves transport Send bounds to adapters"
        )]
        pub async fn route_with_erased_mounts #route_generics(
            &self,
            #request: #runtime::http::Request<#body_type>,
            #state: &#state_type,
            #mounts: &#mount_router_type,
        ) -> #response_type
        where
            #(#bounds),*
        {
            let (#parts_binding, #body) = #request.into_parts();
            #dispatch
        }
    })
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the generated service-router API keeps its validated symbols and emitted boundary together"
)]
fn dynamic_service_api(
    item: &mut ItemImpl,
    handlers: &[Handler],
    dispatches: &[DispatchArm],
    policy: &RoutingPolicy,
    service_name: &Ident,
    module_name: &Ident,
    route_name: &Ident,
    route_path: &TokenStream2,
    generated: &GeneratedIdents,
    shared_state: &SharedState,
    response_body: &ResponseBodyModel,
    erased_mounts: bool,
    tower_adapter: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> syn::Result<TokenStream2> {
    let service_type = &item.self_ty;
    let resolver_name = format_ident!("{}Resolver", route_name, span = route_name.span());
    let resolver_builder_name = format_ident!("{}Builder", resolver_name, span = resolver_name.span());
    let resolver_path = quote! { #module_name::#resolver_name };
    let resolver_builder_path = quote! { #module_name::#resolver_builder_name };
    let service_router_name = format_ident!("{}Router", service_name, span = service_name.span());
    let service_builder_name = format_ident!("{}RouterBuilder", service_name, span = service_name.span());
    let GeneratedIdents {
        request,
        state,
        parts,
        body,
        route,
        response,
        failure,
    } = generated;
    let service = generated_ident("__routerama_service", handlers);
    let body_type = format_ident!("__RouteramaBody");
    let state_type = &shared_state.ty;
    let contract = route_contract(
        handlers,
        policy,
        &body_type,
        shared_state,
        response_body.heterogeneous_data,
        runtime,
        response_runtime,
    )?;
    let bounds = &contract.bounds;
    let route_generics = route_generics(&body_type, shared_state);
    let scaffold = entry_scaffold(
        policy,
        &quote! { #service },
        parts,
        module_name,
        response_body,
        DispatchResponse::Concrete,
        runtime,
        response_runtime,
    );
    let arms = dispatch_arms(
        handlers,
        dispatches,
        policy,
        route_path,
        &quote! { #service },
        parts,
        body,
        response,
        failure,
        state,
        &body_type,
        shared_state,
        module_name,
        response_body,
        scaffold.boundary,
        runtime,
        response_runtime,
    );
    let response_type = response_return_type(
        &body_type,
        shared_state,
        response_body.heterogeneous_data,
        runtime,
        response_runtime,
    );
    let routing_error = resolve_failure_response(
        policy,
        &quote! { #service },
        &quote! { error },
        module_name,
        response_body,
        runtime,
        response_runtime,
    );
    let mounted_route_method = if erased_mounts {
        let mounts = generated_ident("__routerama_mounts", handlers);
        let mounts_type = mount_delegate_type();
        let mount = mount_contract(&body_type, shared_state, response_body.heterogeneous_data, runtime);
        let mounted_response_type = mounted_response_return_type(
            &body_type,
            shared_state,
            response_body.heterogeneous_data,
            runtime,
            response_runtime,
        );
        let mounted_route_generics = mounted_route_generics(&body_type, shared_state);
        let mut mounted_bounds = contract.bounds.clone();
        mounted_bounds.extend(mount.bounds.iter().cloned());
        let mounted_boundary = if response_body.heterogeneous_data {
            DispatchResponse::MountedHeterogeneous
        } else {
            DispatchResponse::Mounted
        };
        let mounted_scaffold = entry_scaffold(
            policy,
            &quote! { #service },
            parts,
            module_name,
            response_body,
            mounted_boundary,
            runtime,
            response_runtime,
        );
        let mounted_arms = dispatch_arms(
            handlers,
            dispatches,
            policy,
            route_path,
            &quote! { #service },
            parts,
            body,
            response,
            failure,
            state,
            &body_type,
            shared_state,
            module_name,
            response_body,
            mounted_scaffold.boundary,
            runtime,
            response_runtime,
        );
        let parts_binding = &mounted_scaffold.parts_binding;
        let mount_body = &mount.body;
        let routing_failure_body = if response_body.heterogeneous_data {
            quote! { #routing_error.map(|body| #response_runtime::DataEitherBody::Left { body }) }
        } else {
            quote! {
                #routing_error.map(
                    |body| #response_runtime::EitherBody::<_, #mount_body>::Left { body }
                )
            }
        };
        let routing_failure = mounted_scaffold.boundary.exit(&routing_failure_body);
        let mounted_response = if response_body.heterogeneous_data {
            quote! { |body| #response_runtime::DataEitherBody::Right { body } }
        } else {
            quote! { |body| #response_runtime::EitherBody::<_, #mount_body>::Right { body } }
        };
        let mount_router_type = quote! { #mounts_type };
        let mounted_dispatch = mounted_scaffold.body(
            &quote! {
                let #route = match self.__resolver.resolve(
                    #parts.method.as_str(),
                    #parts.uri.path(),
                ) {
                    ::core::result::Result::Ok(route) => route,
                    ::core::result::Result::Err(#runtime::ResolveError::NotFound(_)) => {
                        return #mounts
                            .route(#runtime::http::Request::from_parts(#parts, #body), #state)
                            .await
                            .map(#mounted_response);
                    }
                    ::core::result::Result::Err(error) => {
                        #routing_failure
                    }
                };
                match #route {
                    #(#mounted_arms),*
                }
            },
            runtime,
        );
        quote! {
                    /// Routes generated handlers before delegating a complete
                    /// miss to mounts. Generated response bodies remain
                    /// concrete.
                    #[allow(
                        clippy::future_not_send,
                        private_bounds,
                        reason = "the base HTTP boundary intentionally leaves transport Send bounds to adapters"
                    )]
                    pub async fn route_with_erased_mounts #mounted_route_generics(
                        &self,
                        #service: &#service_type,
                        #request: #runtime::http::Request<#body_type>,
                        #state: &#state_type,
                        #mounts: &#mount_router_type,
                    ) -> #mounted_response_type
                    where
                        #(#mounted_bounds),*
                    {
                        let (#parts_binding, #body) = #request.into_parts();
                        #mounted_dispatch
                    }
        }
    } else {
        TokenStream2::new()
    };

    let add_methods = handlers
        .iter()
        .filter(|handler| handler.kind == HandlerKind::Dynamic)
        .map(|handler| {
            let variant_name = handler.variant.to_string();
            let add_name = format_ident!("add_{}", resolver::to_snake_case(&variant_name), span = handler.method.span());
            let handler_name = handler.method.to_string();
            let doc = format!(
                "Registers a method and path template for the dynamic `{handler_name}` handler.\n\n\
                 Call this method more than once to register aliases. Template and capture \
                 validation errors are accumulated and returned by [`build`](Self::build)."
            );
            quote! {
                #[doc = #doc]
                #[must_use]
                pub fn #add_name(
                    mut self,
                    method: impl ::core::convert::AsRef<str>,
                    path: impl ::core::convert::AsRef<str>,
                ) -> Self {
                    self.__builder = self.__builder.#add_name(method, path);
                    self
                }
            }
        });

    item.items.push(syn::parse2(quote! {
        /// Creates a builder for the service's static and dynamic routes.
        #[must_use]
        pub fn router_builder() -> #service_builder_name {
            #service_builder_name {
                __builder: #route_path::builder(),
            }
        }
    })?);

    let parts_binding = &scaffold.parts_binding;
    let routing_failure = scaffold.boundary.exit(&routing_error);
    let dispatch = scaffold.body(
        &quote! {
            let #route = match self.__resolver.resolve(
                #parts.method.as_str(),
                #parts.uri.path(),
            ) {
                ::core::result::Result::Ok(route) => route,
                ::core::result::Result::Err(error) => {
                    #routing_failure
                }
            };
            match #route {
                #(#arms),*
            }
        },
        runtime,
    );
    #[cfg(feature = "tower")]
    let (tower_service_method, tower_route_impl) = if tower_adapter {
        (
            dynamic_tower_service_method(
                handlers,
                policy,
                service_type,
                shared_state,
                response_body.heterogeneous_data,
                runtime,
                response_runtime,
            ),
            dynamic_tower_route_impl(
                handlers,
                policy,
                &service_router_name,
                service_type,
                shared_state,
                response_body.heterogeneous_data,
                runtime,
                response_runtime,
            )?,
        )
    } else {
        (TokenStream2::new(), TokenStream2::new())
    };
    #[cfg(not(feature = "tower"))]
    let (tower_service_method, tower_route_impl) = {
        let _ = tower_adapter;
        (TokenStream2::new(), TokenStream2::new())
    };

    Ok(quote! {
        #[doc = "A configured router for the service."]
        #[derive(Debug)]
        pub struct #service_router_name {
            __resolver: #resolver_path,
        }

        #[doc = "Builds a configured router for the service."]
        #[derive(Debug)]
        pub struct #service_builder_name {
            __builder: #resolver_builder_path,
        }

        #[automatically_derived]
        impl #service_builder_name {
            #(#add_methods)*

            /// Validates dynamic registrations and builds the service router.
            ///
            /// # Errors
            ///
            /// Returns a Routerama configuration error containing every missing
            /// or invalid dynamic route registration.
            pub fn build(self) -> ::core::result::Result<#service_router_name, #runtime::ConfigurationError> {
                ::core::result::Result::Ok(#service_router_name {
                    __resolver: self.__builder.build()?,
                })
            }
        }

        #[automatically_derived]
        #[allow(
            private_interfaces,
            reason = "the service type may intentionally be private to its module"
        )]
        impl #service_router_name {
            #tower_service_method
            #mounted_route_method

            /// Routes an HTTP request through the configured service and
            /// returns its allocation-free, service-specific opaque body sum.
            #[allow(
                clippy::future_not_send,
                private_bounds,
                reason = "the base HTTP boundary intentionally leaves transport Send bounds to adapters"
            )]
            pub async fn route #route_generics(
                &self,
                #service: &#service_type,
                #request: #runtime::http::Request<#body_type>,
                #state: &#state_type,
            ) -> #response_type
            where
                #(#bounds),*
            {
                let (#parts_binding, #body) = #request.into_parts();
                #dispatch
            }
        }

        #tower_route_impl
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "dispatch generation keeps each request owner, ordered predicate, extraction, and handler boundary explicit"
)]
fn dispatch_arms(
    handlers: &[Handler],
    dispatches: &[DispatchArm],
    policy: &RoutingPolicy,
    route_path: &TokenStream2,
    target: &TokenStream2,
    parts: &Ident,
    body: &Ident,
    response: &Ident,
    failure: &Ident,
    state: &Ident,
    body_type: &Ident,
    shared_state: &SharedState,
    module_name: &Ident,
    response_body: &ResponseBodyModel,
    boundary: DispatchBoundary,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> Vec<TokenStream2> {
    dispatches
        .iter()
        .map(|dispatch| {
            let variant = &dispatch.variant;
            let pattern = if dispatch.captures.is_empty() {
                quote! { #route_path::#variant }
            } else {
                let fields = dispatch.captures.iter().map(|(name, _)| name);
                quote! { #route_path::#variant { #(#fields),* } }
            };
            let body = match &dispatch.kind {
                DispatchKind::Direct(index) => emit_direct_dispatch(
                    &handlers[*index],
                    policy,
                    target,
                    parts,
                    body,
                    response,
                    state,
                    body_type,
                    shared_state,
                    module_name,
                    response_body,
                    boundary,
                    runtime,
                    response_runtime,
                ),
                DispatchKind::Overlap(candidates) => emit_overlap_dispatch(
                    candidates,
                    handlers,
                    policy,
                    target,
                    parts,
                    body,
                    response,
                    failure,
                    state,
                    body_type,
                    shared_state,
                    module_name,
                    response_body,
                    boundary,
                    runtime,
                    response_runtime,
                ),
            };
            quote! {
                #pattern => {
                    #body
                }
            }
        })
        .collect()
}

#[expect(
    clippy::too_many_arguments,
    reason = "generated direct dispatch names each owned request component and concrete response boundary"
)]
fn emit_direct_dispatch(
    handler: &Handler,
    policy: &RoutingPolicy,
    target: &TokenStream2,
    parts: &Ident,
    body: &Ident,
    response: &Ident,
    state: &Ident,
    body_type: &Ident,
    shared_state: &SharedState,
    module_name: &Ident,
    response_body: &ResponseBodyModel,
    boundary: DispatchBoundary,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> TokenStream2 {
    let host_check = handler.predicates.host.as_ref().map(|expected| {
        let rejection = wrap_dispatch_response(
            predicate_failure_response(
                policy,
                target,
                &quote! { #runtime::RouteFailure::HostMismatch { path: #parts.uri.path() } },
                module_name,
                response_body,
                runtime,
                response_runtime,
            ),
            boundary,
            response_runtime,
        );
        let rejection = boundary.exit(&rejection);
        quote! {
            if !#runtime::host_matches(&#parts, #expected) {
                #rejection
            }
        }
    });
    let consumes_check = handler.predicates.consumes.as_ref().map(|expected| {
        let expected_media_type = media_type_expression(&expected.value(), expected.span(), runtime);
        let rejection = wrap_dispatch_response(
            predicate_failure_response(
                policy,
                target,
                &quote! { #runtime::RouteFailure::UnsupportedMediaType { path: #parts.uri.path() } },
                module_name,
                response_body,
                runtime,
                response_runtime,
            ),
            boundary,
            response_runtime,
        );
        let rejection = boundary.exit(&rejection);
        quote! {
            if !#runtime::content_type_matches_parsed(&#parts.headers, #expected_media_type) {
                #rejection
            }
        }
    });
    let produces_check = handler.predicates.produces.as_ref().map(|expected| {
        let expected_media_type = media_type_expression(&expected.value(), expected.span(), runtime);
        let rejection = wrap_dispatch_response(
            predicate_failure_response(
                policy,
                target,
                &quote! { #runtime::RouteFailure::NotAcceptable { path: #parts.uri.path() } },
                module_name,
                response_body,
                runtime,
                response_runtime,
            ),
            boundary,
            response_runtime,
        );
        let rejection = boundary.exit(&rejection);
        quote! {
            if !#runtime::accepts_parsed(&#parts.headers, #expected_media_type) {
                #rejection
            }
        }
    });
    let selected = emit_selected_handler(
        handler,
        policy,
        target,
        parts,
        body,
        response,
        state,
        body_type,
        shared_state,
        module_name,
        response_body,
        boundary,
        runtime,
        response_runtime,
    );
    quote! {
        #host_check
        #consumes_check
        #produces_check
        #selected
    }
}

#[derive(Clone, Copy)]
enum PredicateKind {
    Host,
    Consumes,
    Produces,
}

struct OverlapPredicatePlan {
    hosts: Vec<String>,
    consumes: Vec<String>,
    produces: Vec<String>,
}

impl OverlapPredicatePlan {
    fn new(candidates: &[usize], handlers: &[Handler]) -> Self {
        Self {
            hosts: overlap_predicate_values(candidates, handlers, PredicateKind::Host),
            consumes: overlap_predicate_values(candidates, handlers, PredicateKind::Consumes),
            produces: overlap_predicate_values(candidates, handlers, PredicateKind::Produces),
        }
    }

    fn is_empty(&self) -> bool {
        self.hosts.is_empty() && self.consumes.is_empty() && self.produces.is_empty()
    }

    fn index(&self, kind: PredicateKind, value: &LitStr) -> usize {
        let values = match kind {
            PredicateKind::Host => &self.hosts,
            PredicateKind::Consumes => &self.consumes,
            PredicateKind::Produces => &self.produces,
        };
        let normalized = value.value().to_ascii_lowercase();
        values
            .iter()
            .position(|candidate| candidate == &normalized)
            .expect("the overlap plan contains every validated candidate predicate")
    }

    fn setup(&self, state: &Ident, runtime: &TokenStream2) -> TokenStream2 {
        if self.is_empty() {
            return TokenStream2::new();
        }

        let host_count = self.hosts.len();
        let consumes_count = self.consumes.len();
        let produces_count = self.produces.len();
        let hosts = self.hosts.iter().map(|value| LitStr::new(value, Span::call_site()));
        let consumes = self
            .consumes
            .iter()
            .map(|value| media_type_expression(value, Span::call_site(), runtime));
        let produces = self
            .produces
            .iter()
            .map(|value| media_type_expression(value, Span::call_site(), runtime));
        let produces_top_level = self
            .produces
            .first()
            .and_then(|first| first.split_once('/').map(|(top_level, _)| top_level))
            .filter(|top_level| {
                self.produces
                    .iter()
                    .all(|value| value.split_once('/').is_some_and(|(candidate, _)| candidate == *top_level))
            })
            .map_or_else(
                || quote! { ::core::option::Option::None },
                |top_level| {
                    let top_level = LitByteStr::new(top_level.as_bytes(), Span::call_site());
                    quote! { ::core::option::Option::Some(#top_level) }
                },
            );

        quote! {
            const __ROUTERAMA_OVERLAP_HOSTS: [&str; #host_count] = [#(#hosts),*];
            const __ROUTERAMA_OVERLAP_CONSUMES: [#runtime::MediaType<'static>; #consumes_count] = [#(#consumes),*];
            const __ROUTERAMA_OVERLAP_PRODUCES: [#runtime::MediaType<'static>; #produces_count] = [#(#produces),*];
            let mut #state = #runtime::OverlapPredicateState::new(
                &__ROUTERAMA_OVERLAP_HOSTS,
                &__ROUTERAMA_OVERLAP_CONSUMES,
                &__ROUTERAMA_OVERLAP_PRODUCES,
                #produces_top_level,
            );
        }
    }
}

fn overlap_predicate_values(candidates: &[usize], handlers: &[Handler], kind: PredicateKind) -> Vec<String> {
    let mut values = Vec::new();
    for index in candidates {
        let predicates = &handlers[*index].predicates;
        let value = match kind {
            PredicateKind::Host => predicates.host.as_ref(),
            PredicateKind::Consumes => predicates.consumes.as_ref(),
            PredicateKind::Produces => predicates.produces.as_ref(),
        };
        let Some(value) = value else {
            continue;
        };
        let normalized = value.value().to_ascii_lowercase();
        if !values.contains(&normalized) {
            values.push(normalized);
        }
    }

    // Keep the highest-priority value first for the common winner-first path;
    // sorting the tail lets the runtime binary-search every other value.
    if values.len() > 1 {
        match kind {
            // A host is compared as one string, so its natural order matches.
            PredicateKind::Host => values[1..].sort_unstable(),
            // The runtime searches media types by `(top_level, subtype)`, an
            // order the joined string diverges from whenever a top-level type
            // contains a tchar below `/`, such as `.`, `-`, `+`, or `*`.
            PredicateKind::Consumes | PredicateKind::Produces => {
                values[1..].sort_unstable_by(|left, right| media_type_order(left).cmp(&media_type_order(right)));
            }
        }
    }
    values
}

/// Splits a media type into the `(top_level, subtype)` key that the runtime
/// helper `find_media_type` binary-searches on.
fn media_type_order(value: &str) -> (&str, &str) {
    value
        .split_once('/')
        .expect("route parsing validated every consumes and produces media type")
}

fn media_type_expression(value: &str, span: Span, runtime: &TokenStream2) -> TokenStream2 {
    let (top_level, subtype) = media_type_order(value);
    let top_level = LitByteStr::new(top_level.as_bytes(), span);
    let subtype = LitByteStr::new(subtype.as_bytes(), span);
    quote! { #runtime::MediaType::new(#top_level, #subtype) }
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "overlap dispatch keeps the priority decision separate from all extraction owners"
)]
fn emit_overlap_dispatch(
    candidates: &[usize],
    handlers: &[Handler],
    policy: &RoutingPolicy,
    target: &TokenStream2,
    parts: &Ident,
    body: &Ident,
    response: &Ident,
    failure: &Ident,
    state: &Ident,
    body_type: &Ident,
    shared_state: &SharedState,
    module_name: &Ident,
    response_body: &ResponseBodyModel,
    boundary: DispatchBoundary,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> TokenStream2 {
    let has_default_candidate = candidates.last().is_some_and(|index| handlers[*index].predicates.is_empty());
    let predicate_plan = OverlapPredicatePlan::new(candidates, handlers);
    let predicate_state = format_ident!("__routerama_overlap_predicates");
    let predicate_setup = predicate_plan.setup(&predicate_state, runtime);
    let candidate_checks = candidates.iter().map(|index| {
        let handler = &handlers[*index];
        let selected = emit_selected_handler(
            handler,
            policy,
            target,
            parts,
            body,
            response,
            state,
            body_type,
            shared_state,
            module_name,
            response_body,
            boundary,
            runtime,
            response_runtime,
        );
        let mut check = boundary.exit(&quote! { { #selected } });
        if let Some(expected) = &handler.predicates.produces {
            let expected_index = predicate_plan.index(PredicateKind::Produces, expected);
            check = if has_default_candidate {
                quote! {
                    if #predicate_state.produces(&#parts.headers, #expected_index) {
                        #check
                    }
                }
            } else {
                quote! {
                    if #predicate_state.produces(&#parts.headers, #expected_index) {
                        #check
                    } else if #failure < 3 {
                        #failure = 3;
                    }
                }
            };
        }
        if let Some(expected) = &handler.predicates.consumes {
            let expected_index = predicate_plan.index(PredicateKind::Consumes, expected);
            check = if has_default_candidate {
                quote! {
                    if #predicate_state.consumes(&#parts.headers, #expected_index) {
                        #check
                    }
                }
            } else {
                quote! {
                    if #predicate_state.consumes(&#parts.headers, #expected_index) {
                        #check
                    } else if #failure < 2 {
                        #failure = 2;
                    }
                }
            };
        }
        if let Some(expected) = &handler.predicates.host {
            let expected_index = predicate_plan.index(PredicateKind::Host, expected);
            check = if has_default_candidate {
                quote! {
                    if #predicate_state.host(&#parts, #expected_index) {
                        #check
                    }
                }
            } else {
                quote! {
                    if #predicate_state.host(&#parts, #expected_index) {
                        #check
                    } else if #failure < 1 {
                        #failure = 1;
                    }
                }
            };
        }
        check
    });
    let (failure_declaration, rejected) = if has_default_candidate {
        (TokenStream2::new(), TokenStream2::new())
    } else {
        let failure_declaration = quote! { let mut #failure = 0_u8; };
        let typed_failure = quote! {
            match #failure {
                3 => #runtime::RouteFailure::NotAcceptable { path: #parts.uri.path() },
                2 => #runtime::RouteFailure::UnsupportedMediaType { path: #parts.uri.path() },
                _ => #runtime::RouteFailure::HostMismatch { path: #parts.uri.path() },
            }
        };
        let rejected = wrap_dispatch_response(
            predicate_failure_response(
                policy,
                target,
                &typed_failure,
                module_name,
                response_body,
                runtime,
                response_runtime,
            ),
            boundary,
            response_runtime,
        );
        (failure_declaration, rejected)
    };
    quote! {
        #failure_declaration
        #predicate_setup
        #(#candidate_checks)*
        #rejected
    }
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "selected-handler lowering explicitly sequences before, transform, parts, body, catcher, after, and response ownership"
)]
fn emit_selected_handler(
    handler: &Handler,
    policy: &RoutingPolicy,
    target: &TokenStream2,
    parts: &Ident,
    body: &Ident,
    response: &Ident,
    state: &Ident,
    body_type: &Ident,
    shared_state: &SharedState,
    module_name: &Ident,
    response_body: &ResponseBodyModel,
    boundary: DispatchBoundary,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> TokenStream2 {
    let state_type = &shared_state.ty;
    let response_body_name = &response_body.body;
    let per_handler_befores: Vec<TokenStream2> = policy
        .handler_befores(&handler.method)
        .map(|before| {
            emit_before_call(
                before,
                target,
                parts,
                module_name,
                response_body,
                boundary,
                runtime,
                response_runtime,
            )
        })
        .collect();
    let parts_extraction = handler.arguments.iter().filter_map(|argument| {
        let Argument::Parts(name, ty) = argument else {
            return None;
        };
        let rejection_response = if let Some(catcher) = catcher_for(policy, ExtractionKind::Parts, ty) {
            let method = &catcher.method;
            let variant = response_body.variant(&catcher_response_key(catcher));
            quote! {
                #response_runtime::IntoResponse::into_response(
                    #target.#method(rejection).await
                ).map(|body| #module_name::#response_body_name::#variant { body })
            }
        } else {
            let variant = response_body.variant(&parts_rejection_key(ty));
            // Generic state leaves this rejection behind a higher-ranked
            // projection. Erase only its response body so private rejection
            // types stay out of the route signature.
            let rejection_body = if shared_state.generic.is_some() {
                quote! { #response_runtime::SendBoxBody::new(body) }
            } else {
                quote! { body }
            };
            quote! {
                #response_runtime::IntoResponse::into_response(rejection).map(
                    |body| #module_name::#response_body_name::#variant { body: #rejection_body }
                )
            }
        };
        let rejection = boundary.exit(&wrap_dispatch_response(rejection_response, boundary, response_runtime));
        Some(quote! {
            let #name: #ty = match <#ty as #runtime::FromRequestParts<'_, #state_type>>::from_request_parts(
                &#parts,
                #state,
            ) {
                ::core::result::Result::Ok(value) => value,
                ::core::result::Result::Err(rejection) => {
                    #rejection
                }
            };
        })
    });
    let transform = policy.transform_for(&handler.method);
    let (transform_prelude, body_source, body_source_type): (TokenStream2, TokenStream2, TokenStream2) = match transform {
        None => (TokenStream2::new(), quote! { #body }, quote! { #body_type }),
        Some(transform) => {
            let method = &transform.method;
            let short_variant = response_body.variant(&interceptor_response_key(method));
            let short_circuit = boundary.exit(&wrap_dispatch_response(
                quote! {
                    #response_runtime::IntoResponse::into_response(__routerama_short).map(
                        |body| #module_name::#response_body_name::#short_variant { body }
                    )
                },
                boundary,
                response_runtime,
            ));
            // Bounded buffering collects the transport body first; a streaming
            // transform moves it directly into the interceptor's generic body
            // parameter, so the framework neither buffers nor allocates.
            let (owner, transform_input) = match &transform.mode {
                TransformMode::Buffered { limit } => {
                    let buffer_variant = response_body.variant(&transform_buffer_key(method));
                    let buffer_rejection = boundary.exit(&wrap_dispatch_response(
                        quote! {
                            #response_runtime::IntoResponse::into_response(rejection).map(
                                |body| #module_name::#response_body_name::#buffer_variant { body }
                            )
                        },
                        boundary,
                        response_runtime,
                    ));
                    (
                        quote! {
                            let __routerama_buffered = match #runtime::buffer_request_body::<#body_type, { #limit }>(#body).await {
                                ::core::result::Result::Ok(bytes) => bytes,
                                ::core::result::Result::Err(rejection) => {
                                    #buffer_rejection
                                }
                            };
                        },
                        quote! { __routerama_buffered },
                    )
                }
                TransformMode::Streaming => (TokenStream2::new(), quote! { #body }),
            };
            if let Some(replacement_ty) = &transform.replacement_body {
                let prelude = quote! {
                    #owner
                    let __routerama_transformed_body: #replacement_ty = match #target.#method(
                        &#parts,
                        #transform_input,
                    ).await {
                        #runtime::BodyTransform::Replace(__routerama_replacement) => __routerama_replacement,
                        #runtime::BodyTransform::Respond(__routerama_short) => {
                            #short_circuit
                        }
                    };
                };
                (prelude, quote! { __routerama_transformed_body }, quote! { #replacement_ty })
            } else {
                let prelude = quote! {
                    #owner
                    match #target.#method(&#parts, #transform_input).await {
                        #runtime::BodyConsumed::Consumed => {}
                        #runtime::BodyConsumed::Respond(__routerama_short) => {
                            #short_circuit
                        }
                    }
                };
                (prelude, quote! { #body }, quote! { #body_type })
            }
        }
    };
    let body_extraction = handler.arguments.iter().find_map(|argument| match argument {
        Argument::Body(name, ty) => {
            let rejection_response = if let Some(catcher) = catcher_for(policy, ExtractionKind::Body, ty) {
                let method = &catcher.method;
                let variant = response_body.variant(&catcher_response_key(catcher));
                quote! {
                    #response_runtime::IntoResponse::into_response(
                        #target.#method(rejection).await
                    ).map(|body| #module_name::#response_body_name::#variant { body })
                }
            } else {
                let variant = response_body.variant(&body_rejection_key(ty, &body_source_type));
                quote! {
                    #response_runtime::IntoResponse::into_response(rejection).map(
                        |body| #module_name::#response_body_name::#variant { body }
                    )
                }
            };
            let rejection = boundary.exit(&wrap_dispatch_response(rejection_response, boundary, response_runtime));
            Some(quote! {
                let #name: #ty = match <#ty as #runtime::FromRequestBody<#state_type, #body_source_type>>::from_request_body(
                    &#parts,
                    #body_source,
                    #state,
                ).await {
                    ::core::result::Result::Ok(value) => value,
                    ::core::result::Result::Err(rejection) => {
                        #rejection
                    }
                };
            })
        }
        Argument::Capture(_) | Argument::Parts(_, _) => None,
    });
    let arguments = handler.arguments.iter().map(|argument| match argument {
        Argument::Capture(name) | Argument::Parts(name, _) | Argument::Body(name, _) => quote! { #name },
    });
    let method = &handler.method;
    let handler_variant = response_body.variant(&handler_response_key(&handler.response_type));
    let handler_call = quote! { #target.#method(#(#arguments),*).await };
    let static_header_calls = handler.static_headers.iter().map(|header| {
        let name = &header.name;
        let value = &header.value;
        match header.operation {
            StaticHeaderOperation::Insert => quote! {
                #response.headers_mut().insert(
                    const { #runtime::http::header::HeaderName::from_static(#name) },
                    const { #runtime::http::header::HeaderValue::from_static(#value) },
                );
            },
            StaticHeaderOperation::Append => quote! {
                #response.headers_mut().append(
                    const { #runtime::http::header::HeaderName::from_static(#name) },
                    const { #runtime::http::header::HeaderValue::from_static(#value) },
                );
            },
        }
    });
    let produced_header = handler.predicates.produces.as_ref().map(|produced| {
        quote! {
            #response.headers_mut().insert(
                #runtime::http::header::CONTENT_TYPE,
                const { #runtime::http::header::HeaderValue::from_static(#produced) },
            );
        }
    });
    let handler_mapped = if handler.static_headers.is_empty() && produced_header.is_none() {
        quote! {
            #response_runtime::IntoResponse::into_response(#handler_call)
                .map(|body| #module_name::#response_body_name::#handler_variant { body })
        }
    } else {
        quote! {
            {
                let mut #response = #response_runtime::IntoResponse::into_response(#handler_call);
                #(#static_header_calls)*
                #produced_header
                #response.map(|body| #module_name::#response_body_name::#handler_variant { body })
            }
        }
    };
    // Only per-handler response interceptors run here; a generated-wide
    // `#[after]` observes this response, and every other generated response,
    // once at the entry.
    let after_calls: Vec<TokenStream2> = policy
        .handler_afters(&handler.method)
        .map(|after| {
            let after_method = &after.method;
            quote! {
                #target.#after_method(
                    &mut #runtime::AfterContext::new(&#parts, &mut __routerama_response_parts)
                ).await;
            }
        })
        .collect();
    let handler_and_after = if after_calls.is_empty() {
        handler_mapped
    } else {
        quote! {
            {
                let __routerama_after_response = #handler_mapped;
                let (mut __routerama_response_parts, __routerama_after_body) = __routerama_after_response.into_parts();
                #(#after_calls)*
                #runtime::http::Response::from_parts(__routerama_response_parts, __routerama_after_body)
            }
        }
    };
    let handler_response = wrap_dispatch_response(handler_and_after, boundary, response_runtime);
    quote! {
        #(#per_handler_befores)*
        #transform_prelude
        #(#parts_extraction)*
        #body_extraction
        #handler_response
    }
}

fn wrap_dispatch_response(response: TokenStream2, boundary: DispatchBoundary, response_runtime: &TokenStream2) -> TokenStream2 {
    match boundary.response {
        DispatchResponse::Concrete => response,
        DispatchResponse::Mounted => quote! {
            ({ #response }).map(
                |body| #response_runtime::EitherBody::Left { body }
            )
        },
        DispatchResponse::MountedHeterogeneous => quote! {
            ({ #response }).map(
                |body| #response_runtime::DataEitherBody::Left { body }
            )
        },
    }
}

/// Emits one direct `#[before]` interceptor call that either proceeds or
/// short-circuits with a mapped response.
///
/// A router-wide interceptor runs at an entry, before routing, and receives the
/// whole mutable request head. A per-handler interceptor runs inside the
/// selected dispatch arm and receives a split head: the method, URI, and
/// version are borrowed immutably — which is what lets the selected route keep
/// its zero-copy captures alive — while the headers and extensions stay
/// mutable.
#[expect(
    clippy::too_many_arguments,
    reason = "an interceptor call names its target, request head, and concrete short-circuit boundary"
)]
fn emit_before_call(
    before: &BeforeInterceptor,
    target: &TokenStream2,
    parts: &Ident,
    module_name: &Ident,
    response_body: &ResponseBodyModel,
    boundary: DispatchBoundary,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> TokenStream2 {
    let method = &before.method;
    let response_body_name = &response_body.body;
    let variant = response_body.variant(&interceptor_response_key(method));
    let short_circuit = boundary.exit(&wrap_dispatch_response(
        quote! {
            #response_runtime::IntoResponse::into_response(__routerama_short).map(
                |body| #module_name::#response_body_name::#variant { body }
            )
        },
        boundary,
        response_runtime,
    ));
    let outcome = if before.handlers.is_some() {
        quote! {
            {
                let mut __routerama_selected = #runtime::SelectedContext::new(
                    &#parts.method,
                    &#parts.uri,
                    #parts.version,
                    &mut #parts.headers,
                    &mut #parts.extensions,
                );
                #target.#method(&mut __routerama_selected).await
            }
        }
    } else {
        quote! { #target.#method(&mut #runtime::BeforeContext::new(&mut #parts)).await }
    };
    quote! {
        match #outcome {
            #runtime::Before::Next => {}
            #runtime::Before::Respond(__routerama_short) => {
                #short_circuit
            }
        }
    }
}

/// The control flow one generated entry wraps around route resolution and
/// dispatch: the request-head binding, the router-wide `#[before]` preamble,
/// the exit form every stage uses, and the generated-wide `#[after]` epilogue.
struct EntryScaffold {
    parts_binding: TokenStream2,
    boundary: DispatchBoundary,
    preamble: TokenStream2,
    /// The generated-wide `#[after]` calls, when any exist.
    epilogue: Option<TokenStream2>,
}

impl EntryScaffold {
    /// Wraps one entry's resolution-and-dispatch core in that control flow.
    ///
    /// A router-wide `#[after]` adds a common response-head epilogue while
    /// preserving the original body.
    fn body(&self, core: &TokenStream2, runtime: &TokenStream2) -> TokenStream2 {
        let preamble = &self.preamble;
        let Some(afters) = &self.epilogue else {
            return quote! {
                #preamble
                #core
            };
        };
        let label = dispatch_label();
        quote! {
            let __routerama_dispatched = #label: {
                #preamble
                #core
            };
            let (mut __routerama_response_parts, __routerama_response_body) = __routerama_dispatched.into_parts();
            #afters
            #runtime::http::Response::from_parts(__routerama_response_parts, __routerama_response_body)
        }
    }
}

/// Builds the control flow of one generated entry.
///
/// Request mutability and a common response epilogue are emitted only when
/// required by configured interceptors.
#[expect(
    clippy::too_many_arguments,
    reason = "an entry scaffold names its request head, target, and concrete short-circuit boundary"
)]
fn entry_scaffold(
    policy: &RoutingPolicy,
    target: &TokenStream2,
    parts: &Ident,
    module_name: &Ident,
    response_body: &ResponseBodyModel,
    response: DispatchResponse,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> EntryScaffold {
    let parts_binding = if policy.mutates_request() {
        quote! { mut #parts }
    } else {
        quote! { #parts }
    };
    let after_calls: Vec<TokenStream2> = policy
        .generated_wide_afters()
        .map(|after| {
            let after_method = &after.method;
            quote! {
                #target.#after_method(
                    &mut #runtime::AfterContext::new(&#parts, &mut __routerama_response_parts)
                ).await;
            }
        })
        .collect();
    let boundary = DispatchBoundary {
        response,
        exit: if after_calls.is_empty() {
            DispatchExit::Return
        } else {
            DispatchExit::Break
        },
    };
    let befores: Vec<TokenStream2> = policy
        .router_wide_befores()
        .map(|before| {
            emit_before_call(
                before,
                target,
                parts,
                module_name,
                response_body,
                boundary,
                runtime,
                response_runtime,
            )
        })
        .collect();
    EntryScaffold {
        parts_binding,
        boundary,
        preamble: quote! { #(#befores)* },
        epilogue: (!after_calls.is_empty()).then(|| quote! { #(#after_calls)* }),
    }
}

fn resolve_failure_response(
    policy: &RoutingPolicy,
    target: &TokenStream2,
    error: &TokenStream2,
    module_name: &Ident,
    response_body: &ResponseBodyModel,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> TokenStream2 {
    let response_body_name = &response_body.body;
    if let Some(fallback) = &policy.fallback {
        let method = &fallback.method;
        let variant = response_body.variant(FALLBACK_RESPONSE_KEY);
        quote! {
            #response_runtime::IntoResponse::into_response(
                #target.#method(#runtime::route_failure(#error)).await
            ).map(|body| #module_name::#response_body_name::#variant { body })
        }
    } else {
        let variant = response_body.variant(ROUTING_RESPONSE_KEY);
        quote! {
            #runtime::resolve_error_response(#error).map(
                |body| #module_name::#response_body_name::#variant { body }
            )
        }
    }
}

fn predicate_failure_response(
    policy: &RoutingPolicy,
    target: &TokenStream2,
    failure: &TokenStream2,
    module_name: &Ident,
    response_body: &ResponseBodyModel,
    _runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> TokenStream2 {
    let response_body_name = &response_body.body;
    if let Some(fallback) = &policy.fallback {
        let method = &fallback.method;
        let variant = response_body.variant(FALLBACK_RESPONSE_KEY);
        quote! {
            #response_runtime::IntoResponse::into_response(
                #target.#method(#failure).await
            ).map(|body| #module_name::#response_body_name::#variant { body })
        }
    } else {
        let variant = response_body.variant(PREDICATE_RESPONSE_KEY);
        quote! {
            #response_runtime::IntoResponse::into_response(#failure).map(
                |body| #module_name::#response_body_name::#variant { body }
            )
        }
    }
}

fn validate_impl(item: &ItemImpl) -> syn::Result<()> {
    if item.trait_.is_some() {
        return Err(Error::new(item.impl_token.span(), "`#[router]` requires an inherent impl"));
    }
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(Error::new(
            item.generics.span(),
            "`#[router]` does not yet support generic impl blocks",
        ));
    }
    if item.unsafety.is_some() {
        return Err(Error::new(
            item.impl_token.span(),
            "`#[router]` does not support unsafe impl blocks",
        ));
    }
    if let Some(attribute) = item
        .attrs
        .iter()
        .find(|attribute| attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr"))
    {
        return Err(Error::new(
            attribute.span(),
            "`#[router]` does not support conditional compilation on the impl block",
        ));
    }
    Ok(())
}

fn service_name(self_ty: &Type) -> syn::Result<Ident> {
    let Type::Path(path) = self_ty else {
        return Err(Error::new(self_ty.span(), "`#[router]` requires a named service type"));
    };
    if path.qself.is_some()
        || path
            .path
            .segments
            .last()
            .is_some_and(|segment| !matches!(segment.arguments, PathArguments::None))
    {
        return Err(Error::new(self_ty.span(), "`#[router]` does not yet support generic service types"));
    }
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.clone())
        .ok_or_else(|| Error::new(self_ty.span(), "`#[router]` requires a named service type"))
}

struct CatchAttr {
    rejection_type: Type,
    extractor_type: Option<Type>,
}

impl syn::parse::Parse for CatchAttr {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let rejection_type = input.parse()?;
        let mut extractor_type = None;
        if !input.is_empty() {
            let _comma: Token![,] = input.parse()?;
            if input.is_empty() {
                return Err(input.error("expected `from = ExtractorType` after the comma"));
            }
            let key: Ident = input.parse()?;
            if key != "from" {
                return Err(Error::new(key.span(), "expected `from = ExtractorType`"));
            }
            let _equals: Token![=] = input.parse()?;
            extractor_type = Some(input.parse()?);
            let _trailing_comma = input.parse::<Option<Token![,]>>()?;
        }
        if !input.is_empty() {
            return Err(input.error("unexpected catcher argument"));
        }
        Ok(Self {
            rejection_type,
            extractor_type,
        })
    }
}

fn parse_policy(item: &ItemImpl) -> syn::Result<RoutingPolicy> {
    let mut policy = RoutingPolicy::default();
    for impl_item in &item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        if parse_interceptor(&mut policy, method)? {
            continue;
        }
        let fallback_attrs: Vec<_> = method
            .attrs
            .iter()
            .filter(|attribute| attribute.path().is_ident("fallback"))
            .collect();
        let catch_attrs: Vec<_> = method.attrs.iter().filter(|attribute| attribute.path().is_ident("catch")).collect();
        if fallback_attrs.is_empty() && catch_attrs.is_empty() {
            continue;
        }
        if fallback_attrs.len() > 1 || catch_attrs.len() > 1 {
            return Err(Error::new(
                method.sig.ident.span(),
                "a routing policy method may have only one `#[fallback]` or `#[catch(...)]` annotation",
            ));
        }
        if !fallback_attrs.is_empty() && !catch_attrs.is_empty() {
            return Err(Error::new(
                method.sig.ident.span(),
                "a routing policy method cannot be both a fallback and an extractor catcher",
            ));
        }

        let (argument_type, response_type) = validate_policy_signature(method)?;
        if let Some(attribute) = fallback_attrs.first() {
            if !matches!(attribute.meta, syn::Meta::Path(_)) {
                return Err(Error::new(attribute.path().span(), "`#[fallback]` does not accept arguments"));
            }
            validate_route_failure_type(argument_type)?;
            if let Some(previous) = &policy.fallback {
                let mut error = Error::new(
                    attribute.path().span(),
                    "a generated service may declare only one `#[fallback]` method",
                );
                error.combine(Error::new(previous.method.span(), "the first fallback is declared here"));
                return Err(error);
            }
            policy.fallback = Some(FallbackPolicy {
                method: method.sig.ident.clone(),
                response_type,
            });
        } else {
            let attribute = catch_attrs[0];
            let parsed: CatchAttr = attribute.parse_args()?;
            validate_concrete_rejection_type(&parsed.rejection_type)?;
            if type_key(&parsed.rejection_type) != type_key(argument_type) {
                return Err(Error::new(
                    type_diagnostic_span(argument_type),
                    "the `#[catch(RejectionType)]` argument must exactly match the catcher's by-value parameter type",
                ));
            }
            if let Some(previous) = policy
                .catchers
                .iter()
                .find(|catcher| type_key(&catcher.rejection_type) == type_key(&parsed.rejection_type))
            {
                let rejection_type = &parsed.rejection_type;
                let mut error = Error::new(
                    attribute.path().span(),
                    format!("duplicate catcher for rejection type `{}`", quote! { #rejection_type }),
                );
                error.combine(Error::new(previous.span, "the first catcher for this type is declared here"));
                return Err(error);
            }
            policy.catchers.push(CatcherPolicy {
                method: method.sig.ident.clone(),
                rejection_type: parsed.rejection_type,
                extractor_type: parsed.extractor_type,
                response_type,
                span: attribute.path().span(),
            });
        }
    }
    Ok(policy)
}

/// Recognizes and validates one `#[before]`, `#[after]`, or `#[transform]`
/// interceptor method, returning whether the method was an interceptor.
fn parse_interceptor(policy: &mut RoutingPolicy, method: &ImplItemFn) -> syn::Result<bool> {
    let before_attrs: Vec<_> = method
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("before"))
        .collect();
    let after_attrs: Vec<_> = method.attrs.iter().filter(|attribute| attribute.path().is_ident("after")).collect();
    let transform_attrs: Vec<_> = method
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("transform"))
        .collect();
    let total = before_attrs.len() + after_attrs.len() + transform_attrs.len();
    if total == 0 {
        return Ok(false);
    }
    if has_route_attr(&method.attrs) {
        return Err(Error::new(
            method.sig.ident.span(),
            "a method cannot be both a route handler and a `#[before]`, `#[after]`, or `#[transform]` interceptor",
        ));
    }
    if method
        .attrs
        .iter()
        .any(|attribute| attribute.path().is_ident("fallback") || attribute.path().is_ident("catch"))
    {
        return Err(Error::new(
            method.sig.ident.span(),
            "an interceptor method cannot also be a `#[fallback]` or `#[catch]` policy method",
        ));
    }
    if total > 1 {
        return Err(Error::new(
            method.sig.ident.span(),
            "a method may declare only one `#[before]`, `#[after]`, or `#[transform]` interceptor annotation",
        ));
    }

    if let Some(attribute) = before_attrs.first() {
        let handlers = parse_interceptor_scope(attribute)?;
        let response_type = validate_before_signature(method, handlers.is_some())?;
        policy.befores.push(BeforeInterceptor {
            method: method.sig.ident.clone(),
            response_type,
            handlers,
        });
    } else if let Some(attribute) = after_attrs.first() {
        let handlers = parse_interceptor_scope(attribute)?;
        validate_after_signature(method)?;
        policy.afters.push(AfterInterceptor {
            method: method.sig.ident.clone(),
            handlers,
        });
    } else {
        let attribute = transform_attrs[0];
        let parsed: TransformAttr = attribute.parse_args()?;
        let (replacement_body, body_bounds, response_type) = validate_transform_signature(method, &parsed.mode)?;
        policy.transforms.push(TransformInterceptor {
            method: method.sig.ident.clone(),
            mode: parsed.mode,
            handlers: parsed.handlers,
            replacement_body,
            body_bounds,
            response_type,
            span: attribute.path().span(),
        });
    }
    Ok(true)
}

/// Parses the optional handler-name list of a `#[before]`/`#[after]` attribute.
///
/// A bare annotation (`None`) is router-wide for `#[before]` and observes every
/// generated response for `#[after]`; `#[before(a, b)]` and `#[after(a, b)]`
/// restrict the interceptor to the named handlers.
fn parse_interceptor_scope(attribute: &Attribute) -> syn::Result<Option<Vec<Ident>>> {
    match &attribute.meta {
        syn::Meta::Path(_) => Ok(None),
        syn::Meta::List(_) => {
            let handlers = attribute.parse_args_with(<syn::punctuated::Punctuated<Ident, Token![,]>>::parse_terminated)?;
            if handlers.is_empty() {
                return Err(Error::new(
                    attribute.span(),
                    "an interceptor handler list cannot be empty; omit the parentheses for a bare interceptor, which is \
                     router-wide for `#[before]` and observes every generated response for `#[after]`",
                ));
            }
            let mut names: Vec<Ident> = Vec::new();
            for handler in handlers {
                if names.contains(&handler) {
                    return Err(Error::new(
                        handler.span(),
                        format!("duplicate handler `{handler}` in interceptor scope"),
                    ));
                }
                names.push(handler);
            }
            Ok(Some(names))
        }
        syn::Meta::NameValue(_) => Err(Error::new(
            attribute.span(),
            "an interceptor scope is either empty or a `(handler, ...)` list of handler names",
        )),
    }
}

/// The parsed arguments of a `#[transform(limit = N | stream, handler, ...)]`
/// attribute.
struct TransformAttr {
    mode: TransformMode,
    handlers: Vec<Ident>,
}

/// The one grammar message every malformed `#[transform]` annotation reports.
const TRANSFORM_GRAMMAR: &str = "`#[transform]` requires one ownership mode followed by at least one handler name: \
     `#[transform(limit = N, handler, ...)]` collects a bounded `bytes::Bytes` buffer, and \
     `#[transform(stream, handler, ...)]` moves the transport body into an interceptor generic over it";

impl syn::parse::Parse for TransformAttr {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let key: Ident = input.parse().map_err(|error| Error::new(error.span(), TRANSFORM_GRAMMAR))?;
        let mode = if key == "stream" {
            TransformMode::Streaming
        } else if key == "limit" {
            let _equals: Token![=] = input.parse().map_err(|error| Error::new(error.span(), TRANSFORM_GRAMMAR))?;
            TransformMode::Buffered {
                limit: input.parse().map_err(|error| Error::new(error.span(), TRANSFORM_GRAMMAR))?,
            }
        } else {
            return Err(Error::new(key.span(), TRANSFORM_GRAMMAR));
        };
        let mut handlers: Vec<Ident> = Vec::new();
        while !input.is_empty() {
            let _comma: Token![,] = input.parse().map_err(|error| Error::new(error.span(), TRANSFORM_GRAMMAR))?;
            if input.is_empty() {
                break;
            }
            let handler: Ident = input.parse().map_err(|error| Error::new(error.span(), TRANSFORM_GRAMMAR))?;
            if handler == "stream" || handler == "limit" {
                return Err(Error::new(
                    handler.span(),
                    "a `#[transform]` interceptor selects exactly one ownership mode; `limit = N` bounded buffering and \
                     `stream` wrapping are mutually exclusive",
                ));
            }
            if handlers.contains(&handler) {
                return Err(Error::new(
                    handler.span(),
                    format!("duplicate handler `{handler}` in transform scope"),
                ));
            }
            handlers.push(handler);
        }
        if handlers.is_empty() {
            return Err(input.error(
                "`#[transform]` must name at least one handler whose body it owns, so unrelated routes are not forced to buffer or wrap",
            ));
        }
        Ok(Self { mode, handlers })
    }
}

/// Whether an interceptor may declare the one generic transport-body parameter
/// a streaming `#[transform]` needs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InterceptorGenerics {
    /// No generic parameters at all, so the call stays trivially concrete.
    Forbidden,
    /// Exactly one generic type parameter, naming the transport request body.
    BodyParameter,
}

/// Validates shared interceptor signature rules and returns the parameters that
/// follow `&self`.
fn validate_interceptor_common<'method>(
    method: &'method ImplItemFn,
    name: &str,
    generics: InterceptorGenerics,
) -> syn::Result<Vec<&'method syn::PatType>> {
    if method.sig.asyncness.is_none() {
        return Err(Error::new(
            method.sig.fn_token.span(),
            format!("`#[{name}]` interceptor methods must be async"),
        ));
    }
    if method.sig.constness.is_some() || matches!(method.sig.safety, syn::Safety::Unsafe(_)) || method.sig.abi.is_some() {
        return Err(Error::new(
            method.sig.span(),
            format!("`#[{name}]` interceptor methods cannot be const, unsafe, or extern functions"),
        ));
    }
    match generics {
        InterceptorGenerics::Forbidden => {
            if !method.sig.generics.params.is_empty() || method.sig.generics.where_clause.is_some() {
                let span = method
                    .sig
                    .generics
                    .params
                    .first()
                    .map_or_else(|| method.sig.generics.span(), syn::spanned::Spanned::span);
                return Err(Error::new(
                    span,
                    format!(
                        "`#[{name}]` interceptor methods cannot have generic parameters; only a streaming \
                         `#[transform(stream, ...)]` is generic, over its transport request body"
                    ),
                ));
            }
        }
        InterceptorGenerics::BodyParameter => {
            // Report a malformed generic list before argument or return
            // diagnostics, which quote the parameter name.
            let _ = transform_body_parameter(method)?;
        }
    }
    if let Some(attribute) = method
        .attrs
        .iter()
        .find(|attribute| attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr"))
    {
        return Err(Error::new(attribute.span(), "interceptor methods cannot be conditionally compiled"));
    }
    let mut inputs = method.sig.inputs.iter();
    let Some(FnArg::Receiver(receiver)) = inputs.next() else {
        return Err(Error::new(
            method.sig.inputs.span(),
            format!("`#[{name}]` interceptor methods must begin with `&self`"),
        ));
    };
    if !matches!(receiver.kind, syn::ReceiverKind::Reference(_, _, None)) || receiver.mutability.is_some() {
        return Err(Error::new(
            receiver.span(),
            format!("`#[{name}]` interceptor methods must begin with `&self`"),
        ));
    }
    let mut arguments = Vec::new();
    for input in inputs {
        let FnArg::Typed(argument) = input else {
            return Err(Error::new(input.span(), "interceptor methods have exactly one `&self` receiver"));
        };
        parameter_pattern(&argument.pat)?;
        if let Some(attribute) = argument.attrs.iter().find(|attribute| is_parameter_marker(attribute)) {
            return Err(Error::new(
                attribute.path().span(),
                "interceptor arguments cannot use `#[body]` or `#[capture]`",
            ));
        }
        arguments.push(argument);
    }
    Ok(arguments)
}

/// The message every malformed streaming generic list reports.
const STREAMING_GENERIC_SHAPE: &str = "a streaming `#[transform(stream, ...)]` interceptor declares exactly one generic parameter, \
     which names its transport request body; lifetime and const parameters are not part of that contract";

/// Returns the single generic transport-body parameter of a streaming
/// `#[transform]` interceptor.
fn transform_body_parameter(method: &ImplItemFn) -> syn::Result<Ident> {
    let mut body_parameter = None;
    for parameter in &method.sig.generics.params {
        match parameter {
            syn::GenericParam::Type(type_parameter) => {
                if body_parameter.is_some() {
                    return Err(Error::new(type_parameter.ident.span(), STREAMING_GENERIC_SHAPE));
                }
                body_parameter = Some(type_parameter.ident.clone());
            }
            syn::GenericParam::Lifetime(lifetime) => {
                return Err(Error::new(lifetime.lifetime.span(), STREAMING_GENERIC_SHAPE));
            }
            syn::GenericParam::Const(constant) => {
                return Err(Error::new(constant.ident.span(), STREAMING_GENERIC_SHAPE));
            }
        }
    }
    body_parameter.ok_or_else(|| {
        Error::new(
            method.sig.ident.span(),
            "a streaming `#[transform(stream, ...)]` interceptor must be generic over its transport request body, \
             for example `async fn wrap<B>(&self, parts: &RequestParts, body: B) -> BodyTransform<Wrapper<B>, R> \
             where B: http_body::Body<Data = Bytes>`",
        )
    })
}

/// Validates a `#[before]` signature and returns its short-circuit response
/// type. The context type distinguishes the two scopes, because only a
/// router-wide interceptor owns the whole mutable request head.
fn validate_before_signature(method: &ImplItemFn, per_handler: bool) -> syn::Result<Type> {
    let arguments = validate_interceptor_common(method, "before", InterceptorGenerics::Forbidden)?;
    let (expected, explanation) = if per_handler {
        (
            "SelectedContext",
            "a per-handler `#[before(handler, ...)]` interceptor runs after route selection, where the request URI \
             backs the selected route's zero-copy captures. It takes `&mut SelectedContext<'_>`, which reads the \
             method, URI, and version and mutates the headers and extensions. Drop the handler list to get a \
             router-wide `#[before]` taking `&mut BeforeContext<'_>`, which runs before resolution and may rewrite \
             the method and URI",
        )
    } else {
        (
            "BeforeContext",
            "a router-wide `#[before]` interceptor runs before route resolution and takes `&mut BeforeContext<'_>`, \
             the whole mutable request head. Name handlers (`#[before(handler, ...)]`) to run after route selection \
             instead, taking `&mut SelectedContext<'_>`",
        )
    };
    let [argument] = arguments.as_slice() else {
        return Err(Error::new(method.sig.inputs.span(), explanation));
    };
    if mutable_reference_name(&argument.ty).as_deref() != Some(expected) {
        return Err(Error::new(argument.pat.span(), explanation));
    }
    let response_type = interceptor_return_type(method, "before")?;
    extract_single_generic(&response_type, "Before", "a `#[before]` interceptor must return `Before<R>`")
}

/// Validates an `#[after]` signature.
fn validate_after_signature(method: &ImplItemFn) -> syn::Result<()> {
    let arguments = validate_interceptor_common(method, "after", InterceptorGenerics::Forbidden)?;
    let [argument] = arguments.as_slice() else {
        return Err(Error::new(
            method.sig.inputs.span(),
            "`#[after]` interceptors take exactly `&self` and one `&mut AfterContext<'_>` argument",
        ));
    };
    if mutable_reference_name(&argument.ty).as_deref() != Some("AfterContext") {
        return Err(Error::new(
            argument.pat.span(),
            "`#[after]` interceptors take exactly `&self` and one `&mut AfterContext<'_>` argument",
        ));
    }
    if let ReturnType::Type(_, ty) = &method.sig.output
        && !matches!(ty.as_ref(), Type::Tuple(tuple) if tuple.elems.is_empty())
    {
        return Err(Error::new(ty.span(), "`#[after]` interceptors must return `()`"));
    }
    Ok(())
}

/// Validates a `#[transform]` signature and returns its replacement body type
/// (if any) and short-circuit response type.
///
/// A streaming transform's replacement body is returned with its generic body
/// parameter already substituted by the generated transport body type, so every
/// later stage sees one concrete `#[body]` extraction input.
fn validate_transform_signature(method: &ImplItemFn, mode: &TransformMode) -> syn::Result<(Option<Type>, Vec<WherePredicate>, Type)> {
    let generics = match mode {
        TransformMode::Buffered { .. } => InterceptorGenerics::Forbidden,
        TransformMode::Streaming => InterceptorGenerics::BodyParameter,
    };
    let arguments = validate_interceptor_common(method, "transform", generics)?;
    let body_parameter = match mode {
        TransformMode::Buffered { .. } => None,
        TransformMode::Streaming => Some(transform_body_parameter(method)?),
    };
    let expected_body = body_parameter.as_ref().map_or_else(|| "Bytes".to_string(), Ident::to_string);
    let signature = body_parameter.as_ref().map_or_else(
        || {
            "`#[transform(limit = N, ...)]` interceptors take `&self`, one `&RequestParts`, and the collected request \
             body as `bytes::Bytes`"
                .to_string()
        },
        |parameter| {
            format!(
                "`#[transform(stream, ...)]` interceptors take `&self`, one `&RequestParts`, and the transport request \
                 body by value as their generic parameter `{parameter}`"
            )
        },
    );
    let [parts_argument, body_argument] = arguments.as_slice() else {
        return Err(Error::new(method.sig.inputs.span(), signature));
    };
    if !matches!(shared_reference_name(&parts_argument.ty).as_deref(), Some("RequestParts" | "Parts")) {
        return Err(Error::new(parts_argument.pat.span(), signature));
    }
    if terminal_type_name(&body_argument.ty).as_deref() != Some(expected_body.as_str()) {
        return Err(Error::new(body_argument.pat.span(), signature));
    }

    let response_type = interceptor_return_type(method, "transform")?;
    let (replacement_body, short_circuit) = transform_outcome(&response_type)?;
    let Some(body_parameter) = body_parameter else {
        return Ok((replacement_body, Vec::new(), short_circuit));
    };
    if token_stream_contains_ident(quote! { #short_circuit }, &body_parameter.to_string()) {
        return Err(Error::new(
            short_circuit.span(),
            format!(
                "a streaming `#[transform]` short-circuit response cannot depend on the generic body parameter `{body_parameter}`; \
                 name a concrete response type so it can join the generated response body sum"
            ),
        ));
    }
    let replacement_body = replacement_body.map(|mut replacement| {
        SubstituteBodyParameter {
            parameter: &body_parameter,
            replacement: generated_body_type(),
        }
        .visit_type_mut(&mut replacement);
        replacement
    });
    let body_bounds = method
        .sig
        .generics
        .where_clause
        .iter()
        .flat_map(|clause| &clause.predicates)
        .cloned()
        .map(|mut predicate| {
            SubstituteBodyParameter {
                parameter: &body_parameter,
                replacement: generated_body_type(),
            }
            .visit_where_predicate_mut(&mut predicate);
            predicate
        })
        .collect();
    Ok((replacement_body, body_bounds, short_circuit))
}

/// Splits a `#[transform]` return type into its replacement body (if any) and
/// its short-circuit response type.
fn transform_outcome(response_type: &Type) -> syn::Result<(Option<Type>, Type)> {
    const OUTCOME: &str = "a `#[transform]` interceptor must return `BodyTransform<B, R>` or `BodyConsumed<R>`";

    let Type::Path(path) = response_type else {
        return Err(Error::new(response_type.span(), OUTCOME));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(Error::new(response_type.span(), OUTCOME));
    };
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(Error::new(response_type.span(), OUTCOME));
    };
    let type_args: Vec<&Type> = arguments
        .args
        .iter()
        .filter_map(|argument| match argument {
            GenericArgument::Type(ty) => Some(ty),
            _ => None,
        })
        .collect();
    if segment.ident == "BodyTransform" && type_args.len() == 2 {
        Ok((Some(type_args[0].clone()), type_args[1].clone()))
    } else if segment.ident == "BodyConsumed" && type_args.len() == 1 {
        Ok((None, type_args[0].clone()))
    } else {
        Err(Error::new(response_type.span(), OUTCOME))
    }
}

/// Rewrites a streaming transform's generic body parameter into the generated
/// transport body type, so its replacement names one concrete extraction input.
struct SubstituteBodyParameter<'parameter> {
    parameter: &'parameter Ident,
    replacement: Ident,
}

impl syn::visit_mut::VisitMut for SubstituteBodyParameter<'_> {
    fn visit_type_path_mut(&mut self, i: &mut syn::TypePath) {
        if i.qself.is_none()
            && let Some(segment) = i.path.segments.first_mut()
            && segment.ident == *self.parameter
            && matches!(segment.arguments, PathArguments::None)
        {
            segment.ident = self.replacement.clone();
        }
        syn::visit_mut::visit_type_path_mut(self, i);
    }
}

/// Returns the terminal type name behind `&mut Name<...>`.
fn mutable_reference_name(ty: &Type) -> Option<String> {
    let Type::Reference(reference) = ty else {
        return None;
    };
    reference.mutability?;
    terminal_type_name(&reference.elem)
}

/// Returns the terminal type name behind `&Name<...>`.
fn shared_reference_name(ty: &Type) -> Option<String> {
    let Type::Reference(reference) = ty else {
        return None;
    };
    if reference.mutability.is_some() {
        return None;
    }
    terminal_type_name(&reference.elem)
}

/// Returns an interceptor's declared return type, requiring an explicit one.
fn interceptor_return_type(method: &ImplItemFn, name: &str) -> syn::Result<Type> {
    let ReturnType::Type(_, response_type) = &method.sig.output else {
        return Err(Error::new(
            method.sig.output.span(),
            format!("`#[{name}]` interceptors must declare an explicit return type"),
        ));
    };
    if matches!(response_type.as_ref(), Type::ImplTrait(_)) {
        return Err(Error::new(response_type.span(), "interceptor return types cannot use `impl Trait`"));
    }
    Ok(response_type.as_ref().clone())
}

/// Extracts the single generic type argument of `Wrapper<T>`.
fn extract_single_generic(ty: &Type, wrapper: &str, message: &str) -> syn::Result<Type> {
    if let Type::Path(path) = ty
        && let Some(segment) = path.path.segments.last()
        && segment.ident == wrapper
        && let PathArguments::AngleBracketed(arguments) = &segment.arguments
    {
        let type_args: Vec<&Type> = arguments
            .args
            .iter()
            .filter_map(|argument| match argument {
                GenericArgument::Type(inner) => Some(inner),
                _ => None,
            })
            .collect();
        if type_args.len() == 1 {
            return Ok(type_args[0].clone());
        }
    }
    Err(Error::new(ty.span(), message))
}

fn validate_policy_signature(method: &ImplItemFn) -> syn::Result<(&Type, Type)> {
    let policy_name = if method.attrs.iter().any(|attribute| attribute.path().is_ident("fallback")) {
        "fallback"
    } else {
        "catcher"
    };
    if method.sig.asyncness.is_none() {
        return Err(Error::new(
            method.sig.fn_token.span(),
            format!("routing {policy_name} methods must be async"),
        ));
    }
    if method.sig.constness.is_some() || matches!(method.sig.safety, syn::Safety::Unsafe(_)) || method.sig.abi.is_some() {
        return Err(Error::new(
            method.sig.span(),
            format!("routing {policy_name} methods cannot be const, unsafe, or extern functions"),
        ));
    }
    if !method.sig.generics.params.is_empty() || method.sig.generics.where_clause.is_some() {
        let span = method
            .sig
            .generics
            .params
            .first()
            .map_or_else(|| method.sig.generics.span(), syn::spanned::Spanned::span);
        return Err(Error::new(
            span,
            format!("routing {policy_name} methods cannot have generic parameters"),
        ));
    }
    if let Some(attribute) = method
        .attrs
        .iter()
        .find(|attribute| attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr"))
    {
        return Err(Error::new(
            attribute.span(),
            "routing policy methods cannot be conditionally compiled",
        ));
    }
    let mut inputs = method.sig.inputs.iter();
    let Some(FnArg::Receiver(receiver)) = inputs.next() else {
        return Err(Error::new(
            method.sig.inputs.span(),
            format!("routing {policy_name} methods must begin with `&self`"),
        ));
    };
    if !matches!(receiver.kind, syn::ReceiverKind::Reference(_, _, None)) || receiver.mutability.is_some() {
        return Err(Error::new(
            receiver.span(),
            format!("routing {policy_name} methods must begin with `&self`"),
        ));
    }
    if let Some(attribute) = receiver.attrs.iter().find(|attribute| is_parameter_marker(attribute)) {
        return Err(Error::new(
            attribute.span(),
            "routing policy receivers cannot use route extraction markers",
        ));
    }
    let Some(FnArg::Typed(argument)) = inputs.next() else {
        return Err(Error::new(
            method.sig.inputs.span(),
            format!("routing {policy_name} methods require exactly one policy argument after `&self`"),
        ));
    };
    if inputs.next().is_some() {
        return Err(Error::new(
            method.sig.inputs.span(),
            format!("routing {policy_name} methods accept only `&self` and one policy argument"),
        ));
    }
    parameter_pattern(&argument.pat)?;
    if let Some(attribute) = argument.attrs.iter().find(|attribute| is_parameter_marker(attribute)) {
        return Err(Error::new(
            attribute.path().span(),
            "routing policy arguments cannot use `#[body]` or `#[capture]`; catchers cannot recursively extract a request",
        ));
    }
    if let Type::Reference(reference) = argument.ty.as_ref() {
        return Err(Error::new(
            reference.and_token.span(),
            "routing policy arguments are passed by value",
        ));
    }
    let response_type = response_type(method)?;
    Ok((argument.ty.as_ref(), response_type))
}

fn validate_route_failure_type(ty: &Type) -> syn::Result<()> {
    let Type::Path(path) = ty else {
        return Err(Error::new(
            ty.span(),
            "a `#[fallback]` argument must be `RouteFailure<'_>` by value",
        ));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(Error::new(
            ty.span(),
            "a `#[fallback]` argument must be `RouteFailure<'_>` by value",
        ));
    };
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(Error::new(
            ty.span(),
            "a `#[fallback]` argument must be `RouteFailure<'_>` by value",
        ));
    };
    if segment.ident != "RouteFailure"
        || arguments.args.len() != 1
        || !matches!(arguments.args.first(), Some(GenericArgument::Lifetime(lifetime)) if lifetime.ident == "_")
    {
        return Err(Error::new(
            ty.span(),
            "a `#[fallback]` argument must be `RouteFailure<'_>` by value",
        ));
    }
    Ok(())
}

#[derive(Default)]
struct ConcreteRejectionType {
    error: Option<Error>,
}

impl<'ast> syn::visit::Visit<'ast> for ConcreteRejectionType {
    fn visit_type_reference(&mut self, i: &'ast syn::TypeReference) {
        if self.error.is_none() {
            self.error = Some(Error::new(
                i.span(),
                "catcher rejection types must be owned and cannot depend on a borrowed lifetime",
            ));
        }
    }

    fn visit_type_infer(&mut self, i: &'ast syn::TypeInfer) {
        if self.error.is_none() {
            self.error = Some(Error::new(
                i.span(),
                "catcher rejection types must be fully concrete and cannot contain `_`",
            ));
        }
    }

    fn visit_type_impl_trait(&mut self, i: &'ast syn::TypeImplTrait) {
        if self.error.is_none() {
            self.error = Some(Error::new(
                i.span(),
                "catcher rejection types must be concrete and cannot use `impl Trait`",
            ));
        }
    }

    fn visit_type_macro(&mut self, i: &'ast syn::TypeMacro) {
        if self.error.is_none() {
            self.error = Some(Error::new(i.span(), "catcher rejection types cannot be produced by a type macro"));
        }
    }

    fn visit_lifetime(&mut self, i: &'ast Lifetime) {
        if i.ident != "static" && self.error.is_none() {
            self.error = Some(Error::new(
                i.span(),
                "catcher rejection types cannot depend on a non-static lifetime",
            ));
        }
    }
}

fn validate_concrete_rejection_type(ty: &Type) -> syn::Result<()> {
    let mut visitor = ConcreteRejectionType::default();
    visitor.visit_type(ty);
    match visitor.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn bind_catchers(policy: &mut RoutingPolicy, handlers: &[Handler]) -> syn::Result<()> {
    let sites: Vec<(ExtractionKind, &Type)> = handlers
        .iter()
        .flat_map(|handler| &handler.arguments)
        .filter_map(|argument| match argument {
            Argument::Parts(_, ty) => Some((ExtractionKind::Parts, ty)),
            Argument::Body(_, ty) => Some((ExtractionKind::Body, ty)),
            Argument::Capture(_) => None,
        })
        .collect();

    for (catcher_index, catcher) in policy.catchers.iter().enumerate() {
        let mut matched = false;
        for (kind, extractor) in &sites {
            let applies = if let Some(explicit) = &catcher.extractor_type {
                type_key(explicit) == type_key(extractor)
            } else {
                inferred_catcher_applies(&catcher.rejection_type, extractor)
            };
            if !applies {
                continue;
            }
            matched = true;
            let extractor_key = type_key(extractor);
            if let Some(previous) = policy
                .bindings
                .iter()
                .find(|binding| binding.kind == *kind && binding.extractor_key == extractor_key)
            {
                if previous.catcher != catcher_index {
                    return Err(Error::new(
                        catcher.span,
                        format!(
                            "catchers for `{}` are ambiguous; use `from = ExtractorType` to select one exact rejection policy",
                            quote! { #extractor }
                        ),
                    ));
                }
                continue;
            }
            policy.bindings.push(CatchBinding {
                kind: *kind,
                extractor_key,
                catcher: catcher_index,
            });
        }
        if !matched {
            let detail = catcher.extractor_type.as_ref().map_or_else(
                || {
                    "no built-in extractor in this service has that rejection type; custom extractors require `from = ExtractorType`"
                        .to_string()
                },
                |extractor| format!("the service has no extractor of type `{}`", quote! { #extractor }),
            );
            return Err(Error::new(catcher.span, format!("unused extractor catcher: {detail}")));
        }
    }
    Ok(())
}

fn bind_interceptors(policy: &RoutingPolicy, handlers: &[Handler]) -> syn::Result<()> {
    let handler_for = |name: &Ident| handlers.iter().find(|handler| &handler.method == name);

    for before in &policy.befores {
        let Some(names) = &before.handlers else {
            continue;
        };
        for name in names {
            if handler_for(name).is_none() {
                return Err(Error::new(
                    name.span(),
                    format!("`#[before]` names `{name}`, which is not a `#[route]` handler in this service"),
                ));
            }
        }
    }

    for after in &policy.afters {
        if let Some(names) = &after.handlers {
            for name in names {
                if handler_for(name).is_none() {
                    return Err(Error::new(
                        name.span(),
                        format!("`#[after]` names `{name}`, which is not a `#[route]` handler in this service"),
                    ));
                }
            }
        }
    }

    let mut covered: Vec<(&Ident, &Ident)> = Vec::new();
    for transform in &policy.transforms {
        for name in &transform.handlers {
            let Some(handler) = handler_for(name) else {
                return Err(Error::new(
                    name.span(),
                    format!("`#[transform]` names `{name}`, which is not a `#[route]` handler in this service"),
                ));
            };
            if let Some((_, previous)) = covered.iter().find(|(covered_handler, _)| *covered_handler == name) {
                let mut error = Error::new(
                    name.span(),
                    format!("handler `{name}` is transformed more than once; a request body can be replaced only once"),
                );
                error.combine(Error::new(previous.span(), "the first transform for this handler is declared here"));
                return Err(error);
            }
            covered.push((name, &transform.method));
            let has_body = handler.arguments.iter().any(|argument| matches!(argument, Argument::Body(_, _)));
            if has_body && transform.replacement_body.is_none() {
                let replacement = match transform.mode {
                    TransformMode::Buffered { .. } => {
                        "return `BodyTransform<B, R>` with a concrete replacement body for `#[body]` extraction"
                    }
                    TransformMode::Streaming => {
                        "return `BodyTransform<Wrapper<B>, R>` with a replacement that wraps the transport body for `#[body]` extraction"
                    }
                };
                return Err(Error::new(
                    transform.span,
                    format!(
                        "handler `{name}` declares a `#[body]` parameter, but its `#[transform]` consumes the body without a \
                         replacement, so there is nothing left to extract; {replacement}"
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn catcher_for<'a>(policy: &'a RoutingPolicy, kind: ExtractionKind, extractor: &Type) -> Option<&'a CatcherPolicy> {
    let key = type_key(extractor);
    policy
        .bindings
        .iter()
        .find(|binding| binding.kind == kind && binding.extractor_key == key)
        .map(|binding| &policy.catchers[binding.catcher])
}

fn inferred_catcher_applies(rejection: &Type, extractor: &Type) -> bool {
    let Some(rejection) = terminal_type_name(rejection) else {
        return false;
    };
    let Some(extractor) = terminal_type_name(extractor) else {
        return false;
    };
    matches!(
        (rejection.as_str(), extractor.as_str()),
        ("MissingExtension", "ExtensionRef" | "ClonedExtension")
            | ("QueryRejection", "Query")
            | ("BodyRejection", "BytesBody" | "TextBody")
            | ("JsonRejection", "Json")
            | ("FormRejection", "Form")
    )
}

fn terminal_type_name(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    path.path.segments.last().map(|segment| segment.ident.to_string())
}

fn default_body_witness_rejection(extractor: &Type, runtime: &TokenStream2) -> TokenStream2 {
    match terminal_type_name(extractor).as_deref() {
        Some("RawBody") => quote! { ::core::convert::Infallible },
        Some("BytesBody" | "TextBody") => {
            quote! { #runtime::BodyRejection<::core::convert::Infallible> }
        }
        Some("Json") => {
            quote! { #runtime::JsonRejection<::core::convert::Infallible> }
        }
        Some("Form") => {
            quote! { #runtime::FormRejection<::core::convert::Infallible> }
        }
        Some(_) | None => quote! { _ },
    }
}

fn type_key(ty: &Type) -> String {
    quote! { #ty }.to_string()
}

fn type_diagnostic_span(ty: &Type) -> Span {
    match ty {
        Type::Path(path) => path.path.segments.last().map_or_else(|| ty.span(), |segment| segment.ident.span()),
        Type::Reference(reference) => reference.and_token.span(),
        _ => ty.span(),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "dispatch grouping performs one ordered pass from static declarations through validated overlap groups"
)]
fn build_dispatch_arms(handlers: &[Handler]) -> syn::Result<Vec<DispatchArm>> {
    let mut entries = Vec::new();
    let mut dynamic = Vec::new();
    for (handler_index, handler) in handlers.iter().enumerate() {
        if handler.kind == HandlerKind::Dynamic {
            dynamic.push(DispatchArm {
                variant: handler.variant.clone(),
                route_attrs: handler.route_attrs.clone(),
                captures: handler.captures.clone(),
                kind: DispatchKind::Direct(handler_index),
            });
            continue;
        }
        for attribute in &handler.route_attrs {
            let parsed: RouteAttr = attribute.parse_args()?;
            let RouteTarget::Static { method, path } = parsed.target else {
                return Err(Error::new(
                    attribute.span(),
                    "static handler aliases must declare a method and path",
                ));
            };
            let path_value = path.value();
            let template = PathTemplate::parse(&path_value, Grammar::default().with_segment_affixes())
                .map_err(|error| Error::new(path.span(), format!("invalid path template: {error}")))?;
            if let Some(message) = depth_limit_error(template.segments()) {
                return Err(Error::new(path.span(), message));
            }
            let mut capture_keys: Vec<_> = capture_field_names(template.segments())
                .into_iter()
                .map(|name| name.join("."))
                .collect();
            capture_keys.sort();
            let entry_index = entries.len();
            entries.push(StaticRouteEntry {
                handler: handler_index,
                attribute: attribute.clone(),
                route: Route::new(format!("__RouteramaEntry{entry_index}"), method, template),
                capture_keys,
                priority: parsed.priority,
            });
        }
    }

    if entries.is_empty() {
        return Ok(dynamic);
    }
    let routes: Vec<_> = entries.iter().map(|entry| entry.route.clone()).collect();
    let trie = build_trie(&routes);
    let mut groups = Vec::new();
    let mut plans = vec![None; entries.len()];
    collect_route_groups(&trie.root, &mut groups, &mut plans);
    groups.sort_by_key(|group| group.iter().copied().min().unwrap_or(usize::MAX));

    let mut dispatches = Vec::<DispatchArm>::new();
    let mut used_variants: Vec<String> = handlers.iter().map(|handler| handler.variant.to_string()).collect();
    let mut overlap_index = 0_usize;
    for group in groups {
        let candidates = if group.len() == 1 {
            vec![entries[group[0]].handler]
        } else {
            validate_overlap(&entries, handlers, &group, &plans)?
        };
        let representative_entry = if group.len() == 1 {
            group[0]
        } else {
            let highest = candidates[0];
            *group
                .iter()
                .find(|entry| entries[**entry].handler == highest)
                .expect("the sorted candidate came from this overlap group")
        };
        let representative_handler = entries[representative_entry].handler;
        let captures = handlers[representative_handler].captures.clone();
        let kind_matches = |dispatch: &DispatchArm| match (&dispatch.kind, group.len()) {
            (DispatchKind::Direct(existing), 1) => *existing == candidates[0],
            (DispatchKind::Overlap(existing), _) => existing.as_slice() == candidates.as_slice(),
            (DispatchKind::Direct(_), _) => false,
        };
        if let Some(dispatch) = dispatches
            .iter_mut()
            .find(|dispatch| kind_matches(dispatch) && capture_schema_key(&dispatch.captures) == capture_schema_key(&captures))
        {
            dispatch.route_attrs.push(entries[representative_entry].attribute.clone());
            continue;
        }
        let variant = if group.len() == 1 {
            handlers[candidates[0]].variant.clone()
        } else {
            loop {
                let candidate = format!("__RouteramaPolicy{overlap_index}");
                overlap_index += 1;
                if !used_variants.contains(&candidate) {
                    used_variants.push(candidate.clone());
                    break Ident::new(&candidate, handlers[candidates[0]].variant.span());
                }
            }
        };
        let kind = if group.len() == 1 {
            DispatchKind::Direct(candidates[0])
        } else {
            DispatchKind::Overlap(candidates)
        };
        dispatches.push(DispatchArm {
            variant,
            route_attrs: vec![entries[representative_entry].attribute.clone()],
            captures,
            kind,
        });
    }
    dispatches.extend(dynamic);
    Ok(dispatches)
}

fn collect_route_groups(node: &Node, groups: &mut Vec<Vec<usize>>, plans: &mut [Option<String>]) {
    collect_leaf_bucket(&node.exact, groups, plans);
    collect_leaf_bucket(&node.rest, groups, plans);
    for child in node.literals.values() {
        collect_route_groups(child, groups, plans);
    }
    for child in node.affix.values() {
        collect_route_groups(child, groups, plans);
    }
    if let Some(child) = &node.single {
        collect_route_groups(child, groups, plans);
    }
}

fn collect_leaf_bucket(leaves: &[crate::trie::Leaf], groups: &mut Vec<Vec<usize>>, plans: &mut [Option<String>]) {
    let mut bucket_groups: Vec<(&str, Option<&str>, Vec<usize>)> = Vec::new();
    for leaf in leaves {
        plans[leaf.route_index] = Some(capture_plan_key(&leaf.vars));
        if let Some((_, _, routes)) = bucket_groups
            .iter_mut()
            .find(|(method, verb, _)| *method == leaf.method && *verb == leaf.verb.as_deref())
        {
            routes.push(leaf.route_index);
        } else {
            bucket_groups.push((leaf.method.as_str(), leaf.verb.as_deref(), vec![leaf.route_index]));
        }
    }
    groups.extend(bucket_groups.into_iter().map(|(_, _, routes)| routes));
}

fn capture_plan_key(plans: &[VarPlan]) -> String {
    plans
        .iter()
        .map(|plan| match plan {
            VarPlan::Span { field, key, a, b } => format!("span:{field}:{key}:{a}:{b}"),
            VarPlan::Rest { field, key, a } => format!("rest:{field}:{key}:{a}"),
            VarPlan::Affix {
                field,
                key,
                a,
                prefix_len,
                suffix_len,
            } => format!("affix:{field}:{key}:{a}:{prefix_len}:{suffix_len}"),
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn validate_overlap(
    entries: &[StaticRouteEntry],
    handlers: &[Handler],
    group: &[usize],
    plans: &[Option<String>],
) -> syn::Result<Vec<usize>> {
    let first = &entries[group[0]];
    let mut error = None;
    for entry_index in group {
        let entry = &entries[*entry_index];
        if entry.priority.is_none() {
            combine_error(
                &mut error,
                Error::new(
                    entry.attribute.path().span(),
                    "overlapping routes require an explicit `priority = <integer>` on every declaration",
                ),
            );
        }
        let compatible_positions = entry.capture_keys == first.capture_keys && plans[*entry_index] == plans[group[0]];
        if !compatible_positions {
            combine_error(
                &mut error,
                Error::new(
                    entry.attribute.path().span(),
                    "overlapping routes must use identical capture names and capture positions",
                ),
            );
        }
        if compatible_positions
            && capture_schema_key(&handlers[entry.handler].captures) != capture_schema_key(&handlers[first.handler].captures)
        {
            combine_error(
                &mut error,
                Error::new(
                    entry.attribute.path().span(),
                    "overlapping routes must use identical concrete capture types",
                ),
            );
        }
        if group
            .iter()
            .any(|other| *other != *entry_index && entries[*other].handler == entry.handler)
        {
            combine_error(
                &mut error,
                Error::new(
                    entry.attribute.path().span(),
                    "duplicate method/path aliases on one handler are not candidates; remove the duplicate declaration",
                ),
            );
        }
    }
    for (position, left_index) in group.iter().enumerate() {
        let Some(left) = entries[*left_index].priority.as_ref() else {
            continue;
        };
        for right_index in &group[position + 1..] {
            let Some(right) = entries[*right_index].priority.as_ref() else {
                continue;
            };
            if left.value == right.value {
                combine_error(
                    &mut error,
                    Error::new(right.span, format!("overlapping routes cannot share priority {}", right.value)),
                );
            }
        }
    }
    if let Some(error) = error {
        return Err(error);
    }
    let mut ordered = group.to_vec();
    ordered.sort_by(|left, right| {
        entries[*right]
            .priority
            .as_ref()
            .expect("validated explicit priority")
            .value
            .cmp(&entries[*left].priority.as_ref().expect("validated explicit priority").value)
    });
    let candidates: Vec<_> = ordered.iter().map(|entry| entries[*entry].handler).collect();
    for (position, higher_entry) in ordered.iter().enumerate().take(ordered.len() - 1) {
        let higher = &handlers[entries[*higher_entry].handler].predicates;
        if higher.is_empty() {
            return Err(Error::new(
                entries[*higher_entry].attribute.path().span(),
                "a predicate-free overlapping candidate must have the lowest priority because it matches every request",
            ));
        }
        for lower_entry in &ordered[position + 1..] {
            let lower = &handlers[entries[*lower_entry].handler].predicates;
            if higher.same_values(lower) {
                return Err(Error::new(
                    entries[*lower_entry].attribute.path().span(),
                    "overlapping candidates with identical predicates make the lower priority unreachable",
                ));
            }
        }
    }
    Ok(candidates)
}

fn capture_schema_key(captures: &[(Ident, Type)]) -> String {
    let mut fields: Vec<_> = captures.iter().map(|(name, ty)| format!("{}:{}", name, type_key(ty))).collect();
    fields.sort();
    fields.join("|")
}

fn combine_error(slot: &mut Option<Error>, next: Error) {
    if let Some(error) = slot {
        error.combine(next);
    } else {
        *slot = Some(next);
    }
}

fn parse_handler(method: &ImplItemFn) -> syn::Result<Handler> {
    validate_signature(method)?;
    if let Some(attribute) = method
        .attrs
        .iter()
        .find(|attribute| attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr"))
    {
        return Err(Error::new(
            attribute.span(),
            "`#[router]` does not support conditionally compiled route handlers",
        ));
    }
    let route_attrs: Vec<_> = method
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("route"))
        .cloned()
        .collect();
    let kind = route_kind(&route_attrs)?;
    if kind == HandlerKind::Dynamic {
        for attribute in &route_attrs {
            let parsed: RouteAttr = attribute.parse_args()?;
            if let Some(priority) = parsed.priority {
                return Err(Error::new(
                    priority.span,
                    "configured dynamic routes reject `priority`: runtime registrations must not overlap, and static routes remain direct",
                ));
            }
        }
    }
    let predicates = route_predicates(&route_attrs, kind)?;
    let static_headers = route_static_headers(&route_attrs, kind)?;
    let capture_names = if kind == HandlerKind::Static {
        route_capture_names(&route_attrs)?
    } else {
        Vec::new()
    };
    let variant = variant_name(&method.sig.ident)?;
    let (captures, arguments, borrows_path) = handler_parameters(method, kind, &capture_names)?;
    let response_type = response_type(method)?;

    Ok(Handler {
        method: method.sig.ident.clone(),
        variant,
        kind,
        route_attrs,
        predicates,
        static_headers,
        captures,
        arguments,
        response_type,
        borrows_path,
    })
}

fn route_kind(route_attrs: &[Attribute]) -> syn::Result<HandlerKind> {
    match route_declaration(route_attrs)? {
        Some(RouteDeclaration::Static) => Ok(HandlerKind::Static),
        Some(RouteDeclaration::Dynamic) => Ok(HandlerKind::Dynamic),
        None => Err(Error::new(Span::call_site(), "service handler is missing a route declaration")),
    }
}

fn route_predicates(route_attrs: &[Attribute], kind: HandlerKind) -> syn::Result<RoutePredicates> {
    let mut first: Option<RoutePredicates> = None;
    for attribute in route_attrs {
        let parsed: RouteAttr = attribute.parse_args()?;
        if kind == HandlerKind::Static && matches!(&parsed.target, RouteTarget::Dynamic) {
            continue;
        }
        if let Some(expected) = &first {
            if !expected.same_values(&parsed.predicates) {
                let span = parsed
                    .predicates
                    .differing_literal(expected)
                    .map_or_else(|| attribute.span(), LitStr::span);
                return Err(Error::new(
                    span,
                    "every static `#[route]` alias on one handler must declare identical `host`, `consumes`, and `produces` predicates",
                ));
            }
        } else {
            first = Some(parsed.predicates);
        }
    }
    Ok(first.unwrap_or_default())
}

fn route_static_headers(route_attrs: &[Attribute], kind: HandlerKind) -> syn::Result<Vec<StaticHeader>> {
    let mut first: Option<Vec<StaticHeader>> = None;
    for attribute in route_attrs {
        let parsed: RouteAttr = attribute.parse_args()?;
        if kind == HandlerKind::Static && matches!(&parsed.target, RouteTarget::Dynamic) {
            continue;
        }
        if let Some(expected) = &first {
            if !same_static_headers(&parsed.static_headers, expected) {
                let span = differing_static_header(&parsed.static_headers, expected).map_or_else(|| attribute.span(), LitStr::span);
                return Err(Error::new(
                    span,
                    "every `#[route]` alias on one handler must declare identical static response-header operations",
                ));
            }
        } else {
            first = Some(parsed.static_headers);
        }
    }
    Ok(first.unwrap_or_default())
}

type HandlerParameters = (Vec<(Ident, Type)>, Vec<Argument>, bool);

fn handler_parameters(method: &ImplItemFn, kind: HandlerKind, capture_names: &[String]) -> syn::Result<HandlerParameters> {
    let mut captures = Vec::new();
    let mut arguments = Vec::new();
    let mut borrows_path = false;
    let mut body_span = None;

    for input in method.sig.inputs.iter().skip(1) {
        let FnArg::Typed(input) = input else {
            return Err(Error::new(input.span(), "service handlers must have exactly one `&self` receiver"));
        };
        let pattern = parameter_pattern(input.pat.as_ref())?;
        let markers = parameter_markers(input)?;
        if markers.body.is_some() && markers.capture.is_some() {
            return Err(Error::new(
                input.span(),
                "a handler parameter cannot be both `#[body]` and `#[capture]`",
            ));
        }
        let is_static_capture = kind == HandlerKind::Static && capture_names.iter().any(|capture| capture == &pattern.ident.to_string());
        if markers.body.is_some() && is_static_capture {
            return Err(Error::new(
                input.span(),
                "a static path capture cannot also consume the request body",
            ));
        }

        if markers.body.is_some() {
            let span = pattern.ident.span();
            if let Some(first_span) = body_span {
                let mut error = Error::new(
                    span,
                    "a route handler may have at most one `#[body]` parameter because the request body can be consumed only once",
                );
                error.combine(Error::new(first_span, "the first request-body consumer is here"));
                return Err(error);
            }
            if matches!(input.ty.as_ref(), Type::Reference(_)) {
                return Err(Error::new(
                    input.ty.span(),
                    "`#[body]` parameters must own their extracted value; use an owned `RawBody<B>`, `BytesBody<LIMIT>`, `TextBody<LIMIT>`, or another `FromRequestBody` type",
                ));
            }
            body_span = Some(span);
            let ty = input.ty.as_ref().clone();
            arguments.push(Argument::Body(pattern.ident.clone(), ty));
        } else if is_static_capture {
            let (capture_type, borrows) = capture_type(input.ty.as_ref())?;
            borrows_path |= borrows;
            captures.push((pattern.ident.clone(), capture_type));
            arguments.push(Argument::Capture(pattern.ident.clone()));
        } else if markers.capture.is_some() {
            if kind == HandlerKind::Static {
                return Err(Error::new(
                    input.span(),
                    format!(
                        "`#[capture]` parameter `{}` is absent from this static route template",
                        pattern.ident
                    ),
                ));
            }
            if matches!(input.ty.as_ref(), Type::Reference(_)) {
                return Err(Error::new(input.ty.span(), "dynamic `#[capture]` parameters must be owned"));
            }
            let ty = input.ty.as_ref().clone();
            captures.push((pattern.ident.clone(), ty));
            arguments.push(Argument::Capture(pattern.ident.clone()));
        } else {
            arguments.push(Argument::Parts(pattern.ident.clone(), input.ty.as_ref().clone()));
        }
    }

    if kind == HandlerKind::Static {
        let mut declared: Vec<_> = captures.iter().map(|(name, _)| name.to_string()).collect();
        declared.sort();
        let mut expected = capture_names.to_vec();
        expected.sort();
        if declared != expected {
            return Err(Error::new(
                method.sig.ident.span(),
                format!(
                    "handler `{}` capture parameters {} do not match its path captures {}",
                    method.sig.ident,
                    fmt_names(&declared),
                    fmt_names(&expected),
                ),
            ));
        }
    }

    Ok((captures, arguments, borrows_path))
}

#[derive(Default)]
struct ParameterMarkers {
    body: Option<Span>,
    capture: Option<Span>,
}

fn parameter_markers(input: &syn::PatType) -> syn::Result<ParameterMarkers> {
    let mut markers = ParameterMarkers::default();
    for attribute in input.attrs.iter().filter(|attribute| is_parameter_marker(attribute)) {
        if !matches!(attribute.meta, syn::Meta::Path(_)) {
            return Err(Error::new(attribute.span(), "handler parameter markers do not accept arguments"));
        }
        let slot = if attribute.path().is_ident("body") {
            &mut markers.body
        } else {
            &mut markers.capture
        };
        if slot.replace(attribute.span()).is_some() {
            return Err(Error::new(attribute.span(), "duplicate handler parameter marker"));
        }
    }
    Ok(markers)
}

fn is_parameter_marker(attribute: &Attribute) -> bool {
    attribute.path().is_ident("body") || attribute.path().is_ident("capture")
}

fn parameter_pattern(pattern: &Pat) -> syn::Result<&syn::PatIdent> {
    let Pat::Ident(pattern) = pattern else {
        return Err(Error::new(
            pattern.span(),
            "service handler parameters must use simple identifier patterns",
        ));
    };
    if pattern.by_ref.is_some() || pattern.subpat.is_some() {
        return Err(Error::new(
            pattern.span(),
            "service handler parameters must use simple identifier patterns",
        ));
    }
    Ok(pattern)
}

fn response_type(method: &ImplItemFn) -> syn::Result<Type> {
    let ReturnType::Type(_, response_type) = &method.sig.output else {
        return Err(Error::new(
            method.sig.output.span(),
            "service handlers must declare an explicit response type",
        ));
    };
    if matches!(response_type.as_ref(), Type::ImplTrait(_)) {
        return Err(Error::new(
            response_type.span(),
            "service handler response types cannot use `impl Trait`",
        ));
    }
    Ok(response_type.as_ref().clone())
}

fn validate_signature(method: &ImplItemFn) -> syn::Result<()> {
    if method.sig.asyncness.is_none() {
        return Err(Error::new(method.sig.fn_token.span(), "service handlers must be async"));
    }
    if method.sig.constness.is_some() || matches!(method.sig.safety, syn::Safety::Unsafe(_)) || method.sig.abi.is_some() {
        return Err(Error::new(
            method.sig.span(),
            "service handlers cannot be const, unsafe, or extern functions",
        ));
    }
    if !method.sig.generics.params.is_empty() || method.sig.generics.where_clause.is_some() {
        return Err(Error::new(
            method.sig.generics.span(),
            "service handlers cannot have generic parameters",
        ));
    }
    let Some(FnArg::Receiver(receiver)) = method.sig.inputs.first() else {
        return Err(Error::new(method.sig.inputs.span(), "service handlers must begin with `&self`"));
    };
    if let Some(attribute) = receiver.attrs.iter().find(|attribute| is_parameter_marker(attribute)) {
        return Err(Error::new(
            attribute.span(),
            "`#[body]` and `#[capture]` may annotate typed handler parameters only",
        ));
    }
    if !matches!(receiver.kind, syn::ReceiverKind::Reference(_, _, None)) || receiver.mutability.is_some() {
        return Err(Error::new(receiver.span(), "service handlers must begin with `&self`"));
    }
    if let Some(input) = method.sig.inputs.iter().skip(1).find_map(|input| match input {
        FnArg::Typed(input) if matches!(input.ty.as_ref(), Type::ImplTrait(_)) => Some(input),
        FnArg::Receiver(_) | FnArg::Typed(_) => None,
    }) {
        return Err(Error::new(
            input.ty.span(),
            "service handler parameters cannot use `impl Trait`; name a concrete extractor type",
        ));
    }
    Ok(())
}

fn route_capture_names(route_attrs: &[Attribute]) -> syn::Result<Vec<String>> {
    let mut first = None;
    for attribute in route_attrs {
        let RouteAttr { target, .. } = attribute.parse_args()?;
        let RouteTarget::Static { path, .. } = target else {
            return Err(Error::new(attribute.span(), "static route aliases must declare a method and path"));
        };
        let path_value = path.value();
        let template = PathTemplate::parse(&path_value, Grammar::default().with_segment_affixes())
            .map_err(|error| Error::new(path.span(), format!("invalid path template: {error}")))?;
        if let Some(message) = depth_limit_error(template.segments()) {
            return Err(Error::new(path.span(), message));
        }
        let mut captures: Vec<_> = capture_field_names(template.segments())
            .into_iter()
            .map(|name| route_field_name(name.join(".")))
            .collect();
        captures.sort();
        if first.as_ref().is_some_and(|expected| expected != &captures) {
            return Err(Error::new(
                path.span(),
                "every `#[route]` on one handler must capture the same path variables",
            ));
        }
        first = Some(captures);
    }
    Ok(first.unwrap_or_default())
}

fn capture_type(handler_type: &Type) -> syn::Result<(Type, bool)> {
    let mut capture_type = handler_type.clone();
    if let Type::Reference(reference) = &mut capture_type
        && matches!(reference.elem.as_ref(), Type::Path(path) if path.path.is_ident("str"))
    {
        if reference.mutability.is_some() {
            return Err(Error::new(handler_type.span(), "borrowed string captures must use `&str`"));
        }
        if reference.lifetime.as_ref().is_some_and(|lifetime| lifetime.ident != "_") {
            return Err(Error::new(handler_type.span(), "borrowed string captures must use `&str`"));
        }
        reference.lifetime = Some(syn::Lifetime::new("'p", Span::call_site()));
        return Ok((capture_type, true));
    }
    if let Type::Path(path) = &mut capture_type
        && let Some(segment) = path.path.segments.last_mut()
        && segment.ident == "Cow"
        && let PathArguments::AngleBracketed(arguments) = &mut segment.arguments
    {
        let Some(GenericArgument::Lifetime(lifetime)) = arguments.args.first_mut() else {
            return Err(Error::new(handler_type.span(), "borrowed `Cow` captures must use `Cow<'_, str>`"));
        };
        if lifetime.ident != "_" {
            return Err(Error::new(handler_type.span(), "borrowed `Cow` captures must use `Cow<'_, str>`"));
        }
        *lifetime = syn::Lifetime::new("'p", lifetime.apostrophe);
        return Ok((capture_type, true));
    }
    Ok((capture_type, false))
}

fn validate_handlers(handlers: &[Handler]) -> syn::Result<()> {
    let mut variants: Vec<String> = Vec::with_capacity(handlers.len());
    for handler in handlers {
        let name = handler.variant.to_string();
        if variants.contains(&name) {
            return Err(Error::new(
                handler.method.span(),
                format!("handler names generate the duplicate route variant `{name}`"),
            ));
        }
        variants.push(name);
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "the where-clause contract enumerates every parts, body, transform, and interceptor bound in one place"
)]
fn route_contract(
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

fn token_stream_contains_ident(tokens: TokenStream2, name: &str) -> bool {
    tokens.into_iter().any(|token| match token {
        TokenTree::Ident(ident) => ident.to_string().trim_start_matches("r#") == name,
        TokenTree::Group(group) => token_stream_contains_ident(group.stream(), name),
        TokenTree::Punct(_) | TokenTree::Literal(_) => false,
    })
}

struct RequestPartsLifetime<'a> {
    lifetime: &'a Lifetime,
    error: Option<Error>,
    /// The lifetimes the enclosing higher-ranked binders (`for<'a>`) declare;
    /// they are bound by the type itself and are not free in the handler.
    shadowed: ShadowedLifetimes,
}

impl<'a> RequestPartsLifetime<'a> {
    fn new(lifetime: &'a Lifetime) -> Self {
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
    fn rewrite(mut self, ty: &mut Type) -> syn::Result<()> {
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

/// The lifetimes declared by the higher-ranked binders (`for<'a>`) a type
/// visitor is currently inside.
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
struct StaticAnonymousLifetimes {
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

fn generated_idents(handlers: &[Handler]) -> GeneratedIdents {
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

fn generated_ident(base: &str, handlers: &[Handler]) -> Ident {
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

fn variant_name(method: &Ident) -> syn::Result<Ident> {
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

fn fmt_names(names: &[String]) -> String {
    if names.is_empty() {
        "{}".to_string()
    } else {
        format!("{{{}}}", names.join(", "))
    }
}

fn has_route_attr(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| attribute.path().is_ident("route"))
}

fn has_policy_attr(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("fallback")
            || attribute.path().is_ident("catch")
            || attribute.path().is_ident("before")
            || attribute.path().is_ident("after")
            || attribute.path().is_ident("transform")
    })
}

trait ImplItemAttributes {
    fn attrs(&self) -> &[Attribute];
}

impl ImplItemAttributes for ImplItem {
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
    use quote::quote;

    use super::*;

    fn expand_router(item: TokenStream2) -> syn::Result<TokenStream2> {
        expand(
            syn::parse2(item).expect("test input is a syntactically valid impl"),
            None,
            false,
            false,
        )
    }

    fn expand_fixed_router(state: Type, item: TokenStream2) -> syn::Result<TokenStream2> {
        expand(
            syn::parse2(item).expect("test input is a syntactically valid impl"),
            Some(state),
            false,
            false,
        )
    }

    fn expand_heterogeneous_router(item: TokenStream2) -> syn::Result<TokenStream2> {
        expand_with_data(
            syn::parse2(item).expect("test input is a syntactically valid impl"),
            None,
            false,
            false,
            true,
        )
    }

    fn expand_fixed_mounted_router(state: Type, item: TokenStream2) -> syn::Result<TokenStream2> {
        expand(
            syn::parse2(item).expect("test input is a syntactically valid impl"),
            Some(state),
            true,
            false,
        )
    }

    #[cfg(feature = "tower")]
    fn expand_tower_router(state: Option<Type>, item: TokenStream2) -> syn::Result<TokenStream2> {
        expand(
            syn::parse2(item).expect("test input is a syntactically valid impl"),
            state,
            false,
            true,
        )
    }

    fn has_route_method_attribute(file: &syn::File) -> bool {
        file.items.iter().any(|item| {
            let syn::Item::Impl(item) = item else {
                return false;
            };
            item.items.iter().any(|item| {
                let syn::ImplItem::Fn(method) = item else {
                    return false;
                };
                method.attrs.iter().any(|attribute| attribute.path().is_ident("route"))
            })
        })
    }

    #[cfg(feature = "tower")]
    fn generated_method_signature(generated: &TokenStream2, name: &str) -> String {
        let file: syn::File = syn::parse2(generated.clone()).expect("the expansion is valid Rust");
        file.items
            .iter()
            .filter_map(|item| match item {
                syn::Item::Impl(item) => Some(item),
                _ => None,
            })
            .flat_map(|item| &item.items)
            .find_map(|item| match item {
                syn::ImplItem::Fn(method) if method.sig.ident == name => Some(quote! { #method }.to_string()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("the expansion contains a generated `{name}` method"))
    }

    #[test]
    fn valid_router_generates_an_http_entry_and_encapsulates_symbols() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/books")]
                async fn list(&self, method: Method) -> String {
                    method.to_string()
                }

                #[route(GET, "/books/{id}")]
                async fn get(&self, id: u32, headers: HeaderMap) -> StatusCode {
                    response_with_id(headers, id)
                }
            }
        })
        .expect("valid service");
        let code = generated.to_string();
        assert!(code.contains("async fn route"), "{code}");
        assert!(code.contains("mod __routerama_Api"), "{code}");
        assert!(code.contains("enum ApiRoute"), "{code}");
        assert!(code.contains("struct ApiRouteResolver"), "{code}");
        assert!(code.contains("http :: Request < __RouteramaBody >"), "{code}");
        assert!(code.contains("__RouteramaState : ? :: core :: marker :: Sized"), "{code}");
        assert!(code.contains("for < '__routerama_request >"), "{code}");
        assert!(
            code.contains("FromRequestParts < '__routerama_request , __RouteramaState"),
            "{code}"
        );
        assert!(code.contains("IntoResponse :: into_response"), "{code}");
        assert!(code.contains(":: routerama :: response :: IntoResponse"), "{code}");
        assert!(code.contains(":: routerama :: response :: Response"), "{code}");
        assert!(code.contains("resolve_error_response"), "{code}");
        assert!(code.contains("enum ApiResponseBody"), "{code}");
        assert!(code.contains("enum ApiResponseBodyError"), "{code}");
        assert!(code.contains("http_body :: Body for ApiResponseBody"), "{code}");
        assert!(code.contains("Error = impl :: core :: error :: Error"), "{code}");
        assert!(
            code.contains("use < __RouteramaBody , __RouteramaState"),
            "bare routers retain state in the precise opaque capture: {code}"
        );
        assert_eq!(
            code.matches("SendBoxBody").count(),
            2,
            "only the two uncaught request-parts rejection arms erase their body: {code}"
        );
        assert!(code.contains("{ body })"), "success branches keep their concrete body: {code}");
        assert!(
            !code.contains("unsafe"),
            "generated response polling uses safe pin projection: {code}"
        );
        assert!(!code.contains("host_matches"), "plain routes do not inspect Host: {code}");
        assert!(
            !code.contains("content_type_matches"),
            "plain routes do not inspect Content-Type: {code}"
        );
        assert!(!code.contains("accepts"), "plain routes do not inspect Accept: {code}");
        assert!(
            !code.contains("headers_mut ()"),
            "plain routes do not mutate response headers: {code}"
        );
        assert!(
            !code.contains("route predicate rejection"),
            "plain services have no predicate body source: {code}"
        );
        assert!(
            !code.contains("__routerama_failure_stage"),
            "plain services have no candidate state: {code}"
        );
        assert!(
            !code.contains("__RouteramaFixedStateContract") && !code.contains("__routerama_validate_fixed_state"),
            "bare routers emit no fixed-state validation items: {code}"
        );
        assert!(
            !code.contains("routing fallback response"),
            "plain services have no fallback source: {code}"
        );
        assert!(
            !code.contains("extractor catcher response"),
            "plain services have no catcher source: {code}"
        );
        let file: syn::File = syn::parse2(generated).expect("expansion is a valid Rust file");
        assert_eq!(
            file.items.len(),
            2,
            "only the private module and original impl remain at module scope"
        );
        assert!(
            file.items
                .iter()
                .all(|item| !matches!(item, syn::Item::Enum(_) | syn::Item::Struct(_))),
            "generated private types must not escape into the parent module"
        );
        assert!(!has_route_method_attribute(&file), "handler route attributes are consumed: {code}");
    }

    #[test]
    fn heterogeneous_router_uses_runtime_data_sums() {
        let generated = expand_heterogeneous_router(quote! {
            impl Api {
                #[route(GET, "/view")]
                async fn view(&self) -> ViewResponse {
                    view_response()
                }

                #[route(GET, "/bytes")]
                async fn bytes(&self) -> BytesResponse {
                    bytes_response()
                }
            }
        })
        .expect("valid heterogeneous service");
        let code = generated.to_string();

        assert!(code.contains("response :: EitherData"), "{code}");
        assert!(!code.contains("enum ApiResponseData"), "{code}");
        assert!(
            !code.contains("Data = :: routerama :: __private :: route :: bytes :: Bytes"),
            "{code}"
        );
    }

    #[test]
    #[cfg(feature = "tower")]
    fn explicit_tower_contract_generates_send_exact_static_and_dynamic_adapters() {
        let fixed_tokens = expand_tower_router(
            Some(syn::parse_quote!(AppState)),
            quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self) -> Response<StreamBody> {
                        response()
                    }
                }
            },
        )
        .expect("an all-Send fixed service supports the exact Tower adapter");
        let fixed_signature = generated_method_signature(&fixed_tokens, "tower_service");
        let fixed = fixed_tokens.to_string();
        assert!(fixed.contains("pub fn tower_service"), "{fixed}");
        assert!(fixed.contains("Future : :: core :: marker :: Send"), "{fixed}");
        assert!(
            fixed.contains("Error = impl :: core :: error :: Error + :: core :: marker :: Send"),
            "{fixed}"
        );
        assert!(fixed.contains("GeneratedExactRoute"), "{fixed}");
        assert!(!fixed.contains("SendBoxBody"), "{fixed}");
        assert!(!fixed_signature.contains("StreamBody"), "{fixed_signature}");
        assert!(!fixed_signature.contains("Response < StreamBody >"), "{fixed_signature}");
        assert!(!fixed.contains("pub enum ApiResponseBody"), "{fixed}");

        let dynamic_tokens = expand_tower_router(
            None,
            quote! {
                impl Api {
                    #[route(dynamic)]
                    async fn plugin(&self, #[capture] name: String) -> String {
                        name
                    }
                }
            },
        )
        .expect("a configured service supports the exact Tower adapter");
        let dynamic_signature = generated_method_signature(&dynamic_tokens, "tower_service");
        let dynamic = dynamic_tokens.to_string();
        assert!(dynamic.contains("pub fn tower_service"), "{dynamic}");
        assert!(dynamic.contains("__RouteramaRouterHandle"), "{dynamic}");
        assert!(dynamic.contains("GeneratedExactConfiguredRoute"), "{dynamic}");
        assert!(
            dynamic.contains("RouteService :: new ((__routerama_router_handle , __routerama_service_handle)"),
            "{dynamic}"
        );
        assert!(!dynamic.contains("SendBoxBody"), "{dynamic}");
        assert!(!dynamic_signature.contains("String"), "{dynamic_signature}");
    }

    #[test]
    #[cfg(feature = "tower")]
    fn explicit_tower_contract_rejects_a_conflicting_static_method() {
        let error = expand_tower_router(
            None,
            quote! {
                impl Api {
                    fn tower_service(&self) {}

                    #[route(GET, "/")]
                    async fn home(&self) -> StatusCode {
                        StatusCode::NO_CONTENT
                    }
                }
            },
        )
        .expect_err("generated static Tower adapters cannot replace an application method");
        assert!(error.to_string().contains("cannot generate `tower_service`"), "{error}");
    }

    #[test]
    #[cfg(not(feature = "tower"))]
    fn explicit_tower_contract_requires_the_cargo_feature() {
        let error = expand(
            syn::parse2(quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self) -> StatusCode {
                        StatusCode::NO_CONTENT
                    }
                }
            })
            .expect("test input is valid"),
            None,
            false,
            true,
        )
        .expect_err("Tower generation requires its additive Cargo feature");
        assert!(error.to_string().contains("requires Routerama's `tower` Cargo feature"), "{error}");
    }

    #[test]
    fn fixed_router_generates_an_explicit_static_first_mount_entry() {
        let generated = expand_fixed_mounted_router(
            syn::parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self) -> StatusCode {
                        StatusCode::NO_CONTENT
                    }
                }
            },
        )
        .expect("fixed service supports explicit mounts");
        let code = generated.to_string();

        assert!(code.contains("async fn route_with_erased_mounts"), "{code}");
        // The entry is generic over the mount delegate rather than naming
        // `ErasedMountRouter`, which is what lets one method serve both the
        // local and the `Send` mount router.
        assert!(
            code.contains("__RouteramaMounts : :: routerama :: route :: __private :: MountDelegate < __RouteramaBody , AppState >"),
            "{code}"
        );
        assert!(!code.contains("ErasedMountRouter"), "{code}");
        assert!(code.contains(":: Left { body }"), "{code}");
        assert!(code.contains(":: Right { body }"), "{code}");
        assert!(code.contains("ResolveError :: NotFound"), "{code}");
        assert!(code.contains("Request :: from_parts"), "{code}");
    }

    #[test]
    fn generic_router_rejects_an_erased_mount_contract() {
        let error = expand(
            syn::parse2(quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self) -> StatusCode {
                        StatusCode::NO_CONTENT
                    }
                }
            })
            .expect("test input is valid"),
            None,
            true,
            false,
        )
        .expect_err("erased mounts require fixed state");

        assert!(error.to_string().contains("requires a fixed `state"), "{error}");
    }

    #[test]
    fn fixed_state_specializes_the_public_signature_bounds_and_opaque_capture() {
        let state: Type = syn::parse_quote!(self::state::AppState<super::Dependency>);
        let generated = expand_fixed_router(
            state,
            quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self, projected: State<Projected>) -> Response {
                        response(projected)
                    }
                }
            },
        )
        .expect("a qualified concrete state type is valid");
        let code = generated.to_string();

        assert!(
            code.contains("state : & self :: state :: AppState < super :: Dependency >"),
            "{code}"
        );
        assert!(
            !code.contains("FromRequestParts < '__routerama_request"),
            "a fixed state resolves the extractor contract concretely: {code}"
        );
        assert!(
            code.contains("type __RouteramaFixedStateContract = self :: state :: AppState < super :: Dependency >"),
            "the impl-local state alias preserves both `self` and `super`: {code}"
        );
        assert!(
            code.contains("for < '__routerama_witness > State < Projected >")
                && code.contains("__RouteramaFixedStateContract , Rejection = __RouteramaRejection"),
            "the parts extractor is validated against the fixed alias: {code}"
        );
        assert!(!code.contains("__RouteramaState"), "{code}");
        assert!(
            code.contains("use < __RouteramaBody >"),
            "fixed state is not an inference-only opaque capture: {code}"
        );
        assert!(
            !code.contains("host_matches"),
            "state specialization adds no runtime policy: {code}"
        );
        assert!(!code.contains("BoxBody"), "state specialization alone adds no body erasure: {code}");
        assert_eq!(
            code.matches("__routerama_validate_fixed_state").count(),
            1,
            "the dead validation function is declared but never called by routing: {code}"
        );
    }

    #[test]
    fn configured_dynamic_route_uses_the_same_fixed_state_contract() {
        let generated = expand_fixed_router(
            syn::parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(dynamic)]
                    async fn plugin(&self, value: Extractor) -> Response {
                        response(value)
                    }
                }
            },
        )
        .expect("a fixed-state dynamic service is valid");
        let code = generated.to_string();

        assert!(code.contains("struct ApiRouter"), "{code}");
        assert!(code.contains("state : & AppState"), "{code}");
        assert!(
            code.contains("FromRequestParts < '__routerama_witness , __RouteramaFixedStateContract"),
            "{code}"
        );
        assert!(code.contains("FromRequestParts < '_ , AppState >"), "{code}");
        assert!(!code.contains("__RouteramaState"), "{code}");
    }

    #[test]
    fn fixed_state_validation_names_do_not_collide_with_service_items() {
        let generated = expand_fixed_router(
            syn::parse_quote!(__RouteramaFixedStateContract),
            quote! {
                impl Api {
                    fn __routerama_validate_fixed_state() {}

                    #[route(GET, "/")]
                    async fn home(&self, value: Extractor) -> Response {
                        response(value)
                    }
                }
            },
        )
        .expect("generated validation names are made unique");
        let code = generated.to_string();

        assert!(code.contains("fn __routerama_validate_fixed_state ()"), "{code}");
        assert!(code.contains("fn ___routerama_validate_fixed_state ()"), "{code}");
        assert!(
            code.contains("type ___RouteramaFixedStateContract = __RouteramaFixedStateContract"),
            "{code}"
        );
    }

    #[test]
    fn predicate_routes_generate_ordered_checks_and_success_header_mutation_only_for_the_annotated_handler() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/plain")]
                async fn plain(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }

                #[route(
                    POST,
                    "/items",
                    produces = "application/json",
                    host = "api.example",
                    consumes = "application/json",
                )]
                async fn create(&self, method: Method) -> String {
                    method.to_string()
                }

                #[route(dynamic, host = "plugins.example")]
                async fn plugin(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }
            }
        })
        .expect("static and configured-dynamic predicates are valid");
        let code = generated.to_string();
        assert_eq!(code.matches("host_matches").count(), 2, "{code}");
        assert_eq!(code.matches("content_type_matches_parsed").count(), 1, "{code}");
        assert_eq!(code.matches("accepts_parsed").count(), 1, "{code}");
        assert_eq!(code.matches("MediaType :: new").count(), 2, "{code}");
        assert_eq!(code.matches("CONTENT_TYPE").count(), 1, "{code}");
        assert_eq!(
            code.matches("HeaderValue :: from_static (\"application/json\")").count(),
            1,
            "{code}"
        );
        let host = code.find("host_matches").expect("host check");
        let consumes = code.find("content_type_matches_parsed").expect("content-type check");
        let produces = code.find("accepts_parsed").expect("accept check");
        let extraction = code.find("let method : Method").expect("parts extraction");
        assert!(host < consumes && consumes < produces && produces < extraction, "{code}");
        assert!(code.contains("route predicate rejection"), "{code}");
    }

    #[test]
    fn static_response_headers_are_prevalidated_and_emitted_in_source_order_before_negotiation() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(
                    GET,
                    "/items",
                    headers(
                        insert("X-Sequence", "first"),
                        append("x-sequence", "second"),
                        insert("content-type", "text/plain"),
                    ),
                    produces = "application/json",
                )]
                async fn items(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }
            }
        })
        .expect("static response headers are valid");
        let code = generated.to_string();
        assert_eq!(code.matches("HeaderName :: from_static").count(), 3, "{code}");
        assert_eq!(code.matches("HeaderValue :: from_static").count(), 4, "{code}");
        let first = code.find("\"first\"").expect("first inserted value");
        let second = code.find("\"second\"").expect("appended value");
        let static_content_type = code.find("\"text/plain\"").expect("static content type");
        let negotiated_content_type = code.find("\"application/json\"").expect("negotiated content type");
        assert!(
            first < second && second < static_content_type && static_content_type < negotiated_content_type,
            "{code}"
        );
        assert!(code.contains("HeaderName :: from_static (\"x-sequence\")"), "{code}");
        assert!(code.contains("headers_mut () . append"), "{code}");
        assert!(!code.contains("set_produced_content_type"), "{code}");
    }

    #[test]
    fn static_aliases_require_identical_predicates() {
        expand_router(quote! {
            impl Api {
                #[route(GET, "/items", host = "api.example", produces = "application/json")]
                #[route(HEAD, "/items", produces = "application/json", host = "api.example")]
                async fn items(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }

                #[test]
                fn route_aliases_require_identical_static_response_headers() {
                    expand_router(quote! {
                        impl Api {
                            #[route(GET, "/items", headers(insert("x-route", "one")))]
                            #[route(HEAD, "/items", headers(insert("x-route", "one")))]
                            async fn items(&self) -> StatusCode {
                                StatusCode::NO_CONTENT
                            }
                        }
                    })
                    .expect("matching static response-header plans are valid");

                    let error = expand_router(quote! {
                        impl Api {
                            #[route(GET, "/items", headers(insert("x-route", "one")))]
                            #[route(HEAD, "/items", headers(append("x-route", "one")))]
                            async fn items(&self) -> StatusCode {
                                StatusCode::NO_CONTENT
                            }
                        }
                    })
                    .expect_err("mismatched static response-header plans are rejected");
                    assert!(error.to_string().contains("identical static response-header operations"), "{error}");
                }
            }
        })
        .expect("predicate key order does not matter across aliases");

        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/items", host = "api.example")]
                #[route(HEAD, "/items", host = "other.example")]
                async fn items(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }
            }
        })
        .expect_err("one generated variant cannot distinguish aliases with different predicates");
        assert!(error.to_string().contains("must declare identical"), "{error}");
    }

    #[test]
    fn intentional_overlap_emits_priority_order_without_a_runtime_table() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/items/{id}", host = "api.example", priority = 20)]
                async fn host(&self, id: u32) -> StatusCode {
                    response(id)
                }

                #[route(GET, "/items/{id}", consumes = "application/json", priority = 10)]
                async fn json(&self, id: u32) -> StatusCode {
                    response(id)
                }
            }
        })
        .expect("explicit distinct priorities make compatible overlap intentional");
        let code = generated.to_string();
        assert_eq!(code.matches("OverlapPredicateState :: new").count(), 1, "{code}");
        assert_eq!(code.matches("MediaType :: new").count(), 1, "{code}");
        assert!(!code.contains("host_matches"), "{code}");
        assert!(!code.contains("content_type_matches"), "{code}");
        assert!(code.contains("__routerama_failure_stage"), "{code}");
        assert!(
            !code.contains("Vec <"),
            "candidate dispatch must not create a request-time table: {code}"
        );
        let host = code
            .find("__routerama_overlap_predicates . host")
            .expect("higher-priority host candidate");
        let consumes = code
            .find("__routerama_overlap_predicates . consumes")
            .expect("lower-priority consumes candidate");
        assert!(host < consumes, "{code}");
    }

    #[test]
    fn overlap_requires_explicit_distinct_priority_and_compatible_captures() {
        let missing = expand_router(quote! {
            impl Api {
                #[route(GET, "/{id}", host = "one.example", priority = 2)]
                async fn one(&self, id: u32) -> StatusCode { response(id) }
                #[route(GET, "/{id}", host = "two.example")]
                async fn two(&self, id: u32) -> StatusCode { response(id) }
            }
        })
        .expect_err("every overlap candidate needs explicit priority");
        assert!(missing.to_string().contains("explicit `priority"), "{missing}");

        let duplicate = expand_router(quote! {
            impl Api {
                #[route(GET, "/{id}", host = "one.example", priority = 2)]
                async fn one(&self, id: u32) -> StatusCode { response(id) }
                #[route(GET, "/{id}", host = "two.example", priority = 2)]
                async fn two(&self, id: u32) -> StatusCode { response(id) }
            }
        })
        .expect_err("overlap priorities must differ");
        assert!(duplicate.to_string().contains("cannot share priority"), "{duplicate}");

        let captures = expand_router(quote! {
            impl Api {
                #[route(GET, "/{id}", host = "one.example", priority = 2)]
                async fn one(&self, id: u32) -> StatusCode { response(id) }
                #[route(GET, "/{id}", host = "two.example", priority = 1)]
                async fn two(&self, id: String) -> StatusCode { response(id) }
            }
        })
        .expect_err("one typed conversion must serve every candidate");
        assert!(captures.to_string().contains("identical concrete capture types"), "{captures}");
    }

    #[test]
    fn fallback_and_exact_catcher_calls_enter_the_generated_body_sum() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, value: Extractor) -> StatusCode {
                    response(value)
                }

                #[catch(Rejection, from = Extractor)]
                async fn catch(&self, rejection: Rejection) -> CatchResponse {
                    catch_response(rejection)
                }

                #[fallback]
                async fn fallback(&self, failure: RouteFailure<'_>) -> FallbackResponse {
                    fallback_response(failure)
                }
            }
        })
        .expect("typed policy methods are valid");
        let code = generated.to_string();
        assert!(code.contains("self . catch (rejection) . await"), "{code}");
        assert!(code.contains("self . fallback"), "{code}");
        assert!(code.contains("routing fallback response"), "{code}");
        assert!(code.contains("extractor catcher response"), "{code}");
        assert!(!code.contains("BoxBody"), "policy responses remain concrete: {code}");
    }

    #[test]
    fn generated_module_rebases_relative_capture_type_paths() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/{local}/{outer}")]
                async fn get(
                    &self,
                    local: self::LocalId,
                    outer: super::OuterId,
                ) -> Response {
                    response(local, outer)
                }
            }
        })
        .expect("relative capture type paths are valid");
        let code = generated.to_string();
        assert!(code.contains("local : super :: LocalId"), "{code}");
        assert!(code.contains("outer : super :: super :: OuterId"), "{code}");
    }

    #[test]
    fn dynamic_router_generates_a_persistent_router_and_builder() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/health")]
                async fn health(&self) -> &'static str {
                    "healthy"
                }

                #[route(dynamic)]
                async fn plugin(&self, #[capture] name: String, method: Method) -> String {
                    plugin_response(method, name)
                }

                async fn route(&self) -> Response {
                    response()
                }
            }
        })
        .expect("valid mixed service");
        let code = generated.to_string();
        assert!(code.contains("struct ApiRouter"), "{code}");
        assert!(code.contains("struct ApiRouterBuilder"), "{code}");
        assert!(code.contains("fn add_plugin"), "{code}");
        assert!(code.contains("fn router_builder"), "{code}");
        assert!(!code.contains("route (dynamic)"), "dynamic marker is consumed: {code}");
        assert!(!code.contains("# [capture]"), "capture marker is consumed: {code}");
    }

    #[test]
    fn body_position_is_preserved_after_parts_extraction() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(POST, "/books/{id}")]
                async fn create(
                    &self,
                    #[body] bytes: Vec<u8>,
                    method: Method,
                    id: u32,
                    state: State<AppState>,
                ) -> Response {
                    response(bytes, method, id, state)
                }
            }
        })
        .expect("one body parameter may appear before parts and captures");
        let code = generated.to_string();
        let method_extraction = code.find("let method : Method").expect("method extraction is generated");
        let state_extraction = code.find("let state : State").expect("state extraction is generated");
        let body_extraction = code.find("let bytes : Vec").expect("body extraction is generated");
        assert!(method_extraction < body_extraction, "{code}");
        assert!(state_extraction < body_extraction, "{code}");
        assert!(code.contains("create (bytes , method , id , state)"), "{code}");
        assert!(!code.contains("# [body]"), "body marker is consumed: {code}");
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
    fn uncaught_parts_rejections_stay_out_of_the_route_signature() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn inspect(&self, guard: Guard) -> Response {
                    response(guard)
                }
            }
        })
        .expect("an uncaught request-parts rejection needs no route type parameter");
        let code = generated.to_string();

        assert!(!code.contains("PartsRejection"), "{code}");
        assert!(
            code.contains("for < '__routerama_request > Guard : :: routerama :: route :: __private :: FromRequestParts"),
            "{code}"
        );
        assert!(code.contains("SendBoxBody :: new"), "{code}");
    }

    #[test]
    fn fixed_state_routers_erase_no_parts_rejection_body() {
        let generated = expand_fixed_router(
            syn::parse_quote! { AppState },
            quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn inspect(&self, guard: Guard) -> Response {
                        response(guard)
                    }
                }
            },
        )
        .expect("a fixed state resolves the rejection concretely");
        let code = generated.to_string();

        assert!(!code.contains("PartsRejection"), "{code}");
        assert!(!code.contains("SendBoxBody"), "{code}");
    }

    #[test]
    fn handler_contract_errors_are_reported() {
        for (item, expected) in [
            (
                quote! {
                    impl Api {
                        #[route(GET, "/")]
                        fn home(&self) -> Response {
                            response()
                        }
                    }
                },
                "must be async",
            ),
            (
                quote! {
                    impl Api {
                        #[route(GET, "/books/{id}")]
                        async fn get(&self, method: Method) -> Response {
                            response(method)
                        }
                    }
                },
                "do not match",
            ),
            (
                quote! {
                    impl Api {
                        #[route(GET, "/")]
                        async fn home(&self, value: impl Extractor) -> Response {
                            response(value)
                        }
                    }
                },
                "cannot use `impl Trait`",
            ),
            (
                quote! {
                    impl Api {
                        #[route(GET, "/books/{name}")]
                        async fn get(&self, name: &'static str) -> Response {
                            response(name)
                        }
                    }
                },
                "must use `&str`",
            ),
            (
                quote! {
                    impl Api {
                        async fn route(&self) -> Response {
                            response()
                        }

                        #[route(GET, "/")]
                        async fn home(&self) -> Response {
                            response()
                        }
                    }
                },
                "already exists",
            ),
            (
                quote! {
                    impl Api {
                        #[route(POST, "/")]
                        async fn home(&self, #[body] body: &mut Vec<u8>) -> Response {
                            response(body)
                        }
                    }
                },
                "must own",
            ),
            (
                quote! {
                    impl Api {
                        #[route(GET, "/")]
                        async fn home(&self, headers: &'static HeaderMap) -> Response {
                            response(headers)
                        }
                    }
                },
                "lifetimes must be elided or use `'_`",
            ),
        ] {
            let error = expand_router(item).expect_err("invalid router contract");
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    /// Grammar diagnostics whose primary span covers more than one token.
    ///
    /// `syn` can only join multi-token spans when the compiler running the
    /// proc macro exposes `proc_macro::Span::join`, so the rendered caret
    /// differs between stable and nightly. These rules are therefore asserted
    /// by message here instead of through a `trybuild` snapshot.
    #[test]
    fn multi_token_span_grammar_errors_are_reported() {
        for (item, expected) in [
            (
                quote! {
                    impl Api {
                        #[route(GET, "/")]
                        async fn home<T: Default>(&self) -> Response {
                            response(T::default())
                        }
                    }
                },
                "cannot have generic parameters",
            ),
            (
                quote! {
                    impl<T> Api<T> {
                        #[route(GET, "/")]
                        async fn home(&self) -> Response {
                            response()
                        }
                    }
                },
                "generic impl blocks",
            ),
            (
                quote! {
                    impl Api {
                        #[route(dynamic)]
                        async fn home(&self, #[body] #[capture] value: TextBody<16>) -> Response {
                            response(value)
                        }
                    }
                },
                "cannot be both `#[body]` and `#[capture]`",
            ),
            (
                quote! {
                    impl Api {
                        #[route(GET, "/")]
                        async fn home(&self) -> impl IntoResponse {
                            response()
                        }
                    }
                },
                "response types cannot use `impl Trait`",
            ),
        ] {
            let error = expand_router(item).expect_err("invalid handler grammar");
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn heterogeneous_handler_responses_are_accepted() {
        expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> String {
                    String::new()
                }

                #[route(GET, "/other")]
                async fn other(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }
            }
        })
        .expect("each response type is converted independently");
    }

    #[test]
    fn duplicate_body_markers_are_rejected_during_expansion() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(
                    &self,
                    #[body] first: Vec<u8>,
                    #[body] second: Vec<u8>,
                ) -> Response {
                    response(first, second)
                }
            }
        })
        .expect_err("body ownership must be unique");
        assert!(error.to_string().contains("at most one `#[body]`"), "{error}");
    }

    #[test]
    fn dynamic_handler_contract_errors_are_reported() {
        for (item, expected) in [
            (
                quote! {
                    impl Api {
                        #[route(dynamic)]
                        #[route(GET, "/books/{name}")]
                        async fn get(&self, #[capture] name: String) -> Response {
                            response(name)
                        }
                    }
                },
                "cannot be combined",
            ),
            (
                quote! {
                    impl Api {
                        #[route(dynamic)]
                        async fn get(&self, #[capture] name: &str) -> Response {
                            response(name)
                        }
                    }
                },
                "must be owned",
            ),
            (
                quote! {
                    impl Api {
                        fn router_builder() {}

                        #[route(dynamic)]
                        async fn get(&self, #[capture] name: String) -> Response {
                            response(name)
                        }
                    }
                },
                "already exists",
            ),
            (
                quote! {
                    impl Api {
                        #[route(GET, "/")]
                        async fn get(&self, #[capture] name: String) -> Response {
                            response(name)
                        }
                    }
                },
                "absent from this static route",
            ),
        ] {
            let error = expand_router(item).expect_err("invalid dynamic router contract");
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn streaming_transform_substitutes_the_transport_body_and_never_buffers() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self, #[body] data: BytesBody<64>) -> StatusCode {
                    response(data)
                }

                #[transform(stream, create)]
                async fn wrap<B>(&self, parts: &RequestParts, body: B) -> BodyTransform<B::Wrapped, StatusCode>
                where
                    B: http_body::Body<Data = Bytes> + Wrapper + Unpin,
                    B::Wrapped: http_body::Body<Data = Bytes>,
                {
                    wrap(parts, body)
                }
            }
        })
        .expect("a streaming transform is a valid terminal body owner");
        let code = generated.to_string();

        assert!(
            code.contains("let __routerama_transformed_body : __RouteramaBody :: Wrapped"),
            "the leading generic in an associated-type path is substituted: {code}"
        );
        assert!(
            code.contains("FromRequestBody < __RouteramaState , __RouteramaBody :: Wrapped >"),
            "handler `#[body]` extraction binds to the replacement wrapper: {code}"
        );
        assert!(
            code.contains("__RouteramaBody : http_body :: Body < Data = Bytes > + Wrapper + Unpin"),
            "the interceptor's transport-body bounds are propagated: {code}"
        );
        assert!(
            code.contains("__RouteramaBody :: Wrapped : http_body :: Body < Data = Bytes >"),
            "associated-type bounds are substituted and propagated: {code}"
        );
        assert!(
            !code.contains("buffer_request_body"),
            "a streaming transform imposes no framework buffering: {code}"
        );
        assert!(
            !code.contains("request-body buffering rejection"),
            "a streaming transform has no buffering rejection source: {code}"
        );
    }

    #[test]
    fn buffered_transform_keeps_its_bounded_collection() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self, #[body] data: BytesBody<64>) -> StatusCode {
                    response(data)
                }

                #[transform(limit = 32, create)]
                async fn shrink(&self, parts: &RequestParts, body: Bytes) -> BodyTransform<Body, StatusCode> {
                    shrink(parts, body)
                }
            }
        })
        .expect("a buffered transform is a valid terminal body owner");
        let code = generated.to_string();

        assert!(
            code.contains("buffer_request_body :: < __RouteramaBody , { 32 } >"),
            "bounded buffering keeps its explicit limit: {code}"
        );
        assert!(code.contains("request-body buffering rejection"), "{code}");
        assert!(
            code.contains("FromRequestBody < __RouteramaState , Body >"),
            "handler `#[body]` extraction binds to the concrete replacement: {code}"
        );
    }

    #[test]
    fn per_handler_before_splits_the_request_head_so_captures_stay_borrowed() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/books/{slug}")]
                async fn book(&self, slug: &str) -> String {
                    response(slug)
                }

                #[before(book)]
                async fn guard(&self, ctx: &mut SelectedContext<'_>) -> Before<StatusCode> {
                    guard(ctx)
                }
            }
        })
        .expect("a per-handler guard composes with a borrowed capture");
        let code = generated.to_string();

        assert!(
            code.contains("SelectedContext :: new (& __routerama_parts . method , & __routerama_parts . uri , __routerama_parts . version , & mut __routerama_parts . headers , & mut __routerama_parts . extensions ,)"),
            "the guard borrows the request head by field: {code}"
        );
        assert!(
            !code.contains("BeforeContext"),
            "a per-handler guard never takes the whole mutable head: {code}"
        );
    }

    #[test]
    fn generated_wide_after_wraps_every_generated_response() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }

                #[after]
                async fn seal(&self, ctx: &mut AfterContext<'_>) {
                    seal(ctx)
                }
            }
        })
        .expect("a generated-wide response interceptor is valid");
        let code = generated.to_string();

        assert_eq!(
            code.matches("'__routerama_dispatch").count(),
            2,
            "the dispatch is labeled once and its routing failure breaks to it, so that response is observed too: {code}"
        );
        assert_eq!(
            code.matches("AfterContext :: new").count(),
            1,
            "one entry epilogue observes every generated response: {code}"
        );
        assert!(
            code.contains("Response :: from_parts (__routerama_response_parts , __routerama_response_body)"),
            "the original response body is moved back unchanged: {code}"
        );
    }

    #[test]
    fn per_handler_after_stays_inside_its_dispatch_arm() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/one")]
                async fn one(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }

                #[route(GET, "/two")]
                async fn two(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }

                #[after(one)]
                async fn seal(&self, ctx: &mut AfterContext<'_>) {
                    seal(ctx)
                }
            }
        })
        .expect("a per-handler response interceptor is valid");
        let code = generated.to_string();

        assert!(
            !code.contains("'__routerama_dispatch"),
            "a per-handler after does not observe other generated responses: {code}"
        );
        assert_eq!(code.matches("AfterContext :: new").count(), 1, "{code}");
        assert_eq!(code.matches("self . seal").count(), 1, "{code}");
    }

    #[test]
    fn a_service_without_interceptors_keeps_its_previous_lowering() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }
            }
        })
        .expect("a plain service is valid");
        let code = generated.to_string();

        assert!(!code.contains("'__routerama_dispatch"), "{code}");
        assert!(!code.contains("AfterContext"), "{code}");
        assert!(!code.contains("BeforeContext"), "{code}");
        assert!(!code.contains("SelectedContext"), "{code}");
        assert!(
            code.contains("let (__routerama_parts , __routerama_body)"),
            "the request head is not bound mutably without a `#[before]`: {code}"
        );
    }

    #[test]
    fn media_type_tables_are_ordered_by_type_and_subtype() {
        let mut values = ["x.foo/bar", "x-foo/bar", "x+foo/bar", "x/bar", "x/baz"];
        values.sort_unstable_by(|left, right| media_type_order(left).cmp(&media_type_order(right)));

        assert_eq!(values, ["x/bar", "x/baz", "x+foo/bar", "x-foo/bar", "x.foo/bar"]);

        let mut string_ordered = values;
        string_ordered.sort_unstable();
        assert_ne!(
            values, string_ordered,
            "the joined-string order must differ here, otherwise this test proves nothing"
        );
    }
}
