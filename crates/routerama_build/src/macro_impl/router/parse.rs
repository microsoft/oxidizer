// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Parsing and validation of a `#[router]` annotated inherent impl.

use alloc::format;
use alloc::string::{String, ToString as _};
use alloc::vec::Vec;

use http_path_template::{Grammar, PathTemplate};
use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::ToTokens as _;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned as _;
use syn::{Attribute, Error, FnArg, GenericArgument, ImplItem, ImplItemFn, ItemImpl, Pat, PathArguments, ReturnType, Token, Type};

use super::emit::builtin_rejection;
use super::model::{Catcher, Handler, HandlerKind, Interceptor, InterceptorKind, Param, ParamKind, RouteDecl, Router, RouterArgs};
use crate::macro_impl::RouteAttr;
use crate::route_field_name;
use crate::trie::capture_field_names;

/// The interceptor and policy attribute names a router method may declare.
const INTERCEPTOR_ATTRS: [&str; 3] = ["before", "after", "transform"];

impl Parse for RouterArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut args = Self {
            state: None,
            erased_mounts: None,
        };
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            match key.to_string().as_str() {
                "state" => {
                    if args.state.is_some() {
                        return Err(Error::new(key.span(), "duplicate `state` router argument"));
                    }
                    let _equals: Token![=] = input.parse()?;
                    let state: Type = input.parse()?;
                    validate_state_type(&state)?;
                    args.state = Some(state);
                }
                "erased_mounts" => {
                    if args.erased_mounts.is_some() {
                        return Err(Error::new(key.span(), "duplicate `erased_mounts` router argument"));
                    }
                    args.erased_mounts = Some(key.span());
                }
                _ => {
                    return Err(Error::new(
                        key.span(),
                        "unknown router argument; expected `state = StateType` or `erased_mounts`",
                    ));
                }
            }
            if input.is_empty() {
                break;
            }
            let _comma: Token![,] = input.parse()?;
        }
        Ok(args)
    }
}

/// Rejects state spellings a generated entry point cannot name unambiguously.
fn validate_state_type(state: &Type) -> syn::Result<()> {
    if let Some(span) = anonymous_lifetime_span(state) {
        return Err(Error::new(
            span,
            "router state types cannot contain `'_`; use an owned type or an explicit `'static` reference",
        ));
    }
    match state {
        Type::Infer(inferred) => Err(Error::new(inferred.span(), "router state types cannot be inferred")),
        Type::ImplTrait(opaque) => Err(Error::new(opaque.span(), "router state types cannot use `impl Trait`")),
        Type::Macro(mac) => Err(Error::new(mac.span(), "router state types cannot be produced by a macro")),
        Type::Path(path) if path.qself.is_none() && path.path.is_ident("Self") => Err(Error::new(
            state.span(),
            "`Self` is not a valid router state type because generated configured-router methods have a different `Self`; name the service or use a fully qualified associated type",
        )),
        _ => Ok(()),
    }
}

/// The span of the first `'_` lifetime inside `ty`, if any.
fn anonymous_lifetime_span(ty: &Type) -> Option<Span> {
    struct Finder(Option<Span>);

    impl<'ast> syn::visit::Visit<'ast> for Finder {
        fn visit_lifetime(&mut self, i: &'ast syn::Lifetime) {
            if self.0.is_none() && i.ident == "_" {
                self.0 = Some(i.span());
            }
        }
    }

    let mut finder = Finder(None);
    syn::visit::Visit::visit_type(&mut finder, ty);
    finder.0
}

/// The span of an explicit lifetime that would tie a parts extractor to a
/// specific request borrow.
///
/// Only the extractor's own outermost lifetime matters: a nested `'static`
/// type argument (for example `Nested<&'static str>`) does not describe the
/// request-parts borrow.
fn named_lifetime_span(ty: &Type) -> Option<Span> {
    match ty {
        Type::Reference(reference) => reference
            .lifetime
            .as_ref()
            .filter(|lifetime| lifetime.ident != "_")
            .map(syn::spanned::Spanned::span),
        Type::Group(group) => named_lifetime_span(&group.elem),
        Type::Paren(paren) => named_lifetime_span(&paren.elem),
        Type::Path(path) => path.path.segments.last().and_then(|segment| {
            let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
                return None;
            };
            arguments.args.iter().find_map(|argument| match argument {
                GenericArgument::Lifetime(lifetime) if lifetime.ident != "_" => Some(lifetime.span()),
                _ => None,
            })
        }),
        _ => None,
    }
}

/// The last path segment identifier of a type, used for policy matching.
pub(crate) fn type_base(ty: &Type) -> String {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map_or_else(String::new, |segment| segment.ident.to_string()),
        Type::Reference(reference) => type_base(&reference.elem),
        Type::Group(group) => type_base(&group.elem),
        Type::Paren(paren) => type_base(&paren.elem),
        _ => String::new(),
    }
}

pub(crate) fn parse_router(args: RouterArgs, item: &ItemImpl) -> syn::Result<Router> {
    validate_impl(item)?;
    if args.erased_mounts.is_some() && args.state.is_none() {
        return Err(Error::new(
            args.erased_mounts.unwrap_or_else(Span::call_site),
            "`erased_mounts` requires a fixed `state = StateType` router contract",
        ));
    }
    let service_name = service_name(&item.self_ty)?;

    let mut handlers = Vec::new();
    let mut interceptors = Vec::new();
    let mut catchers = Vec::new();
    let mut fallback: Option<(Ident, Type, Span)> = None;

    for impl_item in &item.items {
        let ImplItem::Fn(method) = impl_item else {
            if let Some(attribute) = router_attribute(impl_item_attrs(impl_item)) {
                return Err(Error::new(
                    attribute.span(),
                    "`#[route]` and router policy attributes may only annotate async service methods",
                ));
            }
            continue;
        };
        let is_route = method.attrs.iter().any(|attr| attr.path().is_ident("route"));
        let interceptor_attr = method
            .attrs
            .iter()
            .find(|attr| INTERCEPTOR_ATTRS.iter().any(|name| attr.path().is_ident(name)));
        let interceptor_count = method
            .attrs
            .iter()
            .filter(|attr| INTERCEPTOR_ATTRS.iter().any(|name| attr.path().is_ident(name)))
            .count();
        let is_fallback = method.attrs.iter().any(|attr| attr.path().is_ident("fallback"));
        let catch_attr = method.attrs.iter().find(|attr| attr.path().is_ident("catch"));

        if interceptor_count > 1 {
            return Err(Error::new(
                method.sig.ident.span(),
                "a method may declare only one `#[before]`, `#[after]`, or `#[transform]` interceptor annotation",
            ));
        }
        if interceptor_attr.is_some() && is_route {
            return Err(Error::new(
                method.sig.ident.span(),
                "a method cannot be both a route handler and a `#[before]`, `#[after]`, or `#[transform]` interceptor",
            ));
        }
        if interceptor_attr.is_some() && (is_fallback || catch_attr.is_some()) {
            return Err(Error::new(
                method.sig.ident.span(),
                "an interceptor method cannot also be a `#[fallback]` or `#[catch]` policy method",
            ));
        }

        if let Some(attribute) = interceptor_attr {
            interceptors.push(parse_interceptor(method, attribute)?);
            continue;
        }
        if is_fallback {
            let attribute = method
                .attrs
                .iter()
                .find(|attr| attr.path().is_ident("fallback"))
                .expect("the fallback attribute was just observed");
            validate_policy_signature(method)?;
            if let Some((first, _, _)) = &fallback {
                return Err(Error::new(
                    attribute.path().span(),
                    "a generated service may declare only one `#[fallback]` method",
                )
                .tap_combine(Error::new(first.span(), "the first fallback is declared here")));
            }
            fallback = Some((method.sig.ident.clone(), response_type(method)?, attribute.path().span()));
            continue;
        }
        if let Some(attribute) = catch_attr {
            catchers.push(parse_catcher(method, attribute)?);
            continue;
        }
        if is_route {
            handlers.push(parse_handler(method)?);
        }
    }

    if handlers.is_empty() {
        return Err(Error::new(
            item.impl_token.span(),
            "`#[router]` requires at least one `#[route]` handler",
        ));
    }
    validate_catchers(&catchers)?;
    validate_catcher_usage(&catchers, &handlers)?;
    validate_interceptors(&interceptors, &handlers)?;

    Ok(Router {
        args,
        service_ty: item.self_ty.as_ref().clone(),
        service_name,
        handlers,
        interceptors,
        fallback: fallback.map(|(name, response, _)| (name, response)),
        catchers,
    })
}

/// A small extension that combines two diagnostics into one error value.
trait TapCombine {
    fn tap_combine(self, other: Error) -> Error;
}

impl TapCombine for Error {
    fn tap_combine(mut self, other: Error) -> Error {
        self.combine(other);
        self
    }
}

fn impl_item_attrs(impl_item: &ImplItem) -> &[Attribute] {
    match impl_item {
        ImplItem::Const(item) => &item.attrs,
        ImplItem::Fn(item) => &item.attrs,
        ImplItem::Type(item) => &item.attrs,
        ImplItem::Macro(item) => &item.attrs,
        _ => &[],
    }
}

fn router_attribute(attrs: &[Attribute]) -> Option<&Attribute> {
    attrs.iter().find(|attr| {
        ["route", "fallback", "catch"].iter().any(|name| attr.path().is_ident(name))
            || INTERCEPTOR_ATTRS.iter().any(|name| attr.path().is_ident(name))
    })
}

fn validate_impl(item: &ItemImpl) -> syn::Result<()> {
    if item.trait_.is_some() {
        return Err(Error::new(item.impl_token.span(), "`#[router]` requires an inherent impl"));
    }
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(Error::new(item.generics.span(), "`#[router]` does not support generic impl blocks"));
    }
    if item.unsafety.is_some() {
        return Err(Error::new(
            item.impl_token.span(),
            "`#[router]` does not support unsafe impl blocks",
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
        return Err(Error::new(self_ty.span(), "`#[router]` does not support generic service types"));
    }
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.clone())
        .ok_or_else(|| Error::new(self_ty.span(), "`#[router]` requires a named service type"))
}

fn parse_handler(method: &ImplItemFn) -> syn::Result<Handler> {
    validate_signature(method)?;
    let route_attrs: Vec<&Attribute> = method.attrs.iter().filter(|attr| attr.path().is_ident("route")).collect();

    let mut decls = Vec::with_capacity(route_attrs.len());
    let mut dynamic = false;
    for attribute in &route_attrs {
        let parsed: RouteAttr = attribute.parse_args()?;
        if let Some(span) = parsed.dynamic {
            if let Some((_, priority_span)) = parsed.priority {
                return Err(Error::new(
                    priority_span,
                    "configured dynamic routes reject `priority`: runtime registrations must not overlap, and static routes remain direct",
                ));
            }
            if route_attrs.len() != 1 {
                return Err(Error::new(
                    span,
                    "`#[route(dynamic)]` cannot be combined with another route attribute",
                ));
            }
            dynamic = true;
            decls.push(RouteDecl {
                attr_span: attribute.path().span(),
                method: String::new(),
                path: parsed.path,
                host: parsed.host,
                consumes: parsed.consumes,
                produces: parsed.produces,
                priority: None,
            });
            continue;
        }
        decls.push(RouteDecl {
            attr_span: attribute.path().span(),
            method: parsed.method,
            path: parsed.path,
            host: parsed.host,
            consumes: parsed.consumes,
            produces: parsed.produces,
            priority: parsed.priority,
        });
    }

    let kind = if dynamic { HandlerKind::Dynamic } else { HandlerKind::Static };
    if kind == HandlerKind::Static {
        validate_alias_predicates(&decls)?;
    }

    let capture_names = if kind == HandlerKind::Static {
        route_capture_names(&decls)?
    } else {
        Vec::new()
    };

    let params = parse_params(method, kind, &capture_names)?;
    if kind == HandlerKind::Static {
        let mut declared: Vec<String> = params
            .iter()
            .filter(|param| param.kind == ParamKind::Capture)
            .map(|param| param.name.to_string())
            .collect();
        declared.sort();
        let mut expected = capture_names;
        expected.sort();
        if declared != expected {
            return Err(Error::new(
                method.sig.ident.span(),
                format!(
                    "handler `{}` capture parameters {declared:?} do not match its path captures {expected:?}",
                    method.sig.ident
                ),
            ));
        }
    }

    let response = response_type(method)?;
    Ok(Handler {
        name: method.sig.ident.clone(),
        kind,
        params,
        response,
        routes: decls,
    })
}

/// Every static alias on one handler shares the one generated route variant.
fn validate_alias_predicates(decls: &[RouteDecl]) -> syn::Result<()> {
    let Some(first) = decls.first() else {
        return Ok(());
    };
    let expected = first.predicates();
    for decl in &decls[1..] {
        if decl.predicates() != expected {
            let span = decl
                .host
                .as_ref()
                .or(decl.consumes.as_ref())
                .or(decl.produces.as_ref())
                .map_or(decl.attr_span, syn::spanned::Spanned::span);
            return Err(Error::new(
                span,
                "every static `#[route]` alias on one handler must declare identical `host`, `consumes`, and `produces` predicates",
            ));
        }
    }
    Ok(())
}

fn route_capture_names(decls: &[RouteDecl]) -> syn::Result<Vec<String>> {
    let mut first: Option<Vec<String>> = None;
    for decl in decls {
        let value = decl.path.value();
        let template = PathTemplate::parse(&value, Grammar::default().with_segment_affixes())
            .map_err(|error| Error::new(decl.path.span(), format!("invalid path template: {error}")))?;
        let mut captures: Vec<String> = capture_field_names(template.segments())
            .into_iter()
            .map(|name| route_field_name(name.join(".")))
            .collect();
        captures.sort();
        if first.as_ref().is_some_and(|expected| expected != &captures) {
            return Err(Error::new(
                decl.path.span(),
                "every `#[route]` on one handler must capture the same path variables",
            ));
        }
        first = Some(captures);
    }
    Ok(first.unwrap_or_default())
}

fn parse_params(method: &ImplItemFn, kind: HandlerKind, capture_names: &[String]) -> syn::Result<Vec<Param>> {
    let mut params = Vec::new();
    let mut body_marker: Option<Span> = None;
    for input in method.sig.inputs.iter().skip(1) {
        let FnArg::Typed(input) = input else {
            return Err(Error::new(input.span(), "service handlers must have exactly one `&self` receiver"));
        };
        let pattern = parameter_pattern(input.pat.as_ref())?;
        let has_body = input.attrs.iter().any(|attr| attr.path().is_ident("body"));
        let has_capture = input.attrs.iter().any(|attr| attr.path().is_ident("capture"));
        if has_body {
            if let Some(first) = body_marker {
                return Err(Error::new(
                    pattern.ident.span(),
                    "a route handler may have at most one `#[body]` parameter because the request body can be consumed only once",
                )
                .tap_combine(Error::new(first, "the first request-body consumer is here")));
            }
            body_marker = Some(pattern.ident.span());
        }
        let name = pattern.ident.clone();
        let ty = input.ty.as_ref().clone();
        let param_kind = if has_body {
            ParamKind::Body
        } else if has_capture {
            if kind == HandlerKind::Static {
                return Err(Error::new(
                    pattern.ident.span(),
                    "`#[capture]` marks a configured dynamic route capture; static captures are named by the path template",
                ));
            }
            ParamKind::DynamicCapture
        } else if kind == HandlerKind::Static && capture_names.iter().any(|capture| capture == &name.to_string()) {
            ParamKind::Capture
        } else {
            if let Some(span) = named_lifetime_span(&ty) {
                return Err(Error::new(
                    span,
                    "request-parts extractor lifetimes must be elided or use `'_`; an explicit lifetime cannot be tied unambiguously to the request-parts borrow",
                ));
            }
            ParamKind::Parts
        };
        params.push(Param {
            name,
            ty,
            kind: param_kind,
        });
    }
    Ok(params)
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
    let ReturnType::Type(_, response) = &method.sig.output else {
        return Err(Error::new(
            method.sig.output.span(),
            "service handlers must declare an explicit response type",
        ));
    };
    if matches!(response.as_ref(), Type::ImplTrait(_)) {
        return Err(Error::new(
            response.span(),
            "service handler response types cannot use `impl Trait`",
        ));
    }
    Ok(response.as_ref().clone())
}

fn validate_signature(method: &ImplItemFn) -> syn::Result<()> {
    if method.sig.asyncness.is_none() {
        return Err(Error::new(method.sig.fn_token.span(), "service handlers must be async"));
    }
    if method.sig.constness.is_some() || method.sig.unsafety.is_some() || method.sig.abi.is_some() {
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
    if receiver.reference.is_none() || receiver.mutability.is_some() || receiver.colon_token.is_some() {
        return Err(Error::new(receiver.span(), "service handlers must begin with `&self`"));
    }
    Ok(())
}

/// `#[fallback]` and `#[catch]` methods share one by-value argument contract.
fn validate_policy_signature(method: &ImplItemFn) -> syn::Result<()> {
    validate_signature(method)?;
    for input in method.sig.inputs.iter().skip(1) {
        let FnArg::Typed(input) = input else {
            continue;
        };
        if let Some(attribute) = input
            .attrs
            .iter()
            .find(|attr| attr.path().is_ident("body") || attr.path().is_ident("capture"))
        {
            return Err(Error::new(
                attribute.path().span(),
                "routing policy arguments cannot use `#[body]` or `#[capture]`; catchers cannot recursively extract a request",
            ));
        }
        if let Type::Reference(reference) = input.ty.as_ref() {
            return Err(Error::new(
                reference.and_token.span(),
                "routing policy arguments are passed by value",
            ));
        }
    }
    Ok(())
}

fn parse_catcher(method: &ImplItemFn, attribute: &Attribute) -> syn::Result<Catcher> {
    if !method.sig.generics.params.is_empty() {
        return Err(Error::new(
            method.sig.generics.params.span(),
            "routing catcher methods cannot have generic parameters",
        ));
    }
    validate_policy_signature(method)?;

    let tokens: TokenStream2 = attribute.parse_args()?;
    let (rejection_tokens, from) = split_catch_arguments(tokens)?;
    if let Some(span) = lifetime_span_in(&rejection_tokens) {
        return Err(Error::new(span, "catcher rejection types cannot depend on a non-static lifetime"));
    }
    let wildcard = rejection_tokens.to_string().contains("..");
    let rejection_base = base_of_tokens(&rejection_tokens);

    let parameter = method
        .sig
        .inputs
        .iter()
        .skip(1)
        .find_map(|input| match input {
            FnArg::Typed(typed) => Some(typed.ty.as_ref().clone()),
            FnArg::Receiver(_) => None,
        })
        .ok_or_else(|| {
            Error::new(
                method.sig.ident.span(),
                "an extractor catcher takes exactly one by-value rejection argument",
            )
        })?;

    let parameter_base = type_base(&parameter);
    let matches = if wildcard {
        parameter_base == rejection_base
    } else {
        normalize(&parameter.to_token_stream().to_string()) == normalize(&rejection_tokens.to_string())
    };
    if !matches {
        let span = match &parameter {
            Type::Path(path) => path
                .path
                .segments
                .last()
                .map_or_else(|| parameter.span(), |segment| segment.ident.span()),
            other => other.span(),
        };
        return Err(Error::new(
            span,
            "the `#[catch(RejectionType)]` argument must exactly match the catcher's by-value parameter type",
        ));
    }

    Ok(Catcher {
        name: method.sig.ident.clone(),
        attr_span: attribute.path().span(),
        rejection_base,
        from_base: from.as_ref().map(type_base),
        parameter,
        response: response_type(method)?,
    })
}

/// The bounds a streaming transform declares on its transport request body.
fn transport_bounds(method: &ImplItemFn, generic: &Ident) -> Vec<TokenStream2> {
    let mut bounds = Vec::new();
    if let Some(param) = method.sig.generics.type_params().find(|param| &param.ident == generic) {
        for bound in &param.bounds {
            bounds.push(quote::quote! { #bound });
        }
    }
    if let Some(where_clause) = &method.sig.generics.where_clause {
        for predicate in &where_clause.predicates {
            if let syn::WherePredicate::Type(predicate) = predicate
                && *generic == type_base(&predicate.bounded_ty).as_str()
            {
                for bound in &predicate.bounds {
                    bounds.push(quote::quote! { #bound });
                }
            }
        }
    }
    bounds
}

/// A transform's short-circuit response type.
fn transform_short_circuit(response: &Type, consumes: bool) -> Option<Type> {
    let Type::Path(path) = response else {
        return None;
    };
    let segment = path.path.segments.last()?;
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let mut types = arguments.args.iter().filter_map(|argument| match argument {
        GenericArgument::Type(argument) => Some(argument.clone()),
        _ => None,
    });
    if consumes { types.next() } else { types.nth(1) }
}

fn normalize(text: &str) -> String {
    text.chars().filter(|character| !character.is_whitespace()).collect()
}

fn split_catch_arguments(tokens: TokenStream2) -> syn::Result<(TokenStream2, Option<Type>)> {
    struct CatchArgs {
        rejection: TokenStream2,
        from: Option<Type>,
    }

    impl Parse for CatchArgs {
        fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
            let mut rejection = TokenStream2::new();
            let mut depth = 0_i32;
            while !input.is_empty() {
                if depth == 0 && input.peek(Token![,]) {
                    break;
                }
                let tree: proc_macro2::TokenTree = input.parse()?;
                match &tree {
                    proc_macro2::TokenTree::Punct(punct) if punct.as_char() == '<' => depth += 1,
                    proc_macro2::TokenTree::Punct(punct) if punct.as_char() == '>' => depth -= 1,
                    _ => {}
                }
                rejection.extend(core::iter::once(tree));
            }
            let mut from = None;
            if input.peek(Token![,]) {
                let _comma: Token![,] = input.parse()?;
                if !input.is_empty() {
                    let key: Ident = input.parse()?;
                    if key != "from" {
                        return Err(Error::new(key.span(), "expected `from = ExtractorType`"));
                    }
                    let _equals: Token![=] = input.parse()?;
                    from = Some(input.parse::<Type>()?);
                }
            }
            Ok(Self { rejection, from })
        }
    }

    let parsed: CatchArgs = syn::parse2(tokens)?;
    Ok((parsed.rejection, parsed.from))
}

fn lifetime_span_in(tokens: &TokenStream2) -> Option<Span> {
    let mut previous_apostrophe: Option<Span> = None;
    for tree in tokens.clone() {
        match tree {
            proc_macro2::TokenTree::Punct(punct) if punct.as_char() == '\'' => {
                previous_apostrophe = Some(punct.span());
            }
            proc_macro2::TokenTree::Ident(ident) => {
                if let Some(span) = previous_apostrophe.take()
                    && ident != "static"
                {
                    return Some(span.join(ident.span()).unwrap_or(span));
                }
            }
            proc_macro2::TokenTree::Group(group) => {
                previous_apostrophe = None;
                if let Some(span) = lifetime_span_in(&group.stream()) {
                    return Some(span);
                }
            }
            _ => previous_apostrophe = None,
        }
    }
    None
}

fn base_of_tokens(tokens: &TokenStream2) -> String {
    let mut base = String::new();
    let mut depth = 0_i32;
    for tree in tokens.clone() {
        match tree {
            proc_macro2::TokenTree::Punct(punct) if punct.as_char() == '<' => depth += 1,
            proc_macro2::TokenTree::Punct(punct) if punct.as_char() == '>' => depth -= 1,
            proc_macro2::TokenTree::Ident(ident) if depth == 0 => base = ident.to_string(),
            _ => {}
        }
    }
    base
}

fn parse_interceptor(method: &ImplItemFn, attribute: &Attribute) -> syn::Result<Interceptor> {
    if method.sig.asyncness.is_none() {
        return Err(Error::new(method.sig.fn_token.span(), "interceptor methods must be async"));
    }
    let name = attribute
        .path()
        .get_ident()
        .map_or_else(|| String::from("before"), Ident::to_string);
    match name.as_str() {
        "before" => parse_before(method, attribute),
        "after" => parse_after(method, attribute),
        _ => parse_transform(method, attribute),
    }
}

fn interceptor_handlers(attribute: &Attribute) -> syn::Result<Vec<Ident>> {
    if matches!(attribute.meta, syn::Meta::Path(_)) {
        return Ok(Vec::new());
    }
    let names = attribute.parse_args_with(syn::punctuated::Punctuated::<Ident, Token![,]>::parse_terminated)?;
    Ok(names.into_iter().collect())
}

fn parse_before(method: &ImplItemFn, attribute: &Attribute) -> syn::Result<Interceptor> {
    let handlers = interceptor_handlers(attribute)?;
    let expected = if handlers.is_empty() { "BeforeContext" } else { "SelectedContext" };
    let context = method.sig.inputs.iter().nth(1);
    let Some(FnArg::Typed(context)) = context else {
        return Err(Error::new(
            method.sig.ident.span(),
            format!("a `#[before]` interceptor takes `&self` and `&mut {expected}<'_>`"),
        ));
    };
    if type_base(context.ty.as_ref()) != expected {
        let message = if handlers.is_empty() {
            "a router-wide `#[before]` interceptor runs before route resolution and takes `&mut BeforeContext<'_>`, the whole mutable request head. Name handlers (`#[before(handler, ...)]`) to run after route selection instead, taking `&mut SelectedContext<'_>`"
        } else {
            "a per-handler `#[before(handler, ...)]` interceptor runs after route selection, where the request URI backs the selected route's zero-copy captures. It takes `&mut SelectedContext<'_>`, which reads the method, URI, and version and mutates the headers and extensions. Drop the handler list to get a router-wide `#[before]` taking `&mut BeforeContext<'_>`, which runs before resolution and may rewrite the method and URI"
        };
        return Err(Error::new(context.pat.span(), message));
    }
    let ReturnType::Type(_, response) = &method.sig.output else {
        return Err(Error::new(
            method.sig.output.span(),
            "a `#[before]` interceptor must return `Before<R>`",
        ));
    };
    if type_base(response) != "Before" {
        return Err(Error::new(response.span(), "a `#[before]` interceptor must return `Before<R>`"));
    }
    Ok(Interceptor {
        name: method.sig.ident.clone(),
        attr_span: attribute.path().span(),
        kind: InterceptorKind::Before,
        handlers,
        replacement: None,
        short_circuit: first_generic_type(response),
        transport_bounds: Vec::new(),
    })
}

fn parse_after(method: &ImplItemFn, attribute: &Attribute) -> syn::Result<Interceptor> {
    let handlers = interceptor_handlers(attribute)?;
    if let ReturnType::Type(_, response) = &method.sig.output {
        let is_unit = matches!(response.as_ref(), Type::Tuple(tuple) if tuple.elems.is_empty());
        if !is_unit {
            return Err(Error::new(response.span(), "`#[after]` interceptors must return `()`"));
        }
    }
    Ok(Interceptor {
        name: method.sig.ident.clone(),
        attr_span: attribute.path().span(),
        kind: InterceptorKind::After,
        handlers,
        replacement: None,
        short_circuit: None,
        transport_bounds: Vec::new(),
    })
}

struct TransformArgs {
    limit: Option<syn::Expr>,
    limit_span: Option<Span>,
    stream: Option<Span>,
    handlers: Vec<Ident>,
    first_span: Span,
    end_span: Span,
}

impl Parse for TransformArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut args = Self {
            limit: None,
            limit_span: None,
            stream: None,
            handlers: Vec::new(),
            first_span: input.span(),
            end_span: input.span(),
        };
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            if key == "limit" && input.peek(Token![=]) {
                let _equals: Token![=] = input.parse()?;
                args.limit_span = Some(key.span());
                args.limit = Some(input.parse()?);
            } else if key == "stream" {
                args.stream = Some(key.span());
            } else {
                args.handlers.push(key);
            }
            args.end_span = input.span();
            if input.is_empty() {
                break;
            }
            let _comma: Token![,] = input.parse()?;
            if input.is_empty() {
                return Err(Error::new(
                    input.span(),
                    "`#[transform]` must name at least one handler whose body it owns, so unrelated routes are not forced to buffer or wrap",
                ));
            }
        }
        Ok(args)
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the two ownership modes share one validation sequence; splitting it would separate each check from its diagnostic"
)]
fn parse_transform(method: &ImplItemFn, attribute: &Attribute) -> syn::Result<Interceptor> {
    let args: TransformArgs = attribute.parse_args()?;
    if args.limit.is_some() && args.stream.is_some() {
        let span = args.limit_span.unwrap_or(args.first_span);
        return Err(Error::new(
            span,
            "a `#[transform]` interceptor selects exactly one ownership mode; `limit = N` bounded buffering and `stream` wrapping are mutually exclusive",
        ));
    }
    if args.limit.is_none() && args.stream.is_none() {
        let span = args.handlers.first().map_or(args.first_span, Ident::span);
        return Err(Error::new(
            span,
            "`#[transform]` requires one ownership mode followed by at least one handler name: `#[transform(limit = N, handler, ...)]` collects a bounded `bytes::Bytes` buffer, and `#[transform(stream, handler, ...)]` moves the transport body into an interceptor generic over it",
        ));
    }
    if args.handlers.is_empty() {
        return Err(Error::new(
            args.end_span,
            "unexpected end of input, `#[transform]` must name at least one handler whose body it owns, so unrelated routes are not forced to buffer or wrap",
        ));
    }

    let ReturnType::Type(_, response) = &method.sig.output else {
        return Err(Error::new(
            method.sig.output.span(),
            "a `#[transform]` interceptor must return `BodyTransform<B, R>` or `BodyConsumed<R>`",
        ));
    };
    let returns = type_base(response);
    let consumes = match returns.as_str() {
        "BodyTransform" => false,
        "BodyConsumed" => true,
        _ => {
            return Err(Error::new(
                response.span(),
                "a `#[transform]` interceptor must return `BodyTransform<B, R>` or `BodyConsumed<R>`",
            ));
        }
    };

    let body_param = method.sig.inputs.iter().nth(2);
    let Some(FnArg::Typed(body_param)) = body_param else {
        return Err(Error::new(
            method.sig.ident.span(),
            "`#[transform]` interceptors take `&self`, one `&RequestParts`, and the request body",
        ));
    };

    if args.stream.is_some() {
        let generic = method.sig.generics.type_params().next().map(|param| param.ident.clone());
        let Some(generic) = generic else {
            return Err(Error::new(
                method.sig.ident.span(),
                "a streaming `#[transform(stream, ...)]` interceptor must be generic over its transport request body, for example `async fn wrap<B>(&self, parts: &RequestParts, body: B) -> BodyTransform<Wrapper<B>, R> where B: http_body::Body<Data = Bytes>`",
            ));
        };
        if generic != type_base(body_param.ty.as_ref()).as_str() {
            return Err(Error::new(
                body_param.pat.span(),
                "`#[transform(stream, ...)]` interceptors take `&self`, one `&RequestParts`, and the transport request body by value as their generic parameter `B`",
            ));
        }
        if let Type::Path(path) = response.as_ref()
            && let Some(segment) = path.path.segments.last()
            && let PathArguments::AngleBracketed(arguments) = &segment.arguments
        {
            let response_argument = if consumes {
                arguments.args.first()
            } else {
                arguments.args.iter().nth(1)
            };
            if let Some(GenericArgument::Type(response_argument)) = response_argument
                && mentions_ident(response_argument, &generic)
            {
                return Err(Error::new(
                    response_argument.span(),
                    format!(
                        "a streaming `#[transform]` short-circuit response cannot depend on the generic body parameter `{generic}`; name a concrete response type so it can join the generated response body sum"
                    ),
                ));
            }
        }
        return Ok(Interceptor {
            name: method.sig.ident.clone(),
            attr_span: attribute.path().span(),
            kind: InterceptorKind::TransformStream { consumes },
            handlers: args.handlers,
            replacement: stream_replacement(response, consumes, &generic),
            short_circuit: transform_short_circuit(response, consumes),
            transport_bounds: transport_bounds(method, &generic),
        });
    }

    if let Some(param) = method.sig.generics.params.first() {
        return Err(Error::new(
            param.span(),
            "`#[transform]` interceptor methods cannot have generic parameters; only a streaming `#[transform(stream, ...)]` is generic, over its transport request body",
        ));
    }
    if type_base(body_param.ty.as_ref()) != "Bytes" {
        return Err(Error::new(
            body_param.pat.span(),
            "`#[transform(limit = N, ...)]` interceptors take `&self`, one `&RequestParts`, and the collected request body as `bytes::Bytes`",
        ));
    }

    Ok(Interceptor {
        name: method.sig.ident.clone(),
        attr_span: attribute.path().span(),
        kind: InterceptorKind::TransformBuffered {
            limit: args.limit.expect("the buffered mode was just observed"),
            consumes,
        },
        handlers: args.handlers,
        replacement: buffered_replacement(response, consumes),
        short_circuit: transform_short_circuit(response, consumes),
        transport_bounds: Vec::new(),
    })
}

/// The concrete replacement body a buffered transform hands to `#[body]`.
fn buffered_replacement(response: &Type, consumes: bool) -> Option<Type> {
    if consumes {
        return None;
    }
    first_generic_type(response)
}

/// The replacement body of a streaming transform, expressed in the generated
/// transport body parameter instead of the interceptor's own generic.
fn stream_replacement(response: &Type, consumes: bool, generic: &Ident) -> Option<Type> {
    if consumes {
        return None;
    }
    let mut replacement = first_generic_type(response)?;
    rename_ident(&mut replacement, generic, &Ident::new("__RtrB", generic.span()));
    Some(replacement)
}

fn first_generic_type(ty: &Type) -> Option<Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.iter().find_map(|argument| match argument {
        GenericArgument::Type(argument) => Some(argument.clone()),
        _ => None,
    })
}

fn rename_ident(ty: &mut Type, from: &Ident, to: &Ident) {
    struct Rename<'a> {
        from: &'a Ident,
        to: &'a Ident,
    }

    impl syn::visit_mut::VisitMut for Rename<'_> {
        fn visit_ident_mut(&mut self, i: &mut Ident) {
            if i == self.from {
                *i = self.to.clone();
            }
        }
    }

    syn::visit_mut::VisitMut::visit_type_mut(&mut Rename { from, to }, ty);
}

/// Rewrites elided and anonymous lifetimes so an extractor bound can be
/// expressed as a higher-ranked `where` clause.
pub(crate) fn bind_request_lifetime(ty: &Type, lifetime: &syn::Lifetime) -> Type {
    struct Bind<'a>(&'a syn::Lifetime);

    impl syn::visit_mut::VisitMut for Bind<'_> {
        fn visit_type_reference_mut(&mut self, i: &mut syn::TypeReference) {
            if i.lifetime.as_ref().is_none_or(|existing| existing.ident == "_") {
                i.lifetime = Some(self.0.clone());
            }
            syn::visit_mut::visit_type_reference_mut(self, i);
        }

        fn visit_lifetime_mut(&mut self, i: &mut syn::Lifetime) {
            if i.ident == "_" {
                *i = self.0.clone();
            }
        }
    }

    let mut bound = ty.clone();
    syn::visit_mut::VisitMut::visit_type_mut(&mut Bind(lifetime), &mut bound);
    bound
}

fn mentions_ident(ty: &Type, ident: &Ident) -> bool {
    struct Finder<'a> {
        ident: &'a Ident,
        found: bool,
    }

    impl<'ast> syn::visit::Visit<'ast> for Finder<'_> {
        fn visit_ident(&mut self, i: &'ast Ident) {
            if i == self.ident {
                self.found = true;
            }
        }
    }

    let mut finder = Finder { ident, found: false };
    syn::visit::Visit::visit_type(&mut finder, ty);
    finder.found
}

fn validate_catchers(catchers: &[Catcher]) -> syn::Result<()> {
    for (index, catcher) in catchers.iter().enumerate() {
        for previous in &catchers[..index] {
            if previous.rejection_base == catcher.rejection_base && previous.from_base == catcher.from_base {
                return Err(Error::new(
                    catcher.attr_span,
                    format!("duplicate catcher for rejection type `{}`", catcher.rejection_base),
                )
                .tap_combine(Error::new(previous.attr_span, "the first catcher for this type is declared here")));
            }
        }
    }
    Ok(())
}

/// Rejects a catcher that no built-in extractor in the service can reach.
fn validate_catcher_usage(catchers: &[Catcher], handlers: &[Handler]) -> syn::Result<()> {
    for catcher in catchers {
        if catcher.from_base.is_some() {
            continue;
        }
        let used = handlers.iter().flat_map(|handler| &handler.params).any(|param| {
            matches!(param.kind, ParamKind::Parts | ParamKind::Body)
                && builtin_rejection(&type_base(&param.ty)).is_some_and(|rejection| rejection == catcher.rejection_base)
        });
        if !used {
            return Err(Error::new(
                catcher.attr_span,
                "unused extractor catcher: no built-in extractor in this service has that rejection type; custom extractors require `from = ExtractorType`",
            ));
        }
    }
    Ok(())
}

fn validate_interceptors(interceptors: &[Interceptor], handlers: &[Handler]) -> syn::Result<()> {
    let mut transformed: Vec<(String, Span)> = Vec::new();
    for interceptor in interceptors {
        for named in &interceptor.handlers {
            let Some(handler) = handlers.iter().find(|handler| &handler.name == named) else {
                let attribute = match interceptor.kind {
                    InterceptorKind::Before => "before",
                    InterceptorKind::After => "after",
                    _ => "transform",
                };
                return Err(Error::new(
                    named.span(),
                    format!("`#[{attribute}]` names `{named}`, which is not a `#[route]` handler in this service"),
                ));
            };
            match interceptor.kind {
                InterceptorKind::TransformBuffered { consumes, .. } | InterceptorKind::TransformStream { consumes } => {
                    if let Some((_, first)) = transformed.iter().find(|(name, _)| name == &named.to_string()) {
                        return Err(Error::new(
                            named.span(),
                            format!("handler `{named}` is transformed more than once; a request body can be replaced only once"),
                        )
                        .tap_combine(Error::new(*first, "the first transform for this handler is declared here")));
                    }
                    transformed.push((named.to_string(), interceptor.name.span()));
                    if consumes && handler.has_body() {
                        let advice = if matches!(interceptor.kind, InterceptorKind::TransformStream { .. }) {
                            "return `BodyTransform<Wrapper<B>, R>` with a replacement that wraps the transport body for `#[body]` extraction"
                        } else {
                            "return `BodyTransform<B, R>` with a concrete replacement body for `#[body]` extraction"
                        };
                        return Err(Error::new(
                            interceptor.attr_span,
                            format!(
                                "handler `{named}` declares a `#[body]` parameter, but its `#[transform]` consumes the body without a replacement, so there is nothing left to extract; {advice}"
                            ),
                        ));
                    }
                }
                InterceptorKind::Before | InterceptorKind::After => {}
            }
        }
    }
    Ok(())
}
