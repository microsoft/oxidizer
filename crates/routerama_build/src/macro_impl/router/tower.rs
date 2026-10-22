// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generated `tower::Service` adapters.

#[cfg(feature = "tower")]
use alloc::vec;

#[cfg(feature = "tower")]
use proc_macro2::{Ident, TokenStream as TokenStream2};
#[cfg(feature = "tower")]
use quote::{format_ident, quote};
#[cfg(feature = "tower")]
use syn::{ImplItem, Type};

#[cfg(feature = "tower")]
use super::contract::{generated_ident, route_contract};
#[cfg(feature = "tower")]
use super::model::{GeneratedIdents, Handler, RoutingPolicy, SharedState};
#[cfg(feature = "tower")]
use super::response_body::generated_body_type;

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

#[cfg(feature = "tower")]
pub(super) fn static_tower_route_impl(
    handlers: &[Handler],
    policy: &RoutingPolicy,
    service_type: &Type,
    shared_state: &SharedState,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> TokenStream2 {
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
    )
    .expect(
        "`static_route_method` is called earlier in this same `!has_dynamic` branch and \
         propagates `route_contract` over identical arguments, so any contract error has \
         already aborted the expansion before this point",
    );
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

    quote! {
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
    }
}

#[cfg(feature = "tower")]
/// Builds the Tower-adapter `tower_service` method as an `ImplItem`.
///
/// The only fallible step here is the final `syn::parse2`, over tokens the
/// macro assembles entirely from validated identifiers and bounds (the `?` in
/// the emitted `?::core::marker::Sized` bound is literal `Sized`-relaxation
/// syntax, not a try operator); it can never fail to parse as an `ImplItem`,
/// so this returns the item directly rather than propagating an impossible
/// `Err` to its single caller.
pub(super) fn static_tower_service_method(
    handlers: &[Handler],
    _policy: &RoutingPolicy,
    generated: &GeneratedIdents,
    shared_state: &SharedState,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> ImplItem {
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
        /// Creates an allocation-free Tower adapter preserving the exact response body.
        ///
        /// The body is this service's own generated one.
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
    .expect(
        "the generated Tower-adapter method's identifiers and bounds are all macro-controlled and always form a \
         syntactically valid item",
    )
}

#[cfg(feature = "tower")]
#[expect(
    clippy::too_many_arguments,
    reason = "Tower generation needs the validated router, state, data-mode, and runtime paths"
)]
pub(super) fn dynamic_tower_route_impl(
    handlers: &[Handler],
    policy: &RoutingPolicy,
    router_type: &Ident,
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
    )
    .expect(
        "dynamic_service_api already validated the same handlers, policy, and shared state \
         earlier in this expansion, so an identical contract check cannot fail here",
    );
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

    quote! {
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
    }
}

#[cfg(feature = "tower")]
pub(super) fn dynamic_tower_service_method(
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
        /// Creates an allocation-free Tower adapter preserving the exact response body.
        ///
        /// The body is this configured service's own generated one.
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

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use quote::quote;
    #[cfg(feature = "tower")]
    use syn::ItemImpl;

    #[cfg(not(feature = "tower"))]
    use crate::macro_impl::router::expand;
    #[cfg(feature = "tower")]
    use crate::macro_impl::router::expand_with_data;
    #[cfg(feature = "tower")]
    use crate::macro_impl::router::test_support::{expand_tower_router, generated_method_signature};

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
    fn explicit_tower_contract_supports_a_generic_shared_state() {
        let tokens = expand_tower_router(
            None,
            quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self, #[state] state: &AppState) -> StatusCode {
                        StatusCode::NO_CONTENT
                    }
                }
            },
        )
        .expect("a static handler with a generic shared state supports the exact Tower adapter");
        let generated = tokens.to_string();
        assert!(generated.contains("pub fn tower_service"), "{generated}");
        assert!(generated.contains("GeneratedExactRoute"), "{generated}");
    }

    #[test]
    #[cfg(feature = "tower")]
    fn explicit_tower_contract_supports_heterogeneous_response_data() {
        let item: ItemImpl = syn::parse_quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }

                #[route(GET, "/plugin/{name}")]
                async fn plugin(&self, #[capture] name: String) -> String {
                    name
                }
            }
        };
        let tokens =
            expand_with_data(item, None, false, true, true).expect("a heterogeneous-data static router supports the exact Tower adapter");
        let generated = tokens.to_string();
        assert!(generated.contains("GeneratedExactRouteData"), "{generated}");
    }

    #[test]
    #[cfg(feature = "tower")]
    fn explicit_tower_contract_supports_heterogeneous_dynamic_response_data() {
        let item: ItemImpl = syn::parse_quote! {
            impl Api {
                #[route(dynamic)]
                async fn plugin(&self, #[capture] name: String) -> String {
                    name
                }

                #[route(dynamic)]
                async fn other(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }
            }
        };
        let tokens =
            expand_with_data(item, None, false, true, true).expect("a heterogeneous-data dynamic router supports the exact Tower adapter");
        let generated = tokens.to_string();
        assert!(generated.contains("GeneratedExactConfiguredRouteData"), "{generated}");
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

    #[cfg(feature = "tower")]
    #[test]
    fn route_contract_lifetime_errors_propagate_through_tower_entry_points() {
        let bad_handler = quote! {
            #[route(GET, "/")]
            async fn inspect(&self, headers: &'static HeaderMap) -> Response {
                response(headers)
            }
        };

        // Fixed state, static-only: exercises `static_tower_route_impl`'s own
        // `route_contract` call and its propagation back through `expand`.
        let error = expand_tower_router(
            Some(syn::parse_quote!(AppState)),
            quote! {
                impl Api {
                    #bad_handler
                }
            },
        )
        .expect_err("an explicit request-tied lifetime cannot be honored");
        assert!(error.to_string().contains("must be elided or use `'_`"), "{error}");

        // A dynamic service with a tower adapter: exercises
        // `dynamic_tower_route_impl`'s own `route_contract` call and its
        // propagation both out of `dynamic_service_api` and back through
        // `expand`.
        let error = expand_tower_router(
            None,
            quote! {
                impl Api {
                    #[route(dynamic)]
                    async fn plugin(&self, #[capture] name: String) -> Response {
                        response(name)
                    }

                    #bad_handler
                }
            },
        )
        .expect_err("an explicit request-tied lifetime cannot be honored");
        assert!(error.to_string().contains("must be elided or use `'_`"), "{error}");
    }
}
