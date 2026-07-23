// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementation of `#[service]`.

use alloc::format;
use alloc::string::{String, ToString as _};
use alloc::vec::Vec;

use http_path_template::{Grammar, PathTemplate};
use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::{ToTokens as _, format_ident, quote};
use syn::spanned::Spanned as _;
use syn::{Attribute, Error, FnArg, GenericArgument, ImplItem, ImplItemFn, ItemEnum, ItemImpl, Pat, PathArguments, ReturnType, Type};

use super::{RouteAttr, resolver};
use crate::route_field_name;
use crate::trie::capture_field_names;

struct Handler {
    method: Ident,
    variant: Ident,
    kind: HandlerKind,
    route_attrs: Vec<Attribute>,
    captures: Vec<(Ident, Type)>,
    arguments: Vec<Argument>,
    context_type: Type,
    response_type: Type,
    borrows_path: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HandlerKind {
    Static,
    Dynamic,
}

enum Argument {
    Capture(Ident),
    Context,
}

pub(crate) fn expand(mut item: ItemImpl, context_first: bool) -> syn::Result<TokenStream2> {
    validate_impl(&item)?;
    let service_name = service_name(&item.self_ty)?;
    let route_name = format_ident!("__{}Route", service_name, span = service_name.span());

    let mut handlers = Vec::new();
    for impl_item in &item.items {
        match impl_item {
            ImplItem::Fn(method) if has_route_attr(&method.attrs) => {
                handlers.push(parse_handler(method, context_first)?);
            }
            other if has_route_attr(other.attrs()) => {
                return Err(Error::new(other.span(), "`#[route]` may only annotate async service methods"));
            }
            _ => {}
        }
    }
    if handlers.is_empty() {
        return Err(Error::new(
            item.impl_token.span(),
            "`#[service]` requires at least one `#[route]` handler",
        ));
    }
    validate_handlers(&handlers)?;
    let has_dynamic = handlers.iter().any(|handler| handler.kind == HandlerKind::Dynamic);
    validate_generated_method_names(&item, has_dynamic)?;

    let has_path_lifetime = handlers.iter().any(|handler| handler.borrows_path);
    let route_generics = has_path_lifetime.then(|| quote! { <'p> });
    let variants = handlers.iter().map(|handler| {
        let attrs = &handler.route_attrs;
        let variant = &handler.variant;
        let fields = handler.captures.iter().map(|(name, ty)| quote! { #name: #ty });
        if handler.captures.is_empty() {
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
        enum #route_name #route_generics {
            #(#variants),*
        }
    })?;
    let generated_resolver = resolver::expand_named(route_item, None)?;

    let context_type = &handlers[0].context_type;
    let response_type = &handlers[0].response_type;
    let context_ident = context_ident(&handlers);
    let runtime = resolver::runtime_path();

    for impl_item in &mut item.items {
        if let ImplItem::Fn(method) = impl_item {
            method.attrs.retain(|attribute| !attribute.path().is_ident("route"));
        }
    }
    let service_api = if has_dynamic {
        dynamic_service_api(
            &mut item,
            &handlers,
            &service_name,
            &route_name,
            context_type,
            response_type,
            &context_ident,
            &runtime,
        )?
    } else {
        item.items.push(static_dispatch_method(
            &handlers,
            &route_name,
            context_type,
            response_type,
            &context_ident,
            &runtime,
        )?);
        quote! {}
    };

    Ok(quote! {
        #generated_resolver
        #service_api
        #item
    })
}

fn validate_generated_method_names(item: &ItemImpl, has_dynamic: bool) -> syn::Result<()> {
    let generated = if has_dynamic { "router_builder" } else { "dispatch" };
    let collides = item
        .items
        .iter()
        .any(|impl_item| matches!(impl_item, ImplItem::Fn(method) if method.sig.ident == generated));
    if collides {
        return Err(Error::new(
            item.impl_token.span(),
            format!("`#[service]` cannot generate `{generated}` because that method already exists"),
        ));
    }
    Ok(())
}

fn static_dispatch_method(
    handlers: &[Handler],
    route_name: &Ident,
    context_type: &Type,
    response_type: &Type,
    context_ident: &Ident,
    runtime: &TokenStream2,
) -> syn::Result<ImplItem> {
    let arms = dispatch_arms(handlers, route_name, &quote! { self }, context_ident);
    syn::parse2(quote! {
        /// Resolves a method and path and invokes the matching service handler.
        ///
        /// # Errors
        ///
        /// Returns `routerama::ResolveError` when no route matches or a captured
        /// path value cannot be decoded or converted.
        pub async fn dispatch<'p>(
            &self,
            __method: impl ::core::convert::AsRef<str>,
            __path: &'p str,
            #context_ident: #context_type,
        ) -> ::core::result::Result<#response_type, #runtime::ResolveError<'p>> {
            match #route_name::resolver().resolve(__method, __path)? {
                #(#arms),*
            }
        }
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "the generated service-router API needs each independently validated name and boundary type"
)]
fn dynamic_service_api(
    item: &mut ItemImpl,
    handlers: &[Handler],
    service_name: &Ident,
    route_name: &Ident,
    context_type: &Type,
    response_type: &Type,
    context_ident: &Ident,
    runtime: &TokenStream2,
) -> syn::Result<TokenStream2> {
    let service_type = &item.self_ty;
    let resolver_name = format_ident!("{}Resolver", route_name, span = route_name.span());
    let resolver_builder_name = format_ident!("{}Builder", resolver_name, span = resolver_name.span());
    let service_router_name = format_ident!("{}Router", service_name, span = service_name.span());
    let service_builder_name = format_ident!("{}RouterBuilder", service_name, span = service_name.span());
    let arms = dispatch_arms(handlers, route_name, &quote! { __service }, context_ident);

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
                    method: #runtime::HttpMethod,
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
                __builder: #route_name::builder(),
            }
        }
    })?);

    Ok(quote! {
        #[doc = "A configured router for the service."]
        #[derive(Debug)]
        pub struct #service_router_name {
            __resolver: #resolver_name,
        }

        #[doc = "Builds a configured router for the service."]
        #[derive(Debug)]
        pub struct #service_builder_name {
            __builder: #resolver_builder_name,
        }

        #[automatically_derived]
        impl #service_builder_name {
            #(#add_methods)*

            /// Validates dynamic registrations and builds the service router.
            ///
            /// # Errors
            ///
            /// Returns `routerama::ConfigurationError` containing every missing
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
            /// Resolves a method and path and invokes the matching service handler.
            ///
            /// # Errors
            ///
            /// Returns `routerama::ResolveError` when no route matches or a
            /// captured path value cannot be decoded or converted.
            pub async fn dispatch<'p>(
                &self,
                __service: &#service_type,
                __method: impl ::core::convert::AsRef<str>,
                __path: &'p str,
                #context_ident: #context_type,
            ) -> ::core::result::Result<#response_type, #runtime::ResolveError<'p>> {
                match self.__resolver.resolve(__method, __path)? {
                    #(#arms),*
                }
            }
        }
    })
}

fn dispatch_arms(handlers: &[Handler], route_name: &Ident, target: &TokenStream2, context_ident: &Ident) -> Vec<TokenStream2> {
    handlers
        .iter()
        .map(|handler| {
            let variant = &handler.variant;
            let method = &handler.method;
            let pattern = if handler.captures.is_empty() {
                quote! { #route_name::#variant }
            } else {
                let fields = handler.captures.iter().map(|(name, _)| name);
                quote! { #route_name::#variant { #(#fields),* } }
            };
            let arguments = handler.arguments.iter().map(|argument| match argument {
                Argument::Capture(name) => quote! { #name },
                Argument::Context => quote! { #context_ident },
            });
            quote! {
                #pattern => ::core::result::Result::Ok(#target.#method(#(#arguments),*).await)
            }
        })
        .collect()
}

fn validate_impl(item: &ItemImpl) -> syn::Result<()> {
    if item.trait_.is_some() {
        return Err(Error::new(item.impl_token.span(), "`#[service]` requires an inherent impl"));
    }
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(Error::new(
            item.generics.span(),
            "`#[service]` does not yet support generic impl blocks",
        ));
    }
    if item.unsafety.is_some() {
        return Err(Error::new(
            item.impl_token.span(),
            "`#[service]` does not support unsafe impl blocks",
        ));
    }
    if let Some(attribute) = item
        .attrs
        .iter()
        .find(|attribute| attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr"))
    {
        return Err(Error::new(
            attribute.span(),
            "`#[service]` does not support conditional compilation on the impl block",
        ));
    }
    Ok(())
}

fn service_name(self_ty: &Type) -> syn::Result<Ident> {
    let Type::Path(path) = self_ty else {
        return Err(Error::new(self_ty.span(), "`#[service]` requires a named service type"));
    };
    if path.qself.is_some()
        || path
            .path
            .segments
            .last()
            .is_some_and(|segment| !matches!(segment.arguments, PathArguments::None))
    {
        return Err(Error::new(
            self_ty.span(),
            "`#[service]` does not yet support generic service types",
        ));
    }
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.clone())
        .ok_or_else(|| Error::new(self_ty.span(), "`#[service]` requires a named service type"))
}

fn parse_handler(method: &ImplItemFn, context_first: bool) -> syn::Result<Handler> {
    validate_signature(method)?;
    if let Some(attribute) = method
        .attrs
        .iter()
        .find(|attribute| attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr"))
    {
        return Err(Error::new(
            attribute.span(),
            "`#[service]` does not support conditionally compiled route handlers",
        ));
    }
    let route_attrs: Vec<_> = method
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("route"))
        .cloned()
        .collect();
    let kind = route_kind(&route_attrs)?;
    let capture_names = if kind == HandlerKind::Static {
        route_capture_names(&route_attrs)?
    } else {
        Vec::new()
    };
    let variant = variant_name(&method.sig.ident)?;
    let (captures, arguments, context_type, borrows_path) = handler_parameters(method, kind, &capture_names, context_first)?;

    let context_type = context_type.ok_or_else(|| {
        let message = if context_first {
            "`#[service(context)]` handlers require a context parameter immediately after `&self`"
        } else {
            "service handlers require one borrowed request-context parameter"
        };
        Error::new(method.sig.ident.span(), message)
    })?;
    let response_type = response_type(method)?;

    Ok(Handler {
        method: method.sig.ident.clone(),
        variant,
        kind,
        route_attrs: if kind == HandlerKind::Static { route_attrs } else { Vec::new() },
        captures,
        arguments,
        context_type,
        response_type,
        borrows_path,
    })
}

fn route_kind(route_attrs: &[Attribute]) -> syn::Result<HandlerKind> {
    let dynamic_attrs: Vec<_> = route_attrs
        .iter()
        .filter(|attribute| {
            matches!(
                &attribute.meta,
                syn::Meta::List(list) if list.tokens.to_string() == "dynamic"
            )
        })
        .collect();
    if dynamic_attrs.is_empty() {
        return Ok(HandlerKind::Static);
    }
    if route_attrs.len() != 1 {
        return Err(Error::new(
            dynamic_attrs[0].span(),
            "`#[route(dynamic)]` cannot be combined with another route attribute",
        ));
    }
    Ok(HandlerKind::Dynamic)
}

type HandlerParameters = (Vec<(Ident, Type)>, Vec<Argument>, Option<Type>, bool);

fn handler_parameters(
    method: &ImplItemFn,
    kind: HandlerKind,
    capture_names: &[String],
    context_first: bool,
) -> syn::Result<HandlerParameters> {
    let mut captures = Vec::new();
    let mut arguments = Vec::new();
    let mut context_type = None;
    let mut borrows_path = false;
    let mut inputs = method.sig.inputs.iter().skip(1);

    if context_first {
        let input = inputs.next().ok_or_else(|| {
            Error::new(
                method.sig.ident.span(),
                "`#[service(context)]` handlers require a context parameter immediately after `&self`",
            )
        })?;
        let FnArg::Typed(input) = input else {
            return Err(Error::new(input.span(), "service handlers must have exactly one `&self` receiver"));
        };
        parameter_pattern(input.pat.as_ref())?;
        if matches!(input.ty.as_ref(), Type::ImplTrait(_)) {
            return Err(Error::new(
                input.ty.span(),
                "`#[service(context)]` requires one concrete context type shared by every handler",
            ));
        }
        context_type = Some(input.ty.as_ref().clone());
        arguments.push(Argument::Context);
    }

    for input in inputs {
        let FnArg::Typed(input) = input else {
            return Err(Error::new(input.span(), "service handlers must have exactly one `&self` receiver"));
        };
        let pattern = parameter_pattern(input.pat.as_ref())?;
        let is_static_capture = kind == HandlerKind::Static && capture_names.iter().any(|capture| capture == &pattern.ident.to_string());
        if is_static_capture {
            let (capture_type, borrows) = capture_type(input.ty.as_ref())?;
            borrows_path |= borrows;
            captures.push((pattern.ident.clone(), capture_type));
            arguments.push(Argument::Capture(pattern.ident.clone()));
        } else if kind == HandlerKind::Dynamic && !matches!(input.ty.as_ref(), Type::Reference(_)) {
            captures.push((pattern.ident.clone(), input.ty.as_ref().clone()));
            arguments.push(Argument::Capture(pattern.ident.clone()));
        } else if context_first {
            let message = if kind == HandlerKind::Dynamic {
                "dynamic captures must be owned"
            } else {
                "every parameter after the context must match a static path capture"
            };
            return Err(Error::new(input.span(), message));
        } else {
            let Type::Reference(reference) = input.ty.as_ref() else {
                return Err(Error::new(
                    input.ty.span(),
                    "the request-context parameter must be a shared reference",
                ));
            };
            if reference.mutability.is_some() || context_type.is_some() {
                let message = if kind == HandlerKind::Dynamic {
                    "dynamic captures must be owned and handlers require exactly one shared request-context reference"
                } else {
                    "service handlers require exactly one shared request-context reference"
                };
                return Err(Error::new(input.span(), message));
            }
            context_type = Some(input.ty.as_ref().clone());
            arguments.push(Argument::Context);
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

    Ok((captures, arguments, context_type, borrows_path))
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

fn route_capture_names(route_attrs: &[Attribute]) -> syn::Result<Vec<String>> {
    let mut first = None;
    for attribute in route_attrs {
        let RouteAttr { path, .. } = attribute.parse_args()?;
        let path_value = path.value();
        let template = PathTemplate::parse(&path_value, Grammar::default().with_segment_affixes())
            .map_err(|error| Error::new(path.span(), format!("invalid path template: {error}")))?;
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
    let context = handlers[0].context_type.to_token_stream().to_string();
    let response = handlers[0].response_type.to_token_stream().to_string();
    let mut variants: Vec<String> = Vec::with_capacity(handlers.len());
    for handler in handlers {
        if handler.context_type.to_token_stream().to_string() != context {
            return Err(Error::new(
                handler.context_type.span(),
                "every service handler must use the same request-context type",
            ));
        }

        if handler.response_type.to_token_stream().to_string() != response {
            return Err(Error::new(
                handler.response_type.span(),
                "every service handler must return the same response type",
            ));
        }
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

fn context_ident(handlers: &[Handler]) -> Ident {
    let mut name = "__routerama_request_context".to_string();
    while handlers
        .iter()
        .flat_map(|handler| &handler.captures)
        .any(|(capture, _)| capture == &name)
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

    fn expand_service(item: TokenStream2) -> syn::Result<TokenStream2> {
        expand(syn::parse2(item).expect("test input is a syntactically valid impl"), false)
    }

    fn expand_context_service(item: TokenStream2) -> syn::Result<TokenStream2> {
        expand(syn::parse2(item).expect("test input is a syntactically valid impl"), true)
    }

    #[test]
    fn valid_service_generates_dispatch_and_a_private_resolver() {
        let generated = expand_service(quote! {
            impl Api {
                #[route(GET, "/books")]
                async fn list(&self, request: &Context) -> Response {
                    response(request)
                }

                #[route(GET, "/books/{id}")]
                async fn get(&self, id: u32, request: &Context) -> Response {
                    response_with_id(request, id)
                }
            }
        })
        .expect("valid service");
        let code = generated.to_string();
        assert!(code.contains("async fn dispatch"), "{code}");
        assert!(code.contains("__ApiRouteResolver"), "{code}");
        assert!(!code.contains("# [route"), "handler route attributes are consumed: {code}");
    }

    #[test]
    fn dynamic_service_generates_a_persistent_router_and_builder() {
        let generated = expand_service(quote! {
            impl Api {
                #[route(GET, "/health")]
                async fn health(&self, request: &Context) -> Response {
                    response(request)
                }

                #[route(dynamic)]
                async fn plugin(&self, name: String, request: &Context) -> Response {
                    plugin_response(request, name)
                }

                async fn dispatch(&self, request: &Context) -> Response {
                    response(request)
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
    }

    #[test]
    fn context_mode_forwards_owned_shared_and_mutable_first_parameters() {
        for context_type in [quote! { Context }, quote! { &Context }, quote! { &mut Context }] {
            let generated = expand_context_service(quote! {
                impl Api {
                    #[route(dynamic)]
                    async fn plugin(
                        &self,
                        context: #context_type,
                        name: String,
                    ) -> Response {
                        plugin_response(context, name)
                    }
                }
            })
            .expect("context form is forwarded unchanged");
            let code = generated.to_string();
            assert!(code.contains("context :"), "{code}");
            assert!(code.contains("add_plugin"), "{code}");
        }
    }

    #[test]
    fn context_mode_requires_context_before_captures() {
        let error = expand_context_service(quote! {
            impl Api {
                #[route(GET, "/books/{id}")]
                async fn get(&self, id: u32, context: &Context) -> Response {
                    response(context, id)
                }
            }
        })
        .expect_err("the first parameter is reserved for context");
        assert!(error.to_string().contains("after the context"), "{error}");
    }

    #[test]
    fn context_mode_rejects_an_opaque_context_type() {
        let error = expand_context_service(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn get(&self, context: impl Context) -> Response {
                    response(context)
                }
            }
        })
        .expect_err("context is not a concrete type");
        assert!(error.to_string().contains("concrete context type"), "{error}");
    }

    #[test]
    fn handler_contract_errors_are_reported() {
        for (item, expected) in [
            (
                quote! {
                    impl Api {
                        #[route(GET, "/")]
                        fn home(&self, request: &Context) -> Response {
                            response(request)
                        }
                    }
                },
                "must be async",
            ),
            (
                quote! {
                    impl Api {
                        #[route(GET, "/books/{id}")]
                        async fn get(&self, request: &Context) -> Response {
                            response(request)
                        }
                    }
                },
                "do not match",
            ),
            (
                quote! {
                    impl Api {
                        #[route(GET, "/")]
                        async fn home(&self, request: Context) -> Response {
                            response(&request)
                        }
                    }
                },
                "shared reference",
            ),
            (
                quote! {
                    impl Api {
                        #[route(GET, "/books/{name}")]
                        async fn get(&self, name: &'static str, request: &Context) -> Response {
                            response(request, name)
                        }
                    }
                },
                "must use `&str`",
            ),
            (
                quote! {
                    impl Api {
                        async fn dispatch(&self, request: &Context) -> Response {
                            response(request)
                        }

                        #[route(GET, "/")]
                        async fn home(&self, request: &Context) -> Response {
                            response(request)
                        }
                    }
                },
                "already exists",
            ),
        ] {
            let error = expand_service(item).expect_err("invalid service contract");
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn all_handlers_must_share_boundary_types() {
        for (item, expected) in [
            (
                quote! {
                    impl Api {
                        #[route(GET, "/")]
                        async fn home(&self, request: &Context) -> Response {
                            response(request)
                        }

                        #[route(GET, "/other")]
                        async fn other(&self, request: &OtherContext) -> Response {
                            other_response(request)
                        }
                    }
                },
                "request-context type",
            ),
            (
                quote! {
                    impl Api {
                        #[route(GET, "/")]
                        async fn home(&self, request: &Context) -> First {
                            first(request)
                        }

                        #[route(GET, "/other")]
                        async fn other(&self, request: &Context) -> Second {
                            second(request)
                        }
                    }
                },
                "response type",
            ),
        ] {
            let error = expand_service(item).expect_err("boundary types differ");
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn dynamic_handler_contract_errors_are_reported() {
        for (item, expected) in [
            (
                quote! {
                    impl Api {
                        #[route(dynamic)]
                        #[route(GET, "/books/{name}")]
                        async fn get(&self, name: String, request: &Context) -> Response {
                            response(request, name)
                        }
                    }
                },
                "cannot be combined",
            ),
            (
                quote! {
                    impl Api {
                        #[route(dynamic)]
                        async fn get(&self, name: &str, request: &Context) -> Response {
                            response(request, name)
                        }
                    }
                },
                "dynamic captures must be owned",
            ),
            (
                quote! {
                    impl Api {
                        fn router_builder() {}

                        #[route(dynamic)]
                        async fn get(&self, name: String, request: &Context) -> Response {
                            response(request, name)
                        }
                    }
                },
                "already exists",
            ),
        ] {
            let error = expand_service(item).expect_err("invalid dynamic service contract");
            assert!(error.to_string().contains(expected), "{error}");
        }
    }
}
