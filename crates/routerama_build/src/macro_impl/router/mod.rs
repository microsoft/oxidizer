// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementation of `#[router]`.

mod binding;
mod contract;
mod dispatch;
mod emit;
mod generics;
mod interceptor;
mod lowering;
mod model;
mod parse;
mod policy;
mod response_body;
#[cfg(test)]
mod test_support;
mod tower;

use alloc::vec::Vec;

use binding::{bind_catchers, bind_interceptors};
use contract::{ImplItemAttributes, ParentModulePaths, generated_idents, has_policy_attr, has_route_attr};
use dispatch::build_dispatch_arms;
use emit::{dynamic_service_api, encapsulated_resolver, static_mounted_route_method, static_route_method};
use generics::{fixed_state_validation, shared_state};
use model::HandlerKind;
use parse::{is_parameter_marker, parse_handler, service_name, validate_generated_method_names, validate_handlers, validate_impl};
use policy::parse_policy;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use response_body::{response_body_definition, response_body_model};
use syn::spanned::Spanned as _;
use syn::visit_mut::VisitMut as _;
use syn::{Error, FnArg, ImplItem, ItemEnum, ItemImpl, Type};
#[cfg(feature = "tower")]
use tower::{static_tower_route_impl, static_tower_service_method};

use super::resolver;
use super::runtime::{RuntimeCapability, response_path, runtime_path};

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
    })
    .expect("the generated route enum's identifiers and variants are macro-controlled and always form a syntactically valid enum");
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
            ));
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
            ));
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
        )
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

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use quote::quote;

    use crate::macro_impl::router::test_support::{expand_heterogeneous_router, expand_router, has_route_method_attribute};

    #[test]
    fn a_router_impl_requires_at_least_one_route_handler() {
        let error = expand_router(quote! {
            impl Api {
                async fn home(&self) -> StatusCode {
                    response(())
                }
            }
        })
        .expect_err("an impl with no `#[route]` handlers is rejected");
        assert!(
            error.to_string().contains("`#[router]` requires at least one `#[route]` handler"),
            "{error}"
        );
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
    fn has_route_method_attribute_ignores_non_fn_impl_items() {
        let generated = expand_router(quote! {
            impl Api {
                const VERSION: u32 = 1;

                type Alias = StatusCode;

                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response(())
                }
            }
        })
        .expect("non-`fn` impl items without route or policy attributes are ignored");
        assert!(generated.to_string().contains("const VERSION : u32 = 1"), "{generated}");
        let file: syn::File = syn::parse2(generated).expect("expansion is a valid Rust file");
        assert!(
            !has_route_method_attribute(&file),
            "the const and type impl items are skipped, and the fn item's `#[route]` attribute is consumed"
        );
    }

    #[test]
    fn a_minimal_router_expands() {
        let expanded = expand_router(quote! {
            impl Api {
                #[route(GET, "/books")]
                async fn list(&self) -> &'static str {
                    "ok"
                }
            }
        })
        .expect("a minimal router expands");
        assert!(!expanded.is_empty());
    }

    // --- fallback, catchers, before/after, and transform codegen ---

    #[test]
    fn a_rest_capture_route_expands() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/files/{tail=**}")]
                async fn files(&self, tail: String) -> StatusCode {
                    files(tail)
                }
            }
        })
        .expect("a `**` rest capture route is valid");
        let code = generated.to_string();
        assert!(code.contains("self . files (tail)"), "{code}");
    }

    #[test]
    fn an_affix_capture_route_expands() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/images/img-{id}.png")]
                async fn image(&self, id: String) -> StatusCode {
                    image(id)
                }
            }
        })
        .expect("an intra-segment affix capture route is valid");
        let code = generated.to_string();
        assert!(code.contains("self . image (id)"), "{code}");
    }

    // --- validate_overlap edge cases ---
}
