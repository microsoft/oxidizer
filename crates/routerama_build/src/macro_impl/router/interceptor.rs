// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Parsing and validation of `#[before]`, `#[after]`, and `#[transform]` interceptors.

use alloc::format;
use alloc::string::ToString as _;
use alloc::vec::Vec;

use proc_macro2::Ident;
use quote::quote;
use syn::spanned::Spanned as _;
use syn::visit_mut::VisitMut as _;
use syn::{Attribute, Error, FnArg, GenericArgument, ImplItemFn, PathArguments, ReturnType, Token, Type, WherePredicate};

use super::binding::{mutable_reference_name, shared_reference_name, terminal_type_name};
use super::contract::{has_route_attr, token_stream_contains_ident};
use super::model::{AfterInterceptor, BeforeInterceptor, RoutingPolicy, TransformInterceptor, TransformMode};
use super::parse::{is_parameter_marker, parameter_pattern};
use super::response_body::generated_body_type;

/// Recognizes and validates one `#[before]`, `#[after]`, or `#[transform]` method.
///
/// Returns whether the method was an interceptor.
pub(super) fn parse_interceptor(policy: &mut RoutingPolicy, method: &ImplItemFn) -> syn::Result<bool> {
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

/// The parsed arguments of a `#[transform(limit = N | stream, handler, ...)]` attribute.
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

/// Whether an interceptor may declare a generic transport-body parameter.
///
/// That parameter is the one a streaming `#[transform]` needs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InterceptorGenerics {
    /// No generic parameters at all, so the call stays trivially concrete.
    Forbidden,
    /// Exactly one generic type parameter, naming the transport request body.
    BodyParameter,
}

/// Destructures an interceptor argument `FnArg` that follows the `&self` receiver.
///
/// The receiver is known to have been consumed already.
///
/// `validate_interceptor_common` consumes the receiver via `inputs.next()`
/// before this is called for every remaining `FnArg`; in valid Rust syntax
/// `FnArg::Receiver` (`&self`/`self`) can only be a method's first parameter,
/// so every later parameter is always `FnArg::Typed`. The `Receiver` arm is
/// kept only to avoid a non-exhaustive match.
#[cfg_attr(coverage_nightly, coverage(off))]
fn typed_interceptor_argument(input: &FnArg) -> syn::Result<&syn::PatType> {
    match input {
        FnArg::Typed(argument) => Ok(argument),
        FnArg::Receiver(_) => Err(Error::new(input.span(), "interceptor methods have exactly one `&self` receiver")),
    }
}

/// Validates shared interceptor signature rules.
///
/// Returns the parameters that follow `&self`.
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
        let argument = typed_interceptor_argument(input)?;
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

/// Returns the single generic transport-body parameter of a streaming `#[transform]`.
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

/// Validates a `#[before]` signature and returns its short-circuit response type.
///
/// The context type distinguishes the two scopes, because only a router-wide
/// interceptor owns the whole mutable request head.
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

/// Validates a `#[transform]` signature.
///
/// Returns its replacement body type (if any) and short-circuit response type.
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

/// Splits a `#[transform]` return type into its two components.
///
/// Those are its replacement body (if any) and its short-circuit response type.
fn transform_outcome(response_type: &Type) -> syn::Result<(Option<Type>, Type)> {
    const OUTCOME: &str = "a `#[transform]` interceptor must return `BodyTransform<B, R>` or `BodyConsumed<R>`";

    let Type::Path(path) = response_type else {
        return Err(Error::new(response_type.span(), OUTCOME));
    };
    // A `syn::Path` in type position always has at least one segment (even a
    // single bare identifier parses as one), so `segments.last()` can never
    // be `None` here.
    let segment = path
        .path
        .segments
        .last()
        .expect("a `Type::Path`'s inner path always has at least one segment");
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

/// Rewrites a streaming transform's generic body parameter.
///
/// The parameter becomes the generated transport body type, so the transform's
/// replacement names one concrete extraction input.
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

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use quote::quote;

    use super::*;
    use crate::macro_impl::router::test_support::expand_router;

    #[test]
    fn extract_single_generic_rejects_a_type_that_is_not_the_expected_wrapper() {
        for ty in [
            syn::parse_quote!(NotBefore<u32>),
            syn::parse_quote!(Before),
            syn::parse_quote!(Before<u32, u32>),
            syn::parse_quote!(()),
        ] {
            let rendered = quote! { #ty }.to_string();
            let error = extract_single_generic(&ty, "Before", "must return `Before<R>`")
                .map(|_| ())
                .unwrap_err();
            assert!(error.to_string().contains("must return `Before<R>`"), "{rendered}: {error}");
        }
    }

    #[test]
    fn a_transform_attribute_rejects_an_unrecognized_ownership_mode_keyword() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self, #[body] data: BytesBody<64>) -> StatusCode {
                    response(data)
                }

                #[transform(reverse, create)]
                async fn wrap(&self, parts: &RequestParts, body: Bytes) -> BodyTransform<Body, StatusCode> {
                    wrap(parts, body)
                }
            }
        })
        .expect_err("`reverse` is not a recognized `#[transform]` ownership mode");
        assert!(
            error
                .to_string()
                .contains("requires one ownership mode followed by at least one handler name"),
            "{error}"
        );
    }

    #[test]
    fn a_route_handler_cannot_also_be_an_interceptor() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                #[before]
                async fn home(&self, context: &mut BeforeContext<'_>) -> StatusCode {
                    response(context)
                }
            }
        })
        .expect_err("a route and interceptor cannot combine");
        assert!(
            error
                .to_string()
                .contains("cannot be both a route handler and a `#[before]`, `#[after]`, or `#[transform]` interceptor"),
            "{error}"
        );
    }

    #[test]
    fn an_interceptor_cannot_also_be_a_policy_method() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                #[fallback]
                async fn guard(&self, context: &mut BeforeContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("an interceptor cannot also be a policy method");
        assert!(
            error
                .to_string()
                .contains("cannot also be a `#[fallback]` or `#[catch]` policy method"),
            "{error}"
        );
    }

    #[test]
    fn a_method_cannot_declare_two_interceptor_kinds() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                #[after]
                async fn guard(&self, context: &mut BeforeContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("only one interceptor annotation is permitted per method");
        assert!(
            error
                .to_string()
                .contains("may declare only one `#[before]`, `#[after]`, or `#[transform]` interceptor annotation"),
            "{error}"
        );
    }

    #[test]
    fn an_interceptor_scope_rejects_an_empty_handler_list() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before()]
                async fn guard(&self, context: &mut SelectedContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("an empty interceptor scope is rejected");
        assert!(error.to_string().contains("an interceptor handler list cannot be empty"), "{error}");
    }

    #[test]
    fn an_interceptor_scope_rejects_duplicate_handler_names() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before(home, home)]
                async fn guard(&self, context: &mut SelectedContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("a duplicate handler name in one scope is rejected");
        assert!(
            error.to_string().contains("duplicate handler `home` in interceptor scope"),
            "{error}"
        );
    }

    #[test]
    fn an_interceptor_scope_rejects_a_name_value_form() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before = "home"]
                async fn guard(&self, context: &mut BeforeContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("a name-value interceptor scope is rejected");
        assert!(
            error
                .to_string()
                .contains("an interceptor scope is either empty or a `(handler, ...)` list of handler names"),
            "{error}"
        );
    }

    #[test]
    fn before_names_an_unknown_handler() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before(missing)]
                async fn guard(&self, context: &mut SelectedContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("an unknown before handler is rejected");
        assert!(
            error
                .to_string()
                .contains("`#[before]` names `missing`, which is not a `#[route]` handler in this service"),
            "{error}"
        );
    }

    #[test]
    fn after_names_an_unknown_handler() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[after(missing)]
                async fn observe(&self, context: &mut AfterContext<'_>) -> () {
                    observe(context)
                }
            }
        })
        .expect_err("an unknown after handler is rejected");
        assert!(
            error
                .to_string()
                .contains("`#[after]` names `missing`, which is not a `#[route]` handler in this service"),
            "{error}"
        );
    }

    #[test]
    fn transform_names_an_unknown_handler() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[transform(limit = 16, missing)]
                async fn shrink(&self, parts: &RequestParts, body: Bytes) -> BodyConsumed<StatusCode> {
                    shrink(parts, body)
                }
            }
        })
        .expect_err("an unknown transform handler is rejected");
        assert!(
            error
                .to_string()
                .contains("`#[transform]` names `missing`, which is not a `#[route]` handler in this service"),
            "{error}"
        );
    }

    #[test]
    fn a_handler_transformed_twice_is_rejected() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(limit = 16, create)]
                async fn first(&self, parts: &RequestParts, body: Bytes) -> BodyConsumed<StatusCode> {
                    first(parts, body)
                }

                #[transform(stream, create)]
                async fn second<B>(&self, parts: &RequestParts, body: B) -> BodyConsumed<StatusCode>
                where
                    B: http_body::Body<Data = Bytes>,
                {
                    second(parts, body)
                }
            }
        })
        .expect_err("a handler cannot be transformed twice");
        assert!(
            error
                .to_string()
                .contains("is transformed more than once; a request body can be replaced only once"),
            "{error}"
        );
    }

    #[test]
    fn a_transformed_handler_with_a_body_parameter_requires_a_replacement() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self, #[body] data: BytesBody<64>) -> StatusCode {
                    response(data)
                }

                #[transform(stream, create)]
                async fn wrap<B>(&self, parts: &RequestParts, body: B) -> BodyConsumed<StatusCode>
                where
                    B: http_body::Body<Data = Bytes>,
                {
                    wrap(parts, body)
                }
            }
        })
        .expect_err("a streaming transform without a replacement leaves nothing to extract");
        assert!(
            error.to_string().contains("wraps the transport body for `#[body]` extraction"),
            "{error}"
        );
    }

    // --- transform grammar (`TransformAttr`) ---

    #[test]
    fn a_transform_attribute_requires_a_recognized_leading_keyword() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform()]
                async fn wrap(&self, parts: &RequestParts, body: Bytes) -> BodyConsumed<StatusCode> {
                    wrap(parts, body)
                }
            }
        })
        .expect_err("an empty transform attribute is rejected");
        assert!(
            error
                .to_string()
                .contains("requires one ownership mode followed by at least one handler name"),
            "{error}"
        );
    }

    #[test]
    fn a_transform_attribute_rejects_mixing_stream_and_limit_keywords() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(limit = 16, create, stream)]
                async fn wrap(&self, parts: &RequestParts, body: Bytes) -> BodyConsumed<StatusCode> {
                    wrap(parts, body)
                }
            }
        })
        .expect_err("mixing `limit` and `stream` is rejected");
        assert!(error.to_string().contains("mutually exclusive"), "{error}");
    }

    #[test]
    fn a_transform_attribute_rejects_a_duplicate_handler_name() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(limit = 16, create, create)]
                async fn wrap(&self, parts: &RequestParts, body: Bytes) -> BodyConsumed<StatusCode> {
                    wrap(parts, body)
                }
            }
        })
        .expect_err("a duplicate handler name in a transform scope is rejected");
        assert!(
            error.to_string().contains("duplicate handler `create` in transform scope"),
            "{error}"
        );
    }

    // --- interceptor signature validation ---

    #[test]
    fn an_interceptor_must_be_async() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                fn guard(&self, context: &mut BeforeContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("a synchronous interceptor is rejected");
        assert!(error.to_string().contains("interceptor methods must be async"), "{error}");
    }

    #[test]
    fn an_interceptor_cannot_be_const_unsafe_or_extern() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                async unsafe fn guard(&self, context: &mut BeforeContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("an unsafe interceptor is rejected");
        assert!(
            error.to_string().contains("cannot be const, unsafe, or extern functions"),
            "{error}"
        );
    }

    #[test]
    fn a_non_streaming_interceptor_cannot_declare_generics() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                async fn guard<T>(&self, context: &mut BeforeContext<'_>) -> Before<StatusCode> {
                    guard::<T>(context)
                }
            }
        })
        .expect_err("a non-streaming interceptor cannot be generic");
        assert!(error.to_string().contains("cannot have generic parameters"), "{error}");
    }

    #[test]
    fn an_interceptor_cannot_be_conditionally_compiled() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                #[cfg(test)]
                async fn guard(&self, context: &mut BeforeContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("a conditionally compiled interceptor is rejected");
        assert!(
            error.to_string().contains("interceptor methods cannot be conditionally compiled"),
            "{error}"
        );
    }

    #[test]
    fn an_interceptor_must_begin_with_a_shared_self_receiver() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                async fn guard(&mut self, context: &mut BeforeContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("a mutable self receiver is rejected");
        assert!(error.to_string().contains("interceptor methods must begin with `&self`"), "{error}");
    }

    #[test]
    fn an_interceptor_argument_cannot_use_extraction_markers() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                async fn guard(&self, #[capture] context: &mut BeforeContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("interceptor arguments cannot use extraction markers");
        assert!(
            error
                .to_string()
                .contains("interceptor arguments cannot use `#[body]` or `#[capture]`"),
            "{error}"
        );
    }

    #[test]
    fn a_streaming_transform_requires_exactly_one_generic_body_parameter() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(stream, create)]
                async fn wrap(&self, parts: &RequestParts, body: Bytes) -> BodyConsumed<StatusCode> {
                    wrap(parts, body)
                }
            }
        })
        .expect_err("a streaming transform without a generic body parameter is rejected");
        assert!(
            error.to_string().contains("must be generic over its transport request body"),
            "{error}"
        );
    }

    #[test]
    fn a_streaming_transform_rejects_more_than_one_type_parameter() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(stream, create)]
                async fn wrap<B, C>(&self, parts: &RequestParts, body: B) -> BodyConsumed<StatusCode>
                where
                    B: http_body::Body<Data = Bytes>,
                {
                    wrap::<B, C>(parts, body)
                }
            }
        })
        .expect_err("a streaming transform with two type parameters is rejected");
        assert!(error.to_string().contains("declares exactly one generic parameter"), "{error}");
    }

    #[test]
    fn a_streaming_transform_rejects_a_lifetime_parameter() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(stream, create)]
                async fn wrap<'a, B>(&self, parts: &RequestParts, body: B) -> BodyConsumed<StatusCode>
                where
                    B: http_body::Body<Data = Bytes> + 'a,
                {
                    wrap::<B>(parts, body)
                }
            }
        })
        .expect_err("a streaming transform with a lifetime parameter is rejected");
        assert!(error.to_string().contains("declares exactly one generic parameter"), "{error}");
    }

    #[test]
    fn a_streaming_transform_rejects_a_const_parameter() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(stream, create)]
                async fn wrap<const N: usize, B>(&self, parts: &RequestParts, body: B) -> BodyConsumed<StatusCode>
                where
                    B: http_body::Body<Data = Bytes>,
                {
                    wrap::<N, B>(parts, body)
                }
            }
        })
        .expect_err("a streaming transform with a const parameter is rejected");
        assert!(error.to_string().contains("declares exactly one generic parameter"), "{error}");
    }

    #[test]
    fn an_after_interceptor_rejects_a_non_unit_return_type() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[after]
                async fn observe(&self, context: &mut AfterContext<'_>) -> StatusCode {
                    observe(context)
                }
            }
        })
        .expect_err("a non-unit `#[after]` return type is rejected");
        assert!(error.to_string().contains("interceptors must return `()`"), "{error}");
    }

    #[test]
    fn an_after_interceptor_rejects_the_wrong_argument_type() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[after]
                async fn observe(&self, context: &mut BeforeContext<'_>) -> () {
                    observe(context)
                }
            }
        })
        .expect_err("an `#[after]` interceptor requires an `AfterContext` argument");
        assert!(
            error
                .to_string()
                .contains("`#[after]` interceptors take exactly `&self` and one `&mut AfterContext<'_>` argument"),
            "{error}"
        );
    }

    #[test]
    fn a_before_interceptor_rejects_the_wrong_context_type() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                async fn guard(&self, context: &mut AfterContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("a router-wide before requires a `BeforeContext` argument");
        assert!(
            error
                .to_string()
                .contains("takes `&mut BeforeContext<'_>`, the whole mutable request head"),
            "{error}"
        );
    }

    #[test]
    fn a_per_handler_before_rejects_the_wrong_context_type() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before(home)]
                async fn guard(&self, context: &mut BeforeContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("a per-handler before requires a `SelectedContext` argument");
        assert!(
            error.to_string().contains("runs after route selection, where the request URI"),
            "{error}"
        );
    }

    #[test]
    fn a_transform_parts_argument_must_be_shared_request_parts() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(limit = 16, create)]
                async fn shrink(&self, parts: &mut RequestParts, body: Bytes) -> BodyConsumed<StatusCode> {
                    shrink(parts, body)
                }
            }
        })
        .expect_err("a mutably-borrowed parts argument is rejected");
        assert!(
            error
                .to_string()
                .contains("interceptors take `&self`, one `&RequestParts`, and the collected request"),
            "{error}"
        );
    }

    #[test]
    fn a_buffered_transform_body_argument_must_be_bytes() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(limit = 16, create)]
                async fn shrink(&self, parts: &RequestParts, body: Vec<u8>) -> BodyConsumed<StatusCode> {
                    shrink(parts, body)
                }
            }
        })
        .expect_err("a non-`Bytes` buffered transform body argument is rejected");
        assert!(error.to_string().contains("the collected request"), "{error}");
    }

    #[test]
    fn a_streaming_transform_body_argument_must_match_the_generic_parameter() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(stream, create)]
                async fn wrap<B>(&self, parts: &RequestParts, body: Vec<u8>) -> BodyConsumed<StatusCode>
                where
                    B: http_body::Body<Data = Bytes>,
                {
                    wrap(parts, body)
                }
            }
        })
        .expect_err("a streaming transform body argument that isn't the generic parameter is rejected");
        assert!(
            error
                .to_string()
                .contains("the transport request body by value as their generic parameter"),
            "{error}"
        );
    }

    #[test]
    fn a_streaming_transform_short_circuit_cannot_depend_on_the_body_parameter() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(stream, create)]
                async fn wrap<B>(&self, parts: &RequestParts, body: B) -> BodyTransform<B, B>
                where
                    B: http_body::Body<Data = Bytes>,
                {
                    wrap(parts, body)
                }
            }
        })
        .expect_err("a short-circuit response depending on the generic body parameter is rejected");
        assert!(error.to_string().contains("cannot depend on the generic body parameter"), "{error}");
    }

    #[test]
    fn transform_outcome_rejects_a_non_wrapper_return_type() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(limit = 16, create)]
                async fn shrink(&self, parts: &RequestParts, body: Bytes) -> StatusCode {
                    shrink(parts, body)
                }
            }
        })
        .expect_err("a transform must return `BodyTransform` or `BodyConsumed`");
        assert!(
            error.to_string().contains("must return `BodyTransform<B, R>` or `BodyConsumed<R>`"),
            "{error}"
        );
    }

    #[test]
    fn transform_outcome_rejects_the_wrong_number_of_type_arguments() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(limit = 16, create)]
                async fn shrink(&self, parts: &RequestParts, body: Bytes) -> BodyConsumed<StatusCode, StatusCode> {
                    shrink(parts, body)
                }
            }
        })
        .expect_err("`BodyConsumed` with two type arguments is rejected");
        assert!(
            error.to_string().contains("must return `BodyTransform<B, R>` or `BodyConsumed<R>`"),
            "{error}"
        );
    }

    #[test]
    fn an_interceptor_must_declare_an_explicit_return_type() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                async fn guard(&self, context: &mut BeforeContext<'_>) {
                    guard(context)
                }
            }
        })
        .expect_err("an interceptor without an explicit return type is rejected");
        assert!(
            error.to_string().contains("interceptors must declare an explicit return type"),
            "{error}"
        );
    }

    #[test]
    fn an_interceptor_return_type_cannot_use_impl_trait() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                async fn guard(&self, context: &mut BeforeContext<'_>) -> impl core::fmt::Debug {
                    guard(context)
                }
            }
        })
        .expect_err("`impl Trait` interceptor returns are rejected");
        assert!(
            error.to_string().contains("interceptor return types cannot use `impl Trait`"),
            "{error}"
        );
    }

    #[test]
    fn a_before_interceptor_return_type_must_wrap_before() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                async fn guard(&self, context: &mut BeforeContext<'_>) -> StatusCode {
                    guard(context)
                }
            }
        })
        .expect_err("a `#[before]` must return `Before<R>`");
        assert!(error.to_string().contains("must return `Before<R>`"), "{error}");
    }

    // --- routing policy signature validation ---

    #[test]
    fn an_interceptor_without_any_arguments_is_rejected() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                async fn guard() -> Before<StatusCode> {
                    guard()
                }
            }
        })
        .expect_err("an interceptor without any `&self` receiver is rejected");
        assert!(error.to_string().contains("interceptor methods must begin with `&self`"), "{error}");
    }

    #[test]
    fn a_before_interceptor_requires_exactly_one_context_argument() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                async fn guard(&self) -> Before<StatusCode> {
                    guard()
                }
            }
        })
        .expect_err("a router-wide before with no context argument is rejected");
        assert!(error.to_string().contains("takes `&mut BeforeContext<'_>`"), "{error}");
    }

    #[test]
    fn an_after_interceptor_requires_exactly_one_context_argument() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[after]
                async fn observe(&self) {
                    observe()
                }
            }
        })
        .expect_err("an `#[after]` interceptor with no context argument is rejected");
        assert!(
            error
                .to_string()
                .contains("`#[after]` interceptors take exactly `&self` and one `&mut AfterContext<'_>` argument"),
            "{error}"
        );
    }

    // --- routing policy method attribute conflicts ---

    #[test]
    fn a_transform_attribute_requires_at_least_one_handler() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self, #[body] data: BytesBody<64>) -> StatusCode {
                    response(data)
                }

                #[transform(stream)]
                async fn wrap<B>(&self, parts: &RequestParts, body: B) -> BodyConsumed<StatusCode>
                where
                    B: http_body::Body<Data = Bytes>,
                {
                    wrap(parts, body)
                }
            }
        })
        .expect_err("a `#[transform]` without any named handlers is rejected");
        assert!(error.to_string().contains("must name at least one handler"), "{error}");
    }

    // --- reference-name helpers on non-reference argument types ---

    #[test]
    fn a_before_interceptor_context_argument_must_be_a_mutable_reference() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                async fn guard(&self, context: BeforeContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect_err("an owned (non-reference) context argument is rejected");
        assert!(error.to_string().contains("takes `&mut BeforeContext<'_>`"), "{error}");
    }

    #[test]
    fn a_transform_parts_argument_must_be_a_shared_reference() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(limit = 16, create)]
                async fn shrink(&self, parts: RequestParts, body: Bytes) -> BodyConsumed<StatusCode> {
                    shrink(parts, body)
                }
            }
        })
        .expect_err("an owned (non-reference) parts argument is rejected");
        assert!(
            error
                .to_string()
                .contains("interceptors take `&self`, one `&RequestParts`, and the collected request"),
            "{error}"
        );
    }

    // --- transform argument count and outcome edge cases ---

    #[test]
    fn a_transform_interceptor_requires_exactly_two_arguments() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(limit = 16, create)]
                async fn shrink(&self, parts: &RequestParts) -> BodyConsumed<StatusCode> {
                    shrink(parts)
                }
            }
        })
        .expect_err("a transform with only one argument is rejected");
        assert!(
            error
                .to_string()
                .contains("interceptors take `&self`, one `&RequestParts`, and the collected request"),
            "{error}"
        );
    }

    #[test]
    fn transform_outcome_ignores_a_leading_lifetime_generic_argument() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self, #[body] data: Bytes) -> StatusCode {
                    response(data)
                }

                #[transform(limit = 16, create)]
                async fn shrink(&self, parts: &RequestParts, body: Bytes) -> BodyTransform<'_, Bytes, StatusCode> {
                    shrink(parts, body)
                }
            }
        })
        .expect("a leading lifetime argument in `BodyTransform<'_, B, R>` is ignored");
        let code = generated.to_string();
        assert!(code.contains("BodyTransform :: Replace"), "{code}");
    }

    #[test]
    fn extract_single_generic_ignores_a_leading_lifetime_argument() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                async fn guard(&self, context: &mut BeforeContext<'_>) -> Before<'_, StatusCode> {
                    guard(context)
                }
            }
        })
        .expect("a leading lifetime argument in `Before<'_, R>` is ignored");
        let code = generated.to_string();
        assert!(code.contains("Before :: Next"), "{code}");
    }

    // --- validate_policy_signature receiver-shape edge cases ---

    #[test]
    fn a_buffered_transform_that_consumes_the_body_without_a_replacement_is_rejected() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self, #[body] data: Bytes) -> StatusCode {
                    response(data)
                }

                #[transform(limit = 16, create)]
                async fn shrink(&self, parts: &RequestParts, body: Bytes) -> BodyConsumed<StatusCode> {
                    shrink(parts, body)
                }
            }
        })
        .expect_err("a buffered transform without a replacement body for a `#[body]` handler is rejected");
        assert!(
            error
                .to_string()
                .contains("return `BodyTransform<B, R>` with a concrete replacement body for `#[body]` extraction"),
            "{error}"
        );
    }

    // --- build_dispatch_arms dispatch-merging edge cases ---

    #[test]
    fn a_transform_attribute_with_a_trailing_comma_and_no_handlers_requires_at_least_one_handler() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(limit = 16,)]
                async fn shrink(&self, parts: &RequestParts, body: Bytes) -> BodyConsumed<StatusCode> {
                    shrink(parts, body)
                }
            }
        })
        .expect_err("a transform scope with a trailing comma and no handler names is rejected");
        assert!(
            error.to_string().contains("must name at least one handler whose body it owns"),
            "{error}"
        );
    }

    // --- transform_outcome non-path return type ---

    #[test]
    fn a_transform_interceptor_returning_a_non_path_type_is_rejected() {
        let error = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(limit = 16, create)]
                async fn shrink(&self, parts: &RequestParts, body: Bytes) -> (StatusCode, StatusCode) {
                    shrink(parts, body)
                }
            }
        })
        .expect_err("a transform interceptor returning a tuple type is rejected");
        assert!(
            error
                .to_string()
                .contains("a `#[transform]` interceptor must return `BodyTransform<B, R>` or `BodyConsumed<R>`"),
            "{error}"
        );
    }

    // --- route_contract's heterogeneous_data erased-rejection data bound, under generic state ---
}
