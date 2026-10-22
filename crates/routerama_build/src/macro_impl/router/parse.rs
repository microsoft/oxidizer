// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Attribute parsing and handler/interceptor signature validation.

use alloc::string::{String, ToString as _};
use alloc::vec::Vec;
use alloc::{format, vec};

use http_path_template::{Grammar, PathTemplate};
use proc_macro2::{Ident, Span};
use syn::spanned::Spanned as _;
use syn::{Attribute, Error, FnArg, GenericArgument, ImplItem, ImplItemFn, ItemImpl, LitStr, Pat, PathArguments, ReturnType, Type};

use super::binding::static_route_target;
use super::contract::{fmt_names, variant_name};
use super::model::{Argument, Handler, HandlerKind};
use crate::macro_impl::{
    RouteAttr, RouteDeclaration, RoutePredicates, RouteTarget, StaticHeader, differing_static_header, route_declaration,
    same_static_headers,
};
use crate::route_field_name;
use crate::trie::{capture_field_names, depth_limit_error};

pub(super) fn validate_generated_method_names(
    item: &ItemImpl,
    has_dynamic: bool,
    erased_mounts: bool,
    tower_adapter: bool,
) -> syn::Result<()> {
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

pub(super) fn validate_impl(item: &ItemImpl) -> syn::Result<()> {
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

pub(super) fn service_name(self_ty: &Type) -> syn::Result<Ident> {
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

pub(super) fn parse_handler(method: &ImplItemFn) -> syn::Result<Handler> {
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

/// Classifies a handler's `#[route]` attributes as static or dynamic.
///
/// `route_kind` is only called by `parse_handler` on a `route_attrs` slice
/// that `has_route_attr` already proved non-empty (`parse_handler` is only
/// invoked for methods where it returned `true`, and it filters the exact
/// same `#[route]` predicate that `route_declaration` uses). `route_declaration`
/// only returns `None` when its input slice is empty, so the `None` arm below
/// can never run here.
#[cfg_attr(coverage_nightly, coverage(off))]
fn route_kind(route_attrs: &[Attribute]) -> syn::Result<HandlerKind> {
    match route_declaration(route_attrs)? {
        Some(RouteDeclaration::Static) => Ok(HandlerKind::Static),
        Some(RouteDeclaration::Dynamic) => Ok(HandlerKind::Dynamic),
        None => Err(Error::new(Span::call_site(), "service handler is missing a route declaration")),
    }
}

/// Collects the `host`, `consumes`, and `produces` predicates a handler declares.
///
/// `route_kind` only returns `HandlerKind::Static` when `route_declaration` has
/// already confirmed none of `route_attrs` parses to `RouteTarget::Dynamic` (a
/// lone or combined dynamic attribute either yields `HandlerKind::Dynamic` or
/// is rejected outright), so the `RouteTarget::Dynamic` arm below can never be
/// taken when `kind == HandlerKind::Static`.
#[cfg_attr(coverage_nightly, coverage(off))]
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

/// Collects the static headers a handler's `#[route]` attributes declare.
///
/// Shares `route_predicates`'s unreachability reasoning: `kind ==
/// HandlerKind::Static` guarantees none of `route_attrs` parses to
/// `RouteTarget::Dynamic`, so the `continue` below is never taken.
#[cfg_attr(coverage_nightly, coverage(off))]
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

/// Guards a non-receiver handler parameter.
///
/// It matches every `FnArg` variant `syn` can hand back for a position past
/// index 0.
///
/// `syn`'s `Signature` parser rejects a second receiver outright ("unexpected
/// second method receiver"), so `FnArg::Receiver` can never appear at any
/// position other than index 0, which `handler_parameters` already skips via
/// `.skip(1)`. The `FnArg::Receiver` arm below is therefore unreachable and
/// kept only so this helper stays total over every `FnArg` variant.
#[cfg_attr(coverage_nightly, coverage(off))]
fn typed_handler_argument(input: &FnArg) -> syn::Result<&syn::PatType> {
    let FnArg::Typed(input) = input else {
        return Err(Error::new(input.span(), "service handlers must have exactly one `&self` receiver"));
    };
    Ok(input)
}

fn handler_parameters(method: &ImplItemFn, kind: HandlerKind, capture_names: &[String]) -> syn::Result<HandlerParameters> {
    let mut captures = Vec::new();
    let mut arguments = Vec::new();
    let mut borrows_path = false;
    let mut body_span = None;

    for input in method.sig.inputs.iter().skip(1) {
        let input = typed_handler_argument(input)?;
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

pub(super) fn is_parameter_marker(attribute: &Attribute) -> bool {
    attribute.path().is_ident("body") || attribute.path().is_ident("capture")
}

pub(super) fn parameter_pattern(pattern: &Pat) -> syn::Result<&syn::PatIdent> {
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

pub(super) fn response_type(method: &ImplItemFn) -> syn::Result<Type> {
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
        let (_, path) = static_route_target(target, attribute.span(), "static route aliases must declare a method and path")?;
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

pub(super) fn validate_handlers(handlers: &[Handler]) -> syn::Result<()> {
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

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use quote::quote;
    use syn::parse_quote;

    use crate::macro_impl::router::test_support::{expand_fixed_mounted_router, expand_fixed_router, expand_router};

    #[test]
    fn a_method_cannot_be_both_a_route_handler_and_a_routing_policy_method() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                #[fallback]
                async fn home(&self, failure: RouteFailure<'_>) -> StatusCode {
                    response(failure)
                }
            }
        })
        .expect_err("a method cannot carry both a route and a routing-policy attribute");
        assert!(
            error
                .to_string()
                .contains("a method cannot be both a route handler and a routing policy method"),
            "{error}"
        );
    }

    #[test]
    fn route_and_routing_policy_attributes_may_annotate_methods_only() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response(())
                }

                #[fallback]
                const VERSION: u32 = 1;
            }
        })
        .expect_err("a `#[fallback]` attribute on a non-`fn` impl item is rejected");
        assert!(
            error
                .to_string()
                .contains("route and routing-policy attributes may annotate methods only"),
            "{error}"
        );
    }

    #[test]
    fn erased_mounts_reject_a_conflicting_route_with_erased_mounts_method() {
        let error = expand_fixed_mounted_router(
            syn::parse_quote!(AppState),
            quote! {
                impl Api {
                    fn route_with_erased_mounts(&self) {}

                    #[route(GET, "/")]
                    async fn home(&self) -> StatusCode {
                        StatusCode::NO_CONTENT
                    }
                }
            },
        )
        .expect_err("erased-mount routers cannot generate `route_with_erased_mounts` twice");
        assert!(error.to_string().contains("cannot generate `route_with_erased_mounts`"), "{error}");
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
    fn validate_impl_rejects_a_trait_impl() {
        let error = expand_router(quote! {
            impl SomeTrait for Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }
            }
        })
        .expect_err("a trait impl is rejected");
        assert!(error.to_string().contains("requires an inherent impl"), "{error}");
    }

    #[test]
    fn validate_impl_rejects_generic_impl_blocks() {
        let error = expand_router(quote! {
            impl<T> Api<T> {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }
            }
        })
        .expect_err("a generic impl block is rejected");
        assert!(error.to_string().contains("does not yet support generic impl blocks"), "{error}");
    }

    #[test]
    fn validate_impl_rejects_unsafe_impl_blocks() {
        let error = expand_router(quote! {
            unsafe impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }
            }
        })
        .expect_err("an unsafe impl block is rejected");
        assert!(error.to_string().contains("does not support unsafe impl blocks"), "{error}");
    }

    #[test]
    fn validate_impl_rejects_conditional_compilation_on_the_impl_block() {
        let error = expand_router(quote! {
            #[cfg(test)]
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }
            }
        })
        .expect_err("a `#[cfg]` impl block is rejected");
        assert!(
            error
                .to_string()
                .contains("does not support conditional compilation on the impl block"),
            "{error}"
        );
    }

    #[test]
    fn service_name_rejects_a_generic_service_type() {
        let error = expand_fixed_router(
            parse_quote!(AppState),
            quote! {
                impl Api<T> {
                    #[route(GET, "/")]
                    async fn home(&self) -> StatusCode {
                        response()
                    }
                }
            },
        )
        .expect_err("a generic service type is rejected");
        assert!(error.to_string().contains("does not yet support generic service types"), "{error}");
    }

    // --- fallback / catch policy parsing ---

    #[test]
    fn a_handler_parameter_cannot_be_both_body_and_capture() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books/{id}")]
                async fn book(&self, #[body] #[capture] id: u32) -> StatusCode {
                    book(id)
                }
            }
        })
        .expect_err("a parameter cannot be both #[body] and #[capture]");
        assert!(error.to_string().contains("cannot be both `#[body]` and `#[capture]`"), "{error}");
    }

    #[test]
    fn a_static_path_capture_cannot_also_consume_the_body() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books/{id}")]
                async fn book(&self, #[body] id: BytesBody<1024>) -> StatusCode {
                    book(id)
                }
            }
        })
        .expect_err("a static path capture cannot also consume the body");
        assert!(error.to_string().contains("cannot also consume the request body"), "{error}");
    }

    #[test]
    fn only_one_body_parameter_is_allowed() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, #[body] first: BytesBody<1024>, #[body] second: BytesBody<1024>) -> StatusCode {
                    home(first, second)
                }
            }
        })
        .expect_err("only one `#[body]` parameter is allowed");
        assert!(error.to_string().contains("at most one `#[body]` parameter"), "{error}");
    }

    #[test]
    fn a_body_parameter_must_be_owned() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, #[body] body: &BytesBody<1024>) -> StatusCode {
                    home(body)
                }
            }
        })
        .expect_err("a `#[body]` parameter must be owned");
        assert!(error.to_string().contains("must own their extracted value"), "{error}");
    }

    #[test]
    fn a_dynamic_capture_marker_is_rejected_on_a_static_route() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books")]
                async fn book(&self, #[capture] id: u32) -> StatusCode {
                    book(id)
                }
            }
        })
        .expect_err("a `#[capture]` parameter must exist in the static route template");
        assert!(error.to_string().contains("is absent from this static route template"), "{error}");
    }

    #[test]
    fn a_dynamic_capture_parameter_must_be_owned() {
        let error = expand_router(quote! {
            impl Api {
                #[route(dynamic)]
                async fn dyn_handler(&self, #[capture] id: &str) -> StatusCode {
                    dyn_handler(id)
                }
            }
        })
        .expect_err("a dynamic `#[capture]` parameter must be owned");
        assert!(
            error.to_string().contains("dynamic `#[capture]` parameters must be owned"),
            "{error}"
        );
    }

    #[test]
    fn declared_captures_must_match_the_path_captures() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books/{id}")]
                async fn book(&self, other: u32) -> StatusCode {
                    book(other)
                }
            }
        })
        .expect_err("capture parameters must match the path captures");
        assert!(error.to_string().contains("do not match its path captures"), "{error}");
    }

    #[test]
    fn a_handler_parameter_marker_does_not_accept_arguments() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books/{id}")]
                async fn book(&self, #[capture(foo)] id: u32) -> StatusCode {
                    book(id)
                }
            }
        })
        .expect_err("a marker attribute does not accept arguments");
        assert!(error.to_string().contains("do not accept arguments"), "{error}");
    }

    #[test]
    fn a_handler_parameter_cannot_declare_a_marker_twice() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books/{id}")]
                async fn book(&self, #[capture] #[capture] id: u32) -> StatusCode {
                    book(id)
                }
            }
        })
        .expect_err("a duplicate marker is rejected");
        assert!(error.to_string().contains("duplicate handler parameter marker"), "{error}");
    }

    #[test]
    fn a_handler_parameter_pattern_must_be_a_simple_identifier() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, (a, b): (u32, u32)) -> StatusCode {
                    home(a, b)
                }
            }
        })
        .expect_err("a tuple pattern is rejected");
        assert!(error.to_string().contains("simple identifier patterns"), "{error}");
    }

    #[test]
    fn a_handler_parameter_pattern_cannot_use_ref_or_subpat() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, ref state @ good: Query) -> StatusCode {
                    home(state)
                }
            }
        })
        .expect_err("a subpattern is rejected");
        assert!(error.to_string().contains("simple identifier patterns"), "{error}");
    }

    #[test]
    fn a_handler_must_declare_an_explicit_response_type() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) {
                    home()
                }
            }
        })
        .expect_err("a missing response type is rejected");
        assert!(error.to_string().contains("must declare an explicit response type"), "{error}");
    }

    #[test]
    fn a_handler_response_type_cannot_use_impl_trait() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> impl IntoResponse {
                    home()
                }
            }
        })
        .expect_err("`impl Trait` response types are rejected");
        assert!(error.to_string().contains("cannot use `impl Trait`"), "{error}");
    }

    #[test]
    fn a_handler_must_be_async() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                fn home(&self) -> StatusCode {
                    home()
                }
            }
        })
        .expect_err("a non-async handler is rejected");
        assert!(error.to_string().contains("must be async"), "{error}");
    }

    #[test]
    fn a_handler_cannot_be_const_unsafe_or_extern() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                const async unsafe extern "C" fn home(&self) -> StatusCode {
                    home()
                }
            }
        })
        .expect_err("a const/unsafe/extern handler is rejected");
        assert!(error.to_string().contains("cannot be const, unsafe, or extern"), "{error}");
    }

    #[test]
    fn a_handler_cannot_have_generic_parameters() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home<T>(&self) -> StatusCode {
                    home::<T>()
                }
            }
        })
        .expect_err("a generic handler is rejected");
        assert!(error.to_string().contains("cannot have generic parameters"), "{error}");
    }

    #[test]
    fn a_handler_must_begin_with_a_shared_self_receiver() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home() -> StatusCode {
                    home()
                }
            }
        })
        .expect_err("a handler without a `&self` receiver is rejected");
        assert!(error.to_string().contains("must begin with `&self`"), "{error}");
    }

    #[test]
    fn a_handler_self_receiver_cannot_be_marked() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(#[capture] &self) -> StatusCode {
                    home()
                }
            }
        })
        .expect_err("a marked self receiver is rejected");
        assert!(error.to_string().contains("may annotate typed handler parameters only"), "{error}");
    }

    #[test]
    fn a_handler_self_receiver_cannot_be_mutable() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&mut self) -> StatusCode {
                    home()
                }
            }
        })
        .expect_err("a `&mut self` receiver is rejected");
        assert!(error.to_string().contains("must begin with `&self`"), "{error}");
    }

    #[test]
    fn a_handler_parameter_cannot_use_impl_trait() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, extractor: impl core::fmt::Debug) -> StatusCode {
                    home(extractor)
                }
            }
        })
        .expect_err("an `impl Trait` parameter is rejected");
        assert!(error.to_string().contains("cannot use `impl Trait`"), "{error}");
    }

    // --- route capture / predicate / static header alias mismatches ---

    #[test]
    fn route_predicates_must_match_across_aliases() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books", host = "a.example.com")]
                #[route(GET, "/tomes", host = "b.example.com")]
                async fn book(&self) -> StatusCode {
                    book()
                }
            }
        })
        .expect_err("differing predicates across aliases are rejected");
        assert!(
            error
                .to_string()
                .contains("identical `host`, `consumes`, and `produces` predicates"),
            "{error}"
        );
    }

    #[test]
    fn route_static_headers_must_match_across_aliases() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books", headers(insert("x-a", "1")))]
                #[route(GET, "/tomes", headers(insert("x-b", "2")))]
                async fn book(&self) -> StatusCode {
                    book()
                }
            }
        })
        .expect_err("differing static headers across aliases are rejected");
        assert!(error.to_string().contains("identical static response-header operations"), "{error}");
    }

    #[test]
    fn route_capture_names_must_match_across_aliases() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books/{id}")]
                #[route(POST, "/tomes/{slug}")]
                async fn book(&self, id: u32) -> StatusCode {
                    book(id)
                }
            }
        })
        .expect_err("differing captures across aliases are rejected");
        assert!(error.to_string().contains("must capture the same path variables"), "{error}");
    }

    // --- capture_type ---

    #[test]
    fn a_borrowed_string_capture_cannot_be_mutable() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books/{id}")]
                async fn book(&self, id: &mut str) -> StatusCode {
                    book(id)
                }
            }
        })
        .expect_err("a `&mut str` capture is rejected");
        assert!(error.to_string().contains("borrowed string captures must use `&str`"), "{error}");
    }

    #[test]
    fn a_borrowed_string_capture_cannot_use_a_named_lifetime() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books/{id}")]
                async fn book(&self, id: &'static str) -> StatusCode {
                    book(id)
                }
            }
        })
        .expect_err("a named lifetime on a `&str` capture is rejected");
        assert!(error.to_string().contains("borrowed string captures must use `&str`"), "{error}");
    }

    #[test]
    fn a_borrowed_cow_capture_must_use_the_elided_lifetime() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books/{id}")]
                async fn book(&self, id: Cow<str>) -> StatusCode {
                    book(id)
                }
            }
        })
        .expect_err("a `Cow` without a lifetime argument is rejected");
        assert!(
            error.to_string().contains("borrowed `Cow` captures must use `Cow<'_, str>`"),
            "{error}"
        );
    }

    #[test]
    fn a_borrowed_cow_capture_cannot_use_a_named_lifetime() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books/{id}")]
                async fn book(&self, id: Cow<'static, str>) -> StatusCode {
                    book(id)
                }
            }
        })
        .expect_err("a named lifetime on a `Cow` capture is rejected");
        assert!(
            error.to_string().contains("borrowed `Cow` captures must use `Cow<'_, str>`"),
            "{error}"
        );
    }

    #[test]
    fn borrowed_str_and_cow_captures_are_accepted() {
        expand_router(quote! {
            impl Api {
                #[route(GET, "/books/{id}/{slug}")]
                async fn book(&self, id: &str, slug: Cow<'_, str>) -> StatusCode {
                    book(id, slug)
                }
            }
        })
        .expect("borrowed `&str` and `Cow<'_, str>` captures are accepted");
    }

    #[test]
    fn duplicate_handler_variant_names_are_rejected() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/a")]
                async fn book(&self) -> StatusCode {
                    book()
                }

                #[route(GET, "/b")]
                async fn Book(&self) -> StatusCode {
                    book()
                }
            }
        })
        .expect_err("a duplicate route variant is rejected");
        assert!(error.to_string().contains("generate the duplicate route variant"), "{error}");
    }

    // --- overlapping static route validation ---

    #[test]
    fn service_name_requires_a_named_path_self_type() {
        let error = expand_router(quote! {
            impl (u32, u32) {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }
            }
        })
        .expect_err("a tuple self type is rejected");
        assert!(error.to_string().contains("requires a named service type"), "{error}");
    }

    #[test]
    fn a_route_handler_cannot_be_conditionally_compiled() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                #[cfg(test)]
                async fn home(&self) -> StatusCode {
                    home()
                }
            }
        })
        .expect_err("a conditionally compiled route handler is rejected");
        assert!(
            error
                .to_string()
                .contains("`#[router]` does not support conditionally compiled route handlers"),
            "{error}"
        );
    }

    #[test]
    fn a_dynamic_route_rejects_an_explicit_priority() {
        let error = expand_router(quote! {
            impl Api {
                #[route(dynamic, priority = 1)]
                async fn home(&self) -> StatusCode {
                    home()
                }
            }
        })
        .expect_err("a dynamic route with an explicit priority is rejected");
        assert!(error.to_string().contains("configured dynamic routes reject `priority`"), "{error}");
    }

    // --- route_predicates / route_static_headers dynamic-skip edge cases ---
    //
    // NOTE: `handler_parameters`'s `let FnArg::Typed(input) = input else { .. }` guard
    // (router.rs ~4991-4992) defends against a second `FnArg::Receiver` appearing after
    // the first handler argument. `syn` itself rejects every spelling of a second
    // receiver (`&self`, `mut self`, `self: Box<Self>`) at `Signature` parse time with
    // "unexpected second method receiver", so this branch is unreachable from any
    // syntactically valid input and cannot be exercised by a test here.

    // --- token_stream_contains_ident group recursion (tuple response type under fixed state) ---
}
