// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Route grouping and code generation for `#[router]`.

use alloc::format;
use alloc::string::{String, ToString as _};
use alloc::vec::Vec;

use http_path_template::{Grammar, PathTemplate};
use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::spanned::Spanned as _;
use syn::{Error, GenericArgument, LitStr, PathArguments, Type};

use super::model::{Handler, HandlerKind, Interceptor, InterceptorKind, ParamKind, Router};
use super::parse::type_base;
use crate::route_field_name;
use crate::trie::capture_field_names;

/// One declaration that can serve a route group.
pub(crate) struct Candidate {
    pub(crate) handler: usize,
    pub(crate) decl: usize,
    pub(crate) priority: i32,
}

/// A generated route variant: one method/path shape and its candidates.
pub(crate) struct Group {
    pub(crate) variant: Ident,
    pub(crate) method: String,
    pub(crate) path: LitStr,
    /// The path template with capture names erased, so intentionally
    /// overlapping declarations share one generated route variant.
    pub(crate) shape: String,
    pub(crate) captures: Vec<(Ident, Type)>,
    pub(crate) borrows: bool,
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) dynamic: Option<usize>,
}

/// Erases capture names from a path template, keeping its matching shape.
fn path_shape(path: &str) -> String {
    let mut shape = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        shape.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            shape.push_str(&rest[start..]);
            return shape;
        };
        let inside = &rest[start + 1..start + end];
        shape.push('{');
        if let Some(equals) = inside.find('=') {
            shape.push_str(&inside[equals..]);
        }
        shape.push('}');
        rest = &rest[start + end + 1..];
    }
    shape.push_str(rest);
    shape
}

/// One route candidate's declared predicates, paired with its span.
type PredicateKey = ((Option<String>, Option<String>, Option<String>), Span);

/// One response-producing site in the generated body sum.
struct Site {
    variant: Ident,
    label: String,
    /// The response category, which keeps distinct diagnostics apart.
    kind: &'static str,
    /// The concrete body type this site contributes to the response sum.
    ty: TokenStream2,
}

/// Accumulates the generated response body sum's variants.
struct Sites {
    sites: Vec<Site>,
}

impl Sites {
    const fn new() -> Self {
        Self { sites: Vec::new() }
    }

    fn add(&mut self, kind: &'static str, label: impl Into<String>, ty: TokenStream2) -> Ident {
        // Sites of one category that contribute the same concrete body share
        // one variant, so a service with many identically typed handlers stays
        // small while each category keeps its own diagnostic.
        let key = ty.to_string();
        if let Some(site) = self.sites.iter().find(|site| site.kind == kind && site.ty.to_string() == key) {
            return site.variant.clone();
        }
        let index = self.sites.len();
        let variant = format_ident!("V{}", index);
        self.sites.push(Site {
            variant: variant.clone(),
            label: label.into(),
            kind,
            ty,
        });
        variant
    }
}

/// Builds the route groups shared by the generated resolver and dispatch.
pub(crate) fn build_groups(router: &Router) -> syn::Result<Vec<Group>> {
    let mut groups: Vec<Group> = Vec::new();
    let mut used_names: Vec<String> = Vec::new();

    // Dynamic variants are named first so their `add_<handler>` builder methods
    // match the handler name exactly.
    for (index, handler) in router.handlers.iter().enumerate() {
        if handler.kind != HandlerKind::Dynamic {
            continue;
        }
        let variant = camel_case(&handler.name)?;
        if used_names.contains(&variant.to_string()) {
            return Err(Error::new(
                handler.name.span(),
                format!("handler names generate the duplicate route variant `{variant}`"),
            ));
        }
        used_names.push(variant.to_string());
        let captures = handler
            .params
            .iter()
            .filter(|param| param.kind == ParamKind::DynamicCapture)
            .map(|param| (param.name.clone(), param.ty.clone()))
            .collect();
        groups.push(Group {
            variant,
            method: String::new(),
            path: LitStr::new("", handler.name.span()),
            shape: String::new(),
            captures,
            borrows: false,
            candidates: Vec::new(),
            dynamic: Some(index),
        });
    }

    for (handler_index, handler) in router.handlers.iter().enumerate() {
        if handler.kind != HandlerKind::Static {
            continue;
        }
        for (decl_index, decl) in handler.routes.iter().enumerate() {
            let key_method = decl.method.clone();
            let key_path = decl.path.value();
            let key_shape = path_shape(&key_path);
            for previous in &handler.routes[..decl_index] {
                if previous.method == key_method && previous.path.value() == key_path {
                    return Err(Error::new(
                        previous.attr_span,
                        "duplicate method/path aliases on one handler are not candidates; remove the duplicate declaration",
                    )
                    .tap_combine(Error::new(
                        decl.attr_span,
                        "duplicate method/path aliases on one handler are not candidates; remove the duplicate declaration",
                    )));
                }
            }

            let existing = groups
                .iter()
                .position(|group| group.dynamic.is_none() && group.method == key_method && group.shape == key_shape);
            let template = PathTemplate::parse(&key_path, Grammar::default().with_segment_affixes())
                .map_err(|error| Error::new(decl.path.span(), format!("invalid path template: {error}")))?;
            let capture_order: Vec<String> = capture_field_names(template.segments())
                .into_iter()
                .map(|name| route_field_name(name.join(".")))
                .collect();
            let captures = handler_captures(handler, &capture_order)?;

            if let Some(index) = existing {
                validate_shared_captures(&groups[index], &captures, decl.attr_span)?;
                groups[index].candidates.push(Candidate {
                    handler: handler_index,
                    decl: decl_index,
                    priority: decl.priority.map_or(0, |(value, _)| value),
                });
            } else {
                let mut variant = camel_case(&handler.name)?;
                let mut suffix = 1_u32;
                while used_names.contains(&variant.to_string()) {
                    variant = format_ident!("{}Route{}", camel_case(&handler.name)?, suffix, span = handler.name.span());
                    suffix += 1;
                }
                used_names.push(variant.to_string());
                let borrows = captures.iter().any(|(_, ty)| capture_borrows(ty));
                groups.push(Group {
                    variant,
                    method: key_method,
                    path: decl.path.clone(),
                    shape: key_shape,
                    captures,
                    borrows,
                    candidates: alloc::vec![Candidate {
                        handler: handler_index,
                        decl: decl_index,
                        priority: decl.priority.map_or(0, |(value, _)| value),
                    }],
                    dynamic: None,
                });
            }
        }
    }

    for group in &mut groups {
        validate_candidates(router, group)?;
        group.candidates.sort_by_key(|candidate| ::core::cmp::Reverse(candidate.priority));
    }
    Ok(groups)
}

trait TapCombine {
    fn tap_combine(self, other: Error) -> Error;
}

impl TapCombine for Error {
    fn tap_combine(mut self, other: Error) -> Error {
        self.combine(other);
        self
    }
}

fn validate_shared_captures(group: &Group, captures: &[(Ident, Type)], span: Span) -> syn::Result<()> {
    if group.captures.len() != captures.len() || group.captures.iter().zip(captures).any(|((left, _), (right, _))| left != right) {
        return Err(Error::new(
            span,
            "overlapping routes must use identical capture names and capture positions",
        ));
    }
    if group
        .captures
        .iter()
        .zip(captures)
        .any(|((_, left), (_, right))| !same_type(left, right))
    {
        return Err(Error::new(span, "overlapping routes must use identical concrete capture types"));
    }
    Ok(())
}

fn same_type(left: &Type, right: &Type) -> bool {
    use quote::ToTokens as _;

    left.to_token_stream().to_string() == right.to_token_stream().to_string()
}

fn validate_candidates(router: &Router, group: &Group) -> syn::Result<()> {
    if group.candidates.len() < 2 {
        return Ok(());
    }
    let mut seen: Vec<(i32, Span)> = Vec::new();
    let mut predicates: Vec<PredicateKey> = Vec::new();
    let mut lowest: Option<i32> = None;
    let mut predicate_free: Option<(i32, Span)> = None;

    for candidate in &group.candidates {
        let handler = &router.handlers[candidate.handler];
        let decl = &handler.routes[candidate.decl];
        let Some((priority, priority_span)) = decl.priority else {
            return Err(Error::new(
                decl.attr_span,
                "overlapping routes require an explicit `priority = <integer>` on every declaration",
            ));
        };
        if let Some((_, _first)) = seen.iter().find(|(value, _)| *value == priority) {
            return Err(Error::new(
                priority_span,
                format!("overlapping routes cannot share priority {priority}"),
            ));
        }
        seen.push((priority, priority_span));
        let key = decl.predicates();
        if let Some((_, _first)) = predicates.iter().find(|(existing, _)| existing == &key) {
            return Err(Error::new(
                decl.attr_span,
                "overlapping candidates with identical predicates make the lower priority unreachable",
            ));
        }
        predicates.push((key, decl.attr_span));
        if !decl.has_predicates() {
            predicate_free = Some((priority, decl.attr_span));
        }
        lowest = Some(lowest.map_or(priority, |value: i32| value.min(priority)));
    }

    if let Some((priority, span)) = predicate_free
        && lowest.is_some_and(|lowest| priority != lowest)
    {
        return Err(Error::new(
            span,
            "a predicate-free overlapping candidate must have the lowest priority because it matches every request",
        ));
    }
    Ok(())
}

fn handler_captures(handler: &Handler, order: &[String]) -> syn::Result<Vec<(Ident, Type)>> {
    let mut captures = Vec::with_capacity(order.len());
    for name in order {
        let param = handler
            .params
            .iter()
            .find(|param| param.kind == ParamKind::Capture && param.name == name.as_str());
        let Some(param) = param else {
            return Err(Error::new(
                handler.name.span(),
                format!("handler `{}` is missing a parameter for the path capture `{name}`", handler.name),
            ));
        };
        captures.push((param.name.clone(), capture_type(&param.ty)?));
    }
    Ok(captures)
}

/// Rewrites a handler's borrowed capture type into the route enum's `'p` form.
fn capture_type(handler_type: &Type) -> syn::Result<Type> {
    let mut capture_type = handler_type.clone();
    if let Type::Reference(reference) = &mut capture_type
        && matches!(reference.elem.as_ref(), Type::Path(path) if path.path.is_ident("str"))
    {
        if reference.mutability.is_some() || reference.lifetime.as_ref().is_some_and(|lifetime| lifetime.ident != "_") {
            return Err(Error::new(handler_type.span(), "borrowed string captures must use `&str`"));
        }
        reference.lifetime = Some(syn::Lifetime::new("'p", Span::call_site()));
        return Ok(capture_type);
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
        return Ok(capture_type);
    }
    Ok(capture_type)
}

fn capture_borrows(ty: &Type) -> bool {
    match ty {
        Type::Reference(_) => true,
        Type::Path(path) => path.path.segments.last().is_some_and(|segment| {
            segment.ident == "Cow"
                && matches!(&segment.arguments, PathArguments::AngleBracketed(arguments)
                    if arguments.args.iter().any(|argument| matches!(argument, GenericArgument::Lifetime(_))))
        }),
        _ => false,
    }
}

/// The rendered spelling of a type, used by the generated body diagnostics.
fn type_label(ty: &Type) -> String {
    use quote::ToTokens as _;

    ty.to_token_stream().to_string()
}

fn camel_case(name: &Ident) -> syn::Result<Ident> {
    let spelling = name.to_string();
    if spelling.starts_with("r#") {
        return Err(Error::new(name.span(), "router handler names cannot be raw identifiers"));
    }
    let mut camel = String::new();
    for part in spelling.split('_').filter(|part| !part.is_empty()) {
        let mut characters = part.chars();
        if let Some(first) = characters.next() {
            camel.extend(first.to_uppercase());
            camel.extend(characters);
        }
    }
    if camel.is_empty() {
        return Err(Error::new(name.span(), "router handler names must contain a letter"));
    }
    Ok(Ident::new(&camel, name.span()))
}

/// The generated body sum, error sum, and their `http_body` implementation.
#[expect(
    clippy::too_many_lines,
    reason = "the response sum, its error sum, and their http_body implementation are one cohesive definition"
)]
fn emit_body_sum(body_name: &Ident, error_name: &Ident, sites: &Sites, rt: &TokenStream2, root: &TokenStream2) -> TokenStream2 {
    let projection = format_ident!("{}Projection", body_name);
    let parameters: Vec<Ident> = (0..sites.sites.len()).map(|index| format_ident!("__RtrT{}", index)).collect();
    let errors: Vec<Ident> = (0..sites.sites.len()).map(|index| format_ident!("__RtrE{}", index)).collect();
    let variants = sites.sites.iter().zip(&parameters).map(|(site, parameter)| {
        let variant = &site.variant;
        quote! { #variant { #[pin] body: #parameter } }
    });
    let error_variants = sites.sites.iter().zip(&errors).map(|(site, error)| {
        let variant = &site.variant;
        quote! { #variant(#error) }
    });
    // The generated diagnostics name the failing response source rather than
    // forwarding to the retained error, so the sum implements `Debug`,
    // `Display`, and `Error` for every body set a service can produce.
    let display_arms = sites.sites.iter().map(|site| {
        let variant = &site.variant;
        let label = format!("response body from {} failed", site.label);
        quote! { Self::#variant(_) => ::core::fmt::Formatter::write_str(__rtr_f, #label) }
    });
    let debug_arms = sites.sites.iter().map(|site| {
        let variant = &site.variant;
        let label = site.variant.to_string();
        quote! { Self::#variant(_) => ::core::fmt::Formatter::write_str(__rtr_f, #label) }
    });
    let fixed_display = quote! {
        Self::Fixed(_) => ::core::fmt::Formatter::write_str(__rtr_f, "the generated response body failed"),
    };
    let poll_arms = sites.sites.iter().map(|site| {
        let variant = &site.variant;
        quote! {
            #projection::#variant { body } => match #rt::http_body::Body::poll_frame(body, __rtr_cx) {
                ::core::task::Poll::Ready(::core::option::Option::Some(::core::result::Result::Ok(__rtr_frame))) => {
                    ::core::task::Poll::Ready(::core::option::Option::Some(::core::result::Result::Ok(__rtr_frame)))
                }
                ::core::task::Poll::Ready(::core::option::Option::Some(::core::result::Result::Err(__rtr_error))) => {
                    ::core::task::Poll::Ready(::core::option::Option::Some(::core::result::Result::Err(#error_name::#variant(__rtr_error))))
                }
                ::core::task::Poll::Ready(::core::option::Option::None) => ::core::task::Poll::Ready(::core::option::Option::None),
                ::core::task::Poll::Pending => ::core::task::Poll::Pending,
            }
        }
    });
    let end_arms = sites.sites.iter().map(|site| {
        let variant = &site.variant;
        quote! { Self::#variant { body } => #rt::http_body::Body::is_end_stream(body) }
    });
    let hint_arms = sites.sites.iter().map(|site| {
        let variant = &site.variant;
        quote! { Self::#variant { body } => #rt::http_body::Body::size_hint(body) }
    });

    quote! {
        #rt::pin_project! {
            #[project = #projection]
            #[allow(dead_code, reason = "a generated response body variant may be unreachable in a partial build")]
            #[doc(hidden)]
            pub enum #body_name<#(#parameters),*> {
                #(#variants,)*
                Fixed {
                    #[pin]
                    body: #root::response::Body,
                },
            }
        }

        #[allow(dead_code, reason = "a generated response error variant may be unreachable in a partial build")]
        #[doc(hidden)]
        pub enum #error_name<#(#errors),*> {
            #(#error_variants,)*
            Fixed(::core::convert::Infallible),
        }

        #[automatically_derived]
        impl<#(#errors),*> ::core::fmt::Debug for #error_name<#(#errors),*> {
            fn fmt(&self, __rtr_f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self {
                    #(#debug_arms,)*
                    Self::Fixed(_) => ::core::fmt::Formatter::write_str(__rtr_f, "Fixed"),
                }
            }
        }

        #[automatically_derived]
        impl<#(#errors),*> ::core::fmt::Display for #error_name<#(#errors),*> {
            fn fmt(&self, __rtr_f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self {
                    #(#display_arms,)*
                    #fixed_display
                }
            }
        }

        #[automatically_derived]
        impl<#(#errors),*> ::core::error::Error for #error_name<#(#errors),*> {}

        #[automatically_derived]
        impl<#(#parameters),*> #rt::http_body::Body for #body_name<#(#parameters),*>
        where
            #(#parameters: #rt::http_body::Body<Data = #rt::bytes::Bytes>,)*
        {
            type Data = #rt::bytes::Bytes;
            type Error = #error_name<#(<#parameters as #rt::http_body::Body>::Error),*>;

            fn poll_frame(
                self: ::core::pin::Pin<&mut Self>,
                __rtr_cx: &mut ::core::task::Context<'_>,
            ) -> ::core::task::Poll<::core::option::Option<::core::result::Result<#rt::http_body::Frame<Self::Data>, Self::Error>>>
            {
                match self.project() {
                    #(#poll_arms,)*
                    #projection::Fixed { body } => match #rt::http_body::Body::poll_frame(body, __rtr_cx) {
                        ::core::task::Poll::Ready(::core::option::Option::Some(::core::result::Result::Ok(__rtr_frame))) => {
                            ::core::task::Poll::Ready(::core::option::Option::Some(::core::result::Result::Ok(__rtr_frame)))
                        }
                        ::core::task::Poll::Ready(::core::option::Option::Some(::core::result::Result::Err(__rtr_error))) => {
                            ::core::task::Poll::Ready(::core::option::Option::Some(::core::result::Result::Err(#error_name::Fixed(__rtr_error))))
                        }
                        ::core::task::Poll::Ready(::core::option::Option::None) => ::core::task::Poll::Ready(::core::option::Option::None),
                        ::core::task::Poll::Pending => ::core::task::Poll::Pending,
                    },
                }
            }

            fn is_end_stream(&self) -> bool {
                match self {
                    #(#end_arms,)*
                    Self::Fixed { body } => #rt::http_body::Body::is_end_stream(body),
                }
            }

            fn size_hint(&self) -> #rt::http_body::SizeHint {
                match self {
                    #(#hint_arms,)*
                    Self::Fixed { body } => #rt::http_body::Body::size_hint(body),
                }
            }
        }
    }
}

/// Whether a rejection is converted by a catcher, and how its response body is
/// named when it is not.
#[derive(Clone, Copy)]
enum Slot {
    /// The parameter is a path capture and never rejects.
    None,
    /// An extractor catcher owns the rejection.
    Caught(usize),
    /// The rejection's response body is a nameable associated-type projection.
    Projected,
    /// The rejection's response body is erased once through `BoxBody`.
    ///
    /// A request-parts extractor needs a higher-ranked bound, which blocks
    /// associated-type normalization, so its rejection body cannot be named in
    /// the entry's signature without leaking a private type or pulling the
    /// request borrow into the generated response. The single erasure keeps
    /// private rejection types out of the public signature and never touches a
    /// success path.
    Erased,
}

/// Assigns every extractor rejection to a catcher, a projection, or erasure.
struct RejectionPlan {
    slots: Vec<Vec<Slot>>,
}

fn plan_rejections(router: &Router) -> RejectionPlan {
    let mut slots = Vec::with_capacity(router.handlers.len());
    for handler in &router.handlers {
        let mut handler_slots = Vec::with_capacity(handler.params.len());
        for param in &handler.params {
            let slot = match param.kind {
                ParamKind::Capture | ParamKind::DynamicCapture => Slot::None,
                ParamKind::Parts | ParamKind::Body => match catcher_for(router, param) {
                    Some(index) => Slot::Caught(index),
                    None if param.kind == ParamKind::Body => Slot::Projected,
                    None => Slot::Erased,
                },
            };
            handler_slots.push(slot);
        }
        slots.push(handler_slots);
    }
    RejectionPlan { slots }
}

/// The transport request body a handler's `#[body]` extractor consumes.
fn transport_for(router: &Router, handler: &Handler) -> TokenStream2 {
    router
        .interceptors
        .iter()
        .find(|interceptor| {
            matches!(
                interceptor.kind,
                InterceptorKind::TransformBuffered { .. } | InterceptorKind::TransformStream { .. }
            ) && interceptor.handlers.contains(&handler.name)
        })
        .and_then(|interceptor| interceptor.replacement.clone())
        .map_or_else(|| quote! { __RtrB }, |replacement| quote! { #replacement })
}

/// The nameable spelling of a body extractor's rejection type.
fn projected_rejection(param: &super::model::Param, transport: &TokenStream2, state: &TokenStream2, rt: &TokenStream2) -> TokenStream2 {
    let ty = &param.ty;
    quote! { <#ty as #rt::FromRequestBody<#state, #transport>>::Rejection }
}

/// The extractor obligations a generated entry point carries.
///
/// A fixed-state router proves its request-parts extractors when the impl is
/// defined, so it carries no parts obligation and its diagnostics point at the
/// annotation. Only a state-generic router needs the higher-ranked bound.
fn emit_extractor_bounds(
    router: &Router,
    plan: &RejectionPlan,
    state: &TokenStream2,
    rt: &TokenStream2,
    root: &TokenStream2,
) -> Vec<TokenStream2> {
    let fixed_state = router.args.state.is_some();
    let request = syn::Lifetime::new("'__rtr_request", Span::call_site());
    let mut bounds: Vec<TokenStream2> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for (handler_index, handler) in router.handlers.iter().enumerate() {
        let transport = transport_for(router, handler);
        for (param_index, param) in handler.params.iter().enumerate() {
            let ty = &param.ty;
            let slot = plan.slots[handler_index][param_index];
            if matches!(slot, Slot::None) {
                continue;
            }
            let bound = match param.kind {
                ParamKind::Parts => {
                    let rejection = match slot {
                        Slot::Caught(index) => {
                            let parameter = &router.catchers[index].parameter;
                            quote! { , Rejection = #parameter }
                        }
                        // A fixed-state router proves an uncaught request-parts
                        // extractor when the impl is defined, and its erased
                        // rejection needs no named body, so it carries no
                        // obligation into the generated entry.
                        Slot::Erased if fixed_state => continue,
                        Slot::Erased => quote! {
                            , Rejection: #root::response::IntoResponse<
                                Body: #rt::http_body::Body<Error: ::core::error::Error + ::core::marker::Send + ::core::marker::Sync + 'static>
                                    + ::core::marker::Send + 'static,
                            >
                        },
                        Slot::Projected | Slot::None => quote! {},
                    };
                    let bound_ty = super::parse::bind_request_lifetime(ty, &request);
                    quote! { for<#request> #bound_ty: #rt::FromRequestParts<#request, #state #rejection> }
                }
                ParamKind::Body => {
                    let rejection = match slot {
                        Slot::Caught(index) => {
                            let parameter = &router.catchers[index].parameter;
                            quote! { , Rejection = #parameter }
                        }
                        Slot::Erased | Slot::Projected | Slot::None => quote! {},
                    };
                    quote! { #ty: #rt::FromRequestBody<#state, #transport #rejection> }
                }
                ParamKind::Capture | ParamKind::DynamicCapture => continue,
            };
            let text = bound.to_string();
            if !seen.contains(&text) {
                seen.push(text);
                bounds.push(bound);
            }
        }
    }
    bounds
}

/// The complete generated output for one `#[router]` impl.
pub(crate) struct Generated {
    pub(crate) items: TokenStream2,
    pub(crate) impl_items: Vec<syn::ImplItem>,
}

#[expect(
    clippy::too_many_lines,
    reason = "the generated router assembles the resolver, response sum, entries, and witnesses in one place"
)]
pub(crate) fn emit(router: &Router, groups: &[Group], rt: &TokenStream2, root: &TokenStream2) -> syn::Result<Generated> {
    let service = &router.service_name;
    let service_ty = &router.service_ty;
    let route_enum = format_ident!("__{}Route", service, span = service.span());
    let body_name = format_ident!("__{}ResponseBodySum", service, span = service.span());
    let error_name = format_ident!("__{}ResponseBodyErrorSum", service, span = service.span());
    let router_name = format_ident!("{}Router", service, span = service.span());
    let builder_name = format_ident!("{}RouterBuilder", service, span = service.span());

    let has_dynamic = groups.iter().any(|group| group.dynamic.is_some());
    let has_lifetime = groups.iter().any(|group| group.borrows);
    let generics = has_lifetime.then(|| quote! { <'p> });

    let variants = groups.iter().map(|group| {
        let variant = &group.variant;
        let attrs = group.dynamic.is_none().then(|| {
            let method = syn::LitStr::new(&group.method, group.path.span());
            let path = &group.path;
            quote! { #[route(#method, #path)] }
        });
        if group.captures.is_empty() {
            quote! { #attrs #variant }
        } else {
            let fields = group.captures.iter().map(|(name, ty)| quote! { #name: #ty });
            quote! { #attrs #variant { #(#fields),* } }
        }
    });
    let route_item: syn::ItemEnum = syn::parse2(quote! {
        enum #route_enum #generics {
            #(#variants),*
        }
    })?;
    let resolver_items = crate::macro_impl::resolver::expand_with_runtime(route_item, None, rt, true)?;
    let resolver_ty = format_ident!("{}Resolver", route_enum, span = service.span());
    let resolver_builder_ty = format_ident!("{}Builder", resolver_ty, span = service.span());

    let mut sites = Sites::new();
    let mut impl_items: Vec<syn::ImplItem> = Vec::new();
    let plan = plan_rejections(router);
    let body_of = |ty: &Type| quote! { <#ty as #root::response::IntoResponse>::Body };
    let state_ty = router
        .args
        .state
        .as_ref()
        .map_or_else(|| quote! { __RtrS }, |state| quote! { #state });

    // Only sites the generated dispatch actually constructs become variants,
    // so every response body type is pinned by at least one construction.
    let handler_sites: Vec<Ident> = router
        .handlers
        .iter()
        .map(|handler| {
            sites.add(
                "handler",
                format!("handler response `{}`", type_label(&handler.response)),
                body_of(&handler.response),
            )
        })
        .collect();
    let mut catcher_sites: Vec<Option<Ident>> = alloc::vec![None; router.catchers.len()];
    let mut rejection_sites: Vec<Vec<Option<Ident>>> = Vec::with_capacity(router.handlers.len());
    for (handler_index, handler) in router.handlers.iter().enumerate() {
        let transport = transport_for(router, handler);
        let mut handler_rejections = Vec::with_capacity(handler.params.len());
        for (param_index, param) in handler.params.iter().enumerate() {
            match plan.slots[handler_index][param_index] {
                Slot::None => handler_rejections.push(None),
                Slot::Caught(index) => {
                    if catcher_sites[index].is_none() {
                        let catcher = &router.catchers[index];
                        catcher_sites[index] = Some(sites.add(
                            "catcher",
                            format!("extractor catcher response `{}`", type_label(&catcher.response)),
                            body_of(&catcher.response),
                        ));
                    }
                    handler_rejections.push(None);
                }
                Slot::Projected => {
                    let rejection = projected_rejection(param, &transport, &state_ty, rt);
                    handler_rejections.push(Some(sites.add(
                        "rejection",
                        format!("extractor rejection response `{}`", type_label(&param.ty)),
                        quote! { <#rejection as #root::response::IntoResponse>::Body },
                    )));
                }
                Slot::Erased => {
                    handler_rejections.push(Some(sites.add(
                        "rejection",
                        "erased extractor rejection response",
                        quote! { #root::response::SendBoxBody },
                    )));
                }
            }
        }
        rejection_sites.push(handler_rejections);
    }
    let interceptor_sites: Vec<Option<Ident>> = router
        .interceptors
        .iter()
        .map(|interceptor| match interceptor.kind {
            InterceptorKind::After => None,
            InterceptorKind::Before | InterceptorKind::TransformBuffered { .. } | InterceptorKind::TransformStream { .. } => {
                let label = interceptor.short_circuit.as_ref().map_or_else(
                    || "interceptor response".to_string(),
                    |ty| format!("interceptor response `{}`", type_label(ty)),
                );
                let short_circuit = interceptor
                    .short_circuit
                    .as_ref()
                    .map_or_else(|| quote! { #root::response::Body }, &body_of);
                Some(sites.add("interceptor", label, short_circuit))
            }
        })
        .collect();
    let fallback_site = router.fallback.as_ref().map(|(_, response)| {
        sites.add(
            "fallback",
            format!("routing fallback response `{}`", type_label(response)),
            body_of(response),
        )
    });

    let context = Context {
        rt: rt.clone(),
        root: root.clone(),
        body_name: body_name.clone(),
        route_enum: route_enum.clone(),
        state_ty: state_ty.clone(),
        handler_sites,
        catcher_sites,
        interceptor_sites,
        rejection_sites,
        fallback_site,
    };

    if let Some((name, response)) = &router.fallback {
        impl_items.push(syn::parse2(quote! {
            #[doc(hidden)]
            async fn __rtr_fallback_response(
                &self,
                __rtr_failure: #rt::RouteFailure<'_>,
            ) -> #root::response::Response<<#response as #root::response::IntoResponse>::Body> {
                #root::response::IntoResponse::into_response(self.#name(__rtr_failure).await)
            }
        })?);
    }

    let core = emit_core(router, &plan, groups, &context, false);
    let mount_core = router.args.erased_mounts.map(|_| emit_core(router, &plan, groups, &context, true));
    let body_sum = emit_body_sum(&body_name, &error_name, &sites, rt, root);

    let (state_generics, state_argument) = match &router.args.state {
        Some(state) => (quote! { <__RtrB> }, quote! { __rtr_state: &#state }),
        None => (quote! { <__RtrB, __RtrS> }, quote! { __rtr_state: &__RtrS }),
    };
    let state_bound = router.args.state.is_none().then(|| quote! { __RtrS: ?::core::marker::Sized, });
    let extractor_bounds = emit_extractor_bounds(router, &plan, &state_ty, rt, root);
    let buffers = router
        .interceptors
        .iter()
        .any(|interceptor| matches!(interceptor.kind, InterceptorKind::TransformBuffered { .. }));
    let transport_bound = buffers.then(|| quote! { __RtrB: #rt::http_body::Body<Data = #rt::bytes::Bytes>, });
    let forwarded_bounds: Vec<&TokenStream2> = router
        .interceptors
        .iter()
        .flat_map(|interceptor| &interceptor.transport_bounds)
        .collect();
    let where_clause = quote! {
        where
            #transport_bound
            #(__RtrB: #forwarded_bounds,)*
            #state_bound
            #(#extractor_bounds,)*
    };
    let body_types: Vec<&TokenStream2> = sites.sites.iter().map(|site| &site.ty).collect();
    let return_ty = quote! {
        #rt::http::Response<#body_name<#(#body_types),*>>
    };

    let after_all: Vec<&Ident> = router
        .interceptors
        .iter()
        .filter(|interceptor| matches!(interceptor.kind, InterceptorKind::After) && interceptor.handlers.is_empty())
        .map(|interceptor| &interceptor.name)
        .collect();
    let after_calls: TokenStream2 = after_all
        .iter()
        .map(|name| {
            quote! {
                {
                    let mut __rtr_after = #rt::AfterContext::new(&__rtr_parts, &mut __rtr_response_parts);
                    __rtr_self.#name(&mut __rtr_after).await;
                }
            }
        })
        .collect();

    let resolver_binding = if has_dynamic {
        quote! { let __rtr_resolver = &self.__rtr_resolver; }
    } else {
        quote! { let __rtr_resolver = #route_enum::resolver(); let __rtr_resolver = &__rtr_resolver; }
    };

    let entry_doc = "Routes one HTTP request through this service's generated dispatch.";
    let entry = if has_dynamic {
        quote! {
            #[doc = #entry_doc]
            pub async fn route #state_generics (
                &self,
                __rtr_service: &#service_ty,
                __rtr_request: #rt::http::Request<__RtrB>,
                #state_argument,
            ) -> #return_ty
            #where_clause
            {
                let __rtr_self = __rtr_service;
                #resolver_binding
                #core
                let (mut __rtr_response_parts, __rtr_response_body) = __rtr_outcome.into_parts();
                #after_calls
                #rt::http::Response::from_parts(__rtr_response_parts, __rtr_response_body)
            }
        }
    } else {
        quote! {
            #[doc = #entry_doc]
            pub async fn route #state_generics (
                &self,
                __rtr_request: #rt::http::Request<__RtrB>,
                #state_argument,
            ) -> #return_ty
            #where_clause
            {
                let __rtr_self = self;
                #resolver_binding
                #core
                let (mut __rtr_response_parts, __rtr_response_body) = __rtr_outcome.into_parts();
                #after_calls
                #rt::http::Response::from_parts(__rtr_response_parts, __rtr_response_body)
            }
        }
    };

    let mount_entry = mount_core.map(|mount_core| {
        let state = router.args.state.as_ref().expect("`erased_mounts` requires a fixed state contract");
        let mount_return = quote! {
            #rt::http::Response<
                #root::response::EitherBody<#body_name<#(#body_types),*>, #root::response::BoxBody>
            >
        };
        let mount_doc = "Routes one HTTP request, delegating a complete routing miss to an explicitly erased mount table.";
        let mount_argument = quote! { __rtr_mounts: &#rt::ErasedMountRouter<__RtrB, #state> };
        let receiver = if has_dynamic {
            quote! { &self, __rtr_service: &#service_ty, }
        } else {
            quote! { &self, }
        };
        let bind_self = if has_dynamic {
            quote! { let __rtr_self = __rtr_service; }
        } else {
            quote! { let __rtr_self = self; }
        };
        quote! {
            #[doc = #mount_doc]
            pub async fn route_with_erased_mounts #state_generics (
                #receiver
                __rtr_request: #rt::http::Request<__RtrB>,
                #state_argument,
                #mount_argument,
            ) -> #mount_return
            #where_clause
            {
                #bind_self
                #resolver_binding
                #mount_core
                let (mut __rtr_response_parts, __rtr_response_body) = __rtr_outcome.into_parts();
                #after_calls
                #rt::http::Response::from_parts(
                    __rtr_response_parts,
                    #root::response::EitherBody::Left { body: __rtr_response_body },
                )
            }
        }
    });

    let router_api = if has_dynamic {
        let add_methods = groups.iter().filter_map(|group| {
            let handler_index = group.dynamic?;
            let handler = &router.handlers[handler_index];
            let add = format_ident!("add_{}", handler.name, span = handler.name.span());
            let doc = format!("Registers a method and path template for the dynamic `{}` handler.", handler.name);
            Some(quote! {
                #[doc = #doc]
                #[must_use]
                pub fn #add(mut self, method: impl ::core::convert::AsRef<str>, path: impl ::core::convert::AsRef<str>) -> Self {
                    self.__rtr_builder = self.__rtr_builder.#add(method, path);
                    self
                }
            })
        });
        let router_doc = format!("A configured router for [`{service}`].");
        let builder_doc = format!("Builds a configured router for [`{service}`].");
        impl_items.push(syn::parse2(quote! {
            /// Creates a builder for this service's static and dynamic routes.
            #[must_use]
            pub fn router_builder() -> #builder_name {
                #builder_name {
                    __rtr_builder: #route_enum::builder(),
                }
            }
        })?);
        quote! {
            #[doc = #router_doc]
            #[derive(Debug)]
            pub struct #router_name {
                __rtr_resolver: #resolver_ty,
            }

            #[doc = #builder_doc]
            #[derive(Debug)]
            pub struct #builder_name {
                __rtr_builder: #resolver_builder_ty,
            }

            #[automatically_derived]
            impl #builder_name {
                #(#add_methods)*

                /// Validates dynamic registrations and builds the service router.
                ///
                /// # Errors
                ///
                /// Returns `routerama::route::ConfigurationError` containing every
                /// missing or invalid dynamic route registration.
                pub fn build(self) -> ::core::result::Result<#router_name, #rt::ConfigurationError> {
                    ::core::result::Result::Ok(#router_name {
                        __rtr_resolver: self.__rtr_builder.build()?,
                    })
                }
            }

            #[automatically_derived]
            #[allow(
                private_interfaces,
                reason = "the service type may intentionally be private to its module"
            )]
            impl #router_name {
                #entry
                #mount_entry
            }
        }
    } else {
        impl_items.push(syn::parse2(entry)?);
        if let Some(mount_entry) = mount_entry {
            impl_items.push(syn::parse2(mount_entry)?);
        }
        quote! {}
    };

    let witnesses = emit_witnesses(router, rt, root);

    Ok(Generated {
        items: quote! {
            #resolver_items
            #body_sum
            #router_api
            #witnesses
        },
        impl_items,
    })
}

/// Re-spans a type's tokens at the macro invocation.
fn respan(ty: &Type) -> TokenStream2 {
    use quote::ToTokens as _;

    fn walk(stream: TokenStream2) -> TokenStream2 {
        stream
            .into_iter()
            .map(|tree| {
                let mut tree = match tree {
                    proc_macro2::TokenTree::Group(group) => {
                        proc_macro2::TokenTree::Group(proc_macro2::Group::new(group.delimiter(), walk(group.stream())))
                    }
                    other => other,
                };
                tree.set_span(Span::call_site());
                tree
            })
            .collect()
    }

    walk(ty.to_token_stream())
}

fn quote_spanned_body(ty: &Type) -> TokenStream2 {
    quote::quote_spanned! { ty.span() => __routerama_assert_body::<#ty, _>(); }
}

/// Shared state threaded through dispatch generation.
struct Context {
    rt: TokenStream2,
    root: TokenStream2,
    body_name: Ident,
    route_enum: Ident,
    state_ty: TokenStream2,
    handler_sites: Vec<Ident>,
    catcher_sites: Vec<Option<Ident>>,
    interceptor_sites: Vec<Option<Ident>>,
    rejection_sites: Vec<Vec<Option<Ident>>>,
    fallback_site: Option<Ident>,
}

impl Context {
    /// Converts `value` into the generated response sum's `variant`.
    fn respond(&self, value: &TokenStream2, variant: &Ident, produces: Option<&LitStr>, after: &[&Ident]) -> TokenStream2 {
        let root = &self.root;
        let rt = &self.rt;
        let body = &self.body_name;
        let content_type = produces.map(|value| {
            quote! { #rt::set_produced_content_type(&mut __rtr_out, #value); }
        });
        let after_calls = after.iter().map(|name| {
            quote! {
                {
                    let mut __rtr_after = #rt::AfterContext::new(&__rtr_parts, &mut __rtr_rp);
                    __rtr_self.#name(&mut __rtr_after).await;
                }
            }
        });
        quote! {
            {
                #[allow(unused_mut, reason = "only produced routes mutate the response before splitting it")]
                let mut __rtr_out = #root::response::IntoResponse::into_response(#value);
                #content_type
                let (mut __rtr_rp, __rtr_rb) = __rtr_out.into_parts();
                #(#after_calls)*
                #rt::http::Response::from_parts(__rtr_rp, #body::#variant { body: __rtr_rb })
            }
        }
    }

    /// Converts `value` into the sum through one explicit `BoxBody` erasure.
    fn respond_erased(&self, value: &TokenStream2, variant: &Ident) -> TokenStream2 {
        let root = &self.root;
        let rt = &self.rt;
        let body = &self.body_name;
        quote! {
            {
                let __rtr_out = #root::response::IntoResponse::into_response(#value);
                let (__rtr_rp, __rtr_rb) = __rtr_out.into_parts();
                #rt::http::Response::from_parts(
                    __rtr_rp,
                    #body::#variant { body: #root::response::SendBoxBody::new(__rtr_rb) },
                )
            }
        }
    }

    /// Converts a `routerama::response::Body` response into the fixed variant.
    fn respond_fixed(&self, value: &TokenStream2) -> TokenStream2 {
        let root = &self.root;
        let rt = &self.rt;
        let body = &self.body_name;
        quote! {
            {
                let __rtr_out = #root::response::IntoResponse::into_response(#value);
                let (__rtr_rp, __rtr_rb) = __rtr_out.into_parts();
                #rt::http::Response::from_parts(__rtr_rp, #body::Fixed { body: __rtr_rb })
            }
        }
    }

    /// The response for a routing or predicate failure.
    fn failure(&self, failure: &TokenStream2) -> TokenStream2 {
        match &self.fallback_site {
            Some(variant) => {
                let rt = &self.rt;
                let body = &self.body_name;
                quote! {
                    {
                        let __rtr_out = __rtr_self.__rtr_fallback_response(#failure).await;
                        let (__rtr_rp, __rtr_rb) = __rtr_out.into_parts();
                        #rt::http::Response::from_parts(__rtr_rp, #body::#variant { body: __rtr_rb })
                    }
                }
            }
            None => self.respond_fixed(failure),
        }
    }
}

fn emit_core(router: &Router, plan: &RejectionPlan, groups: &[Group], context: &Context, mounts: bool) -> TokenStream2 {
    let rt = &context.rt;
    let root = &context.root;
    let before_all: Vec<(&Ident, &Ident)> = router
        .interceptors
        .iter()
        .zip(&context.interceptor_sites)
        .filter(|(interceptor, _)| matches!(interceptor.kind, InterceptorKind::Before) && interceptor.handlers.is_empty())
        .filter_map(|(interceptor, site)| site.as_ref().map(|site| (&interceptor.name, site)))
        .collect();
    let before_calls = before_all.iter().map(|(name, site)| {
        let respond = context.respond(&quote! { __rtr_short }, site, None, &[]);
        quote! {
            {
                let mut __rtr_before = #rt::BeforeContext::new(&mut __rtr_parts);
                match __rtr_self.#name(&mut __rtr_before).await {
                    #rt::Before::Next => {}
                    #rt::Before::Respond(__rtr_short) => break '__rtr_route #respond,
                }
            }
        }
    });

    let arms: Vec<TokenStream2> = groups.iter().map(|group| emit_group(router, plan, group, context)).collect();

    let miss = context.failure(&quote! { #rt::route_failure(__rtr_error) });
    let delegate = mounts.then(|| {
        quote! {
            if ::core::matches!(__rtr_error, #rt::ResolveError::NotFound(_)) {
                let __rtr_request = #rt::http::Request::from_parts(__rtr_parts, __rtr_body);
                let __rtr_mounted = #rt::ErasedMountRouter::route(__rtr_mounts, __rtr_request, __rtr_state).await;
                let (__rtr_mounted_parts, __rtr_mounted_body) = __rtr_mounted.into_parts();
                return #rt::http::Response::from_parts(
                    __rtr_mounted_parts,
                    #root::response::EitherBody::Right { body: __rtr_mounted_body },
                );
            }
        }
    });

    // A router that never consumes the transport body drops it before the
    // first await, so the generated future does not retain the transport body
    // type and stays `Send` whenever the caller's own types are.
    let consumes_body = mounts
        || router.handlers.iter().any(Handler::has_body)
        || router.interceptors.iter().any(|interceptor| {
            matches!(
                interceptor.kind,
                InterceptorKind::TransformBuffered { .. } | InterceptorKind::TransformStream { .. }
            )
        });
    let release_body = (!consumes_body).then(|| quote! { ::core::mem::drop(__rtr_body); });

    quote! {
        let (mut __rtr_parts, __rtr_body) = __rtr_request.into_parts();
        #release_body
        let __rtr_outcome = '__rtr_route: {
            #(#before_calls)*
            let __rtr_path: &str = __rtr_parts.uri.path();
            let __rtr_resolved = #rt::Resolver::resolve(__rtr_resolver, __rtr_parts.method.as_str(), __rtr_path);
            match __rtr_resolved {
                ::core::result::Result::Ok(__rtr_matched) => match __rtr_matched {
                    #(#arms)*
                },
                ::core::result::Result::Err(__rtr_error) => {
                    #delegate
                    break '__rtr_route #miss;
                }
            }
        };
    }
}

fn emit_group(router: &Router, plan: &RejectionPlan, group: &Group, context: &Context) -> TokenStream2 {
    let route_enum = &context.route_enum;
    let rt = &context.rt;
    let variant = &group.variant;
    let pattern = if group.captures.is_empty() {
        quote! { #route_enum::#variant }
    } else {
        let fields = group.captures.iter().map(|(name, _)| name);
        quote! { #route_enum::#variant { #(#fields),* } }
    };

    let candidates: Vec<(usize, usize)> = match group.dynamic {
        Some(handler_index) => router.handlers[handler_index]
            .routes
            .first()
            .map(|_| (handler_index, 0))
            .into_iter()
            .collect(),
        None => group
            .candidates
            .iter()
            .map(|candidate| (candidate.handler, candidate.decl))
            .collect(),
    };
    let single_plain = candidates.len() <= 1
        && candidates
            .first()
            .is_none_or(|(handler, decl)| !router.handlers[*handler].routes[*decl].has_predicates());

    let body = if single_plain {
        let (handler_index, decl_index) = match candidates.first() {
            Some((handler, decl)) => (*handler, Some(*decl)),
            None => (group.dynamic.expect("a static group always has a candidate"), None),
        };
        emit_dispatch(router, plan, handler_index, decl_index, context)
    } else {
        let mut checks = Vec::new();
        for (handler_index, decl_index) in &candidates {
            let handler = &router.handlers[*handler_index];
            let decl = &handler.routes[*decl_index];
            let host = decl
                .host
                .as_ref()
                .map_or_else(|| quote! { true }, |value| quote! { #rt::host_matches(&__rtr_parts, #value) });
            let consumes = decl.consumes.as_ref().map_or_else(
                || quote! { true },
                |value| quote! { #rt::content_type_matches(&__rtr_parts.headers, #value) },
            );
            let produces = decl
                .produces
                .as_ref()
                .map_or_else(|| quote! { true }, |value| quote! { #rt::accepts(&__rtr_parts.headers, #value) });
            let dispatch = emit_dispatch(router, plan, *handler_index, Some(*decl_index), context);
            checks.push(quote! {
                let __rtr_host_ok = #host;
                let __rtr_type_ok = #consumes;
                let __rtr_accept_ok = #produces;
                if __rtr_host_ok && __rtr_type_ok && __rtr_accept_ok {
                    #dispatch
                }
                if !__rtr_host_ok {
                    if __rtr_stage < 1 { __rtr_stage = 1; }
                } else if !__rtr_type_ok {
                    if __rtr_stage < 2 { __rtr_stage = 2; }
                } else if !__rtr_accept_ok {
                    if __rtr_stage < 3 { __rtr_stage = 3; }
                }
            });
        }
        let failure = context.failure(&quote! {
            match __rtr_stage {
                1 => #rt::RouteFailure::HostMismatch { path: __rtr_path },
                2 => #rt::RouteFailure::UnsupportedMediaType { path: __rtr_path },
                3 => #rt::RouteFailure::NotAcceptable { path: __rtr_path },
                _ => #rt::RouteFailure::NotFound { path: __rtr_path },
            }
        });
        quote! {
            let mut __rtr_stage: u8 = 0;
            #({ #checks })*
            break '__rtr_route #failure;
        }
    };

    quote! { #pattern => { #body } }
}

#[expect(
    clippy::too_many_lines,
    reason = "one dispatch arm sequences interceptors, the transform, extraction, and the handler call in order"
)]
fn emit_dispatch(
    router: &Router,
    plan: &RejectionPlan,
    handler_index: usize,
    decl_index: Option<usize>,
    context: &Context,
) -> TokenStream2 {
    let rt = &context.rt;
    let handler = &router.handlers[handler_index];
    let name = &handler.name;
    let produces = decl_index.and_then(|index| handler.routes[index].produces.as_ref());

    let before_calls = router
        .interceptors
        .iter()
        .zip(&context.interceptor_sites)
        .filter(|(interceptor, _)| matches!(interceptor.kind, InterceptorKind::Before) && interceptor.handlers.contains(name))
        .filter_map(|(interceptor, site)| site.as_ref().map(|site| (interceptor, site)))
        .map(|(interceptor, site)| {
            let interceptor_name = &interceptor.name;
            let respond = context.respond(&quote! { __rtr_short }, site, None, &[]);
            quote! {
                {
                    let mut __rtr_selected = #rt::SelectedContext::new(
                        &__rtr_parts.method,
                        &__rtr_parts.uri,
                        __rtr_parts.version,
                        &mut __rtr_parts.headers,
                        &mut __rtr_parts.extensions,
                    );
                    match __rtr_self.#interceptor_name(&mut __rtr_selected).await {
                        #rt::Before::Next => {}
                        #rt::Before::Respond(__rtr_short) => break '__rtr_route #respond,
                    }
                }
            }
        })
        .collect::<Vec<_>>();

    let transform = router
        .interceptors
        .iter()
        .zip(&context.interceptor_sites)
        .find(|(interceptor, _)| {
            matches!(
                interceptor.kind,
                InterceptorKind::TransformBuffered { .. } | InterceptorKind::TransformStream { .. }
            ) && interceptor.handlers.contains(name)
        })
        .and_then(|(interceptor, site)| site.as_ref().map(|site| emit_transform(interceptor, site, context)));

    let after_handler: Vec<&Ident> = router
        .interceptors
        .iter()
        .filter(|interceptor| matches!(interceptor.kind, InterceptorKind::After) && interceptor.handlers.contains(name))
        .map(|interceptor| &interceptor.name)
        .collect();

    let mut extraction = Vec::new();
    let mut arguments = Vec::new();
    let state_ty = &context.state_ty;
    let replacement = router
        .interceptors
        .iter()
        .find(|interceptor| {
            matches!(
                interceptor.kind,
                InterceptorKind::TransformBuffered { .. } | InterceptorKind::TransformStream { .. }
            ) && interceptor.handlers.contains(name)
        })
        .and_then(|interceptor| interceptor.replacement.clone());
    let transport = replacement
        .as_ref()
        .map_or_else(|| quote! { __RtrB }, |replacement| quote! { #replacement });
    for (param_index, param) in handler.params.iter().enumerate() {
        let param_name = &param.name;
        let param_ty = &param.ty;
        let site = context.rejection_sites[handler_index][param_index].as_ref();
        match param.kind {
            ParamKind::Capture | ParamKind::DynamicCapture => {
                arguments.push(quote! { #param_name });
            }
            ParamKind::Parts => {
                let rejection = emit_rejection(router, plan, handler_index, param_index, param, site, context);
                extraction.push(quote! {
                    let #param_name: #param_ty = match <#param_ty as #rt::FromRequestParts<'_, #state_ty>>::from_request_parts(
                        &__rtr_parts,
                        __rtr_state,
                    ) {
                        ::core::result::Result::Ok(__rtr_value) => __rtr_value,
                        ::core::result::Result::Err(__rtr_rejection) => break '__rtr_route #rejection,
                    };
                });
                arguments.push(quote! { #param_name });
            }
            ParamKind::Body => {
                let rejection = emit_rejection(router, plan, handler_index, param_index, param, site, context);
                extraction.push(quote! {
                    let #param_name: #param_ty = match <#param_ty as #rt::FromRequestBody<#state_ty, #transport>>::from_request_body(
                        &__rtr_parts,
                        __rtr_body,
                        __rtr_state,
                    ).await {
                        ::core::result::Result::Ok(__rtr_value) => __rtr_value,
                        ::core::result::Result::Err(__rtr_rejection) => break '__rtr_route #rejection,
                    };
                });
                arguments.push(quote! { #param_name });
            }
        }
    }

    let handler_site = &context.handler_sites[handler_index];
    let respond = context.respond(&quote! { __rtr_value }, handler_site, produces, &after_handler);
    quote! {
        #(#before_calls)*
        #transform
        #(#extraction)*
        let __rtr_value = __rtr_self.#name(#(#arguments),*).await;
        break '__rtr_route #respond;
    }
}

fn emit_transform(interceptor: &Interceptor, site: &Ident, context: &Context) -> TokenStream2 {
    let rt = &context.rt;
    let name = &interceptor.name;
    let respond = context.respond(&quote! { __rtr_short }, site, None, &[]);
    match &interceptor.kind {
        InterceptorKind::TransformBuffered { limit, consumes } => {
            let rejection = context.respond_fixed(&quote! { __rtr_rejection });
            let buffered = quote! {
                let __rtr_buffered = match #rt::buffer_request_body::<_, { #limit }>(__rtr_body).await {
                    ::core::result::Result::Ok(__rtr_bytes) => __rtr_bytes,
                    ::core::result::Result::Err(__rtr_rejection) => break '__rtr_route #rejection,
                };
            };
            if *consumes {
                quote! {
                    #buffered
                    match __rtr_self.#name(&__rtr_parts, __rtr_buffered).await {
                        #rt::BodyConsumed::Consumed => {}
                        #rt::BodyConsumed::Respond(__rtr_short) => break '__rtr_route #respond,
                    }
                }
            } else {
                quote! {
                    #buffered
                    let __rtr_body = match __rtr_self.#name(&__rtr_parts, __rtr_buffered).await {
                        #rt::BodyTransform::Replace(__rtr_replacement) => __rtr_replacement,
                        #rt::BodyTransform::Respond(__rtr_short) => break '__rtr_route #respond,
                    };
                }
            }
        }
        InterceptorKind::TransformStream { consumes } => {
            if *consumes {
                quote! {
                    match __rtr_self.#name(&__rtr_parts, __rtr_body).await {
                        #rt::BodyConsumed::Consumed => {}
                        #rt::BodyConsumed::Respond(__rtr_short) => break '__rtr_route #respond,
                    }
                }
            } else {
                quote! {
                    let __rtr_body = match __rtr_self.#name(&__rtr_parts, __rtr_body).await {
                        #rt::BodyTransform::Replace(__rtr_replacement) => __rtr_replacement,
                        #rt::BodyTransform::Respond(__rtr_short) => break '__rtr_route #respond,
                    };
                }
            }
        }
        InterceptorKind::Before | InterceptorKind::After => quote! {},
    }
}

/// The catcher that owns an extractor's rejection, if any.
fn catcher_for(router: &Router, param: &super::model::Param) -> Option<usize> {
    let extractor = type_base(&param.ty);
    let direct = router
        .catchers
        .iter()
        .position(|catcher| catcher.from_base.as_deref() == Some(extractor.as_str()));
    direct.or_else(|| {
        let rejection = builtin_rejection(&extractor)?;
        router
            .catchers
            .iter()
            .position(|catcher| catcher.from_base.is_none() && catcher.rejection_base == rejection)
    })
}

/// Emits an extractor rejection response, routing through a catcher when one
/// declares the extractor or its rejection type.
fn emit_rejection(
    router: &Router,
    plan: &RejectionPlan,
    handler_index: usize,
    param_index: usize,
    param: &super::model::Param,
    site: Option<&Ident>,
    context: &Context,
) -> TokenStream2 {
    let _ = param;
    match plan.slots[handler_index][param_index] {
        Slot::Caught(index) => {
            let name = &router.catchers[index].name;
            let catcher_site = context.catcher_sites[index]
                .as_ref()
                .expect("a matched catcher always reserves a response site");
            context.respond(&quote! { __rtr_self.#name(__rtr_rejection).await }, catcher_site, None, &[])
        }
        Slot::Erased => {
            let site = site.expect("an erased rejection always reserves a response site");
            context.respond_erased(&quote! { __rtr_rejection }, site)
        }
        Slot::Projected | Slot::None => {
            let site = site.expect("an uncaught rejection always reserves a response site");
            context.respond(&quote! { __rtr_rejection }, site, None, &[])
        }
    }
}

/// The rejection type produced by each built-in extractor.
pub(crate) fn builtin_rejection(extractor: &str) -> Option<&'static str> {
    match extractor {
        "Query" => Some("QueryRejection"),
        "Json" => Some("JsonRejection"),
        "Form" => Some("FormRejection"),
        "BytesBody" | "TextBody" => Some("BodyRejection"),
        "RawBody" => Some("Infallible"),
        "ExtensionRef" | "ClonedExtension" => Some("MissingExtension"),
        _ => None,
    }
}

/// Definition-time proof that every extractor supports the fixed state type.
fn emit_witnesses(router: &Router, rt: &TokenStream2, root: &TokenStream2) -> TokenStream2 {
    let Some(state) = &router.args.state else {
        return quote! {};
    };
    let mut proofs = Vec::new();
    for handler in &router.handlers {
        for param in &handler.params {
            match param.kind {
                ParamKind::Parts => {
                    let ty = &param.ty;
                    if lifetime_free(ty) {
                        // The proof is spanned at the annotation, not the
                        // handler parameter, so the diagnostic names the
                        // router contract that requires it.
                        let ty = respan(ty);
                        proofs.push(quote! { __routerama_assert_parts::<#ty>(); });
                    }
                }
                ParamKind::Body => {
                    let ty = &param.ty;
                    // Built-in body extractors ship their own witnesses; their
                    // generic rejection would make the proof's rejection type
                    // parameter ambiguous.
                    if builtin_rejection(&type_base(ty)).is_none() {
                        proofs.push(quote_spanned_body(ty));
                    }
                }
                ParamKind::Capture | ParamKind::DynamicCapture => {}
            }
        }
    }
    quote! {
        #[automatically_derived]
        #[doc(hidden)]
        const _: () = {
            // The state is spelled exactly once here, so an ill-formed state
            // type - an omitted lifetime parameter, for example - reports one
            // diagnostic at the annotation instead of one per witness bound.
            #[allow(dead_code, reason = "the alias only checks that the state type is well formed")]
            type __RoutersmaState = #state;

            #[allow(dead_code, reason = "the witnesses are compile-time proofs only")]
            fn __routerama_assert_parts<__RtrT>()
            where
                __RtrT: for<'__routerama_witness> #rt::FromRequestParts<'__routerama_witness, __RoutersmaState>,
            {
            }

            #[allow(dead_code, reason = "the witnesses are compile-time proofs only")]
            fn __routerama_assert_body<__RtrT, __RtrR>()
            where
                __RtrR: #root::response::IntoResponse,
                __RtrT: #rt::BodyStateWitness<__RoutersmaState, __RtrR>
                    + #rt::FromRequestBody<
                        __RoutersmaState,
                        <__RtrT as #rt::BodyStateWitness<__RoutersmaState, __RtrR>>::RequestBody,
                    >,
            {
            }

            #[allow(dead_code, reason = "the witnesses are compile-time proofs only")]
            fn __rtr_prove() {
                #(#proofs)*
            }
        };
    }
}

fn lifetime_free(ty: &Type) -> bool {
    struct Finder(bool);

    impl<'ast> syn::visit::Visit<'ast> for Finder {
        fn visit_lifetime(&mut self, _lifetime: &'ast syn::Lifetime) {
            self.0 = false;
        }

        fn visit_type_reference(&mut self, _reference: &'ast syn::TypeReference) {
            self.0 = false;
        }
    }

    let mut finder = Finder(true);
    syn::visit::Visit::visit_type(&mut finder, ty);
    finder.0
}
