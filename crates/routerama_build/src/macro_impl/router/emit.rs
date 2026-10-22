// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Emission of route methods and the generated service API.

use alloc::format;
use alloc::string::ToString as _;
use alloc::vec::Vec;

use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{ImplItem, ItemImpl};

use super::contract::{generated_ident, route_contract};
use super::dispatch::dispatch_arms;
use super::generics::{
    mount_contract, mount_delegate_type, mounted_response_return_type, mounted_route_generics, response_return_type, route_generics,
};
use super::lowering::{dispatch_label, emit_before_call, resolve_failure_response};
use super::model::{
    DispatchArm, DispatchBoundary, DispatchExit, DispatchResponse, GeneratedIdents, Handler, HandlerKind, ResponseBodyModel, RoutingPolicy,
    SharedState,
};
#[cfg(feature = "tower")]
use super::tower::{dynamic_tower_route_impl, dynamic_tower_service_method};
use crate::macro_impl::resolver;

pub(super) fn encapsulated_resolver(
    module_name: &Ident,
    generated_resolver: &TokenStream2,
    generated_response_body: &TokenStream2,
) -> TokenStream2 {
    // The two `#[allow(...)]` attributes below are emitted into the consumer's
    // crate rather than applied here, so they cannot be `#[expect(...)]`: the
    // generated module may or may not trip either lint depending on the service.
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

#[expect(
    clippy::too_many_arguments,
    reason = "the static entry keeps its generated route, policy, response, and runtime models explicit"
)]
#[expect(
    clippy::too_many_lines,
    reason = "the generated static route keeps dispatch, policy, response, and runtime contracts together"
)]
/// Builds the base-boundary `route` method as an `ImplItem`.
pub(super) fn static_route_method(
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
    let item: ImplItem = syn::parse2(quote! {
        /// Routes an HTTP request and returns a composed response.
        ///
        /// The response carries an allocation-free, service-specific opaque body
        /// sum.
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
    .expect(
        "the generated base-boundary route method's identifiers, bounds, and dispatch arms are all macro-controlled \
         and always form a syntactically valid item",
    );
    Ok(item)
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the explicit mount entry keeps its generated route, policy, response, interceptor, and runtime models visible"
)]
/// Builds the `route_with_erased_mounts` method as an `ImplItem`.
///
/// The only fallible step here is the final `syn::parse2`, over tokens the
/// macro assembles entirely from validated identifiers, bounds, and dispatch
/// arms; it can never fail to parse as an `ImplItem`, so this returns the item
/// directly rather than propagating an impossible `Err` to its single caller.
pub(super) fn static_mounted_route_method(
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
) -> ImplItem {
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
    )
    .expect(
        "static_route_method already validated the same handlers, policy, and shared state \
         earlier in this expansion, so an identical contract check cannot fail here",
    );
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
        /// Routes generated handlers before delegating a complete miss to mounts.
        ///
        /// A complete miss goes to the mounted-service router. Generated response
        /// bodies remain concrete.
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
    .expect("a template assembled entirely from validated identifiers, bounds, and dispatch arms always parses as an impl item")
}

/// Generates the `add_<handler>` builder method for one dynamic handler.
fn dynamic_add_method(handler: &Handler) -> TokenStream2 {
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
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the generated service-router API keeps its validated symbols and emitted boundary together"
)]
pub(super) fn dynamic_service_api(
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
                    /// Routes generated handlers before delegating a complete miss to mounts.
                    ///
                    /// Generated response bodies remain concrete.
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
        .map(dynamic_add_method);

    item.items.push(
        syn::parse2(quote! {
            /// Creates a builder for the service's static and dynamic routes.
            #[must_use]
            pub fn router_builder() -> #service_builder_name {
                #service_builder_name {
                    __builder: #route_path::builder(),
                }
            }
        })
        .expect("fixed template interpolating only an identifier always parses as an impl item"),
    );

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
            ),
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

            /// Routes an HTTP request through the configured service.
            ///
            /// Returns its allocation-free, service-specific opaque body sum.
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

/// The control flow one generated entry wraps around route resolution and dispatch.
///
/// This is the request-head binding, the router-wide `#[before]` preamble, the
/// exit form every stage uses, and the generated-wide `#[after]` epilogue.
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

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use quote::quote;

    use crate::macro_impl::router::expand_with_data;
    use crate::macro_impl::router::test_support::{expand_fixed_mounted_router, expand_router};

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
    fn fixed_erased_mount_router_supports_heterogeneous_data() {
        let generated = expand_with_data(
            syn::parse_quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self) -> StatusCode {
                        StatusCode::NO_CONTENT
                    }
                }
            },
            Some(syn::parse_quote!(AppState)),
            true,
            false,
            true,
        )
        .expect("fixed erased-mount service supports heterogeneous response data");
        let code = generated.to_string();
        assert!(code.contains("async fn route_with_erased_mounts"), "{code}");
        assert!(code.contains("DataEitherBody"), "{code}");
    }

    #[test]
    fn fixed_erased_mount_dynamic_router_supports_a_mounted_route_method() {
        // Exercises `dynamic_service_api`'s own mounted-route generation
        // (its `route_with_erased_mounts` branch), which only fires when a
        // fixed, erased-mount router also has a dynamic handler.
        let generated = expand_fixed_mounted_router(
            syn::parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(dynamic)]
                    async fn plugin(&self, #[capture] name: String) -> StatusCode {
                        StatusCode::NO_CONTENT
                    }
                }
            },
        )
        .expect("fixed erased-mount router supports a dynamic handler");
        let code = generated.to_string();
        assert!(code.contains("async fn route_with_erased_mounts"), "{code}");
        assert!(code.contains(":: Left { body }"), "{code}");
        assert!(code.contains(":: Right { body }"), "{code}");
        assert!(code.contains("ResolveError :: NotFound"), "{code}");
        assert!(code.contains("Request :: from_parts"), "{code}");
    }

    #[test]
    fn fixed_erased_mount_dynamic_router_supports_heterogeneous_mounted_data() {
        // Same as above, but with heterogeneous response data, which routes
        // through the `DataEitherBody` variants of the mounted dispatch
        // inside `dynamic_service_api`.
        let generated = expand_with_data(
            syn::parse_quote! {
                impl Api {
                    #[route(dynamic)]
                    async fn plugin(&self, #[capture] name: String) -> StatusCode {
                        StatusCode::NO_CONTENT
                    }
                }
            },
            Some(syn::parse_quote!(AppState)),
            true,
            false,
            true,
        )
        .expect("fixed erased-mount dynamic router supports heterogeneous response data");
        let code = generated.to_string();
        assert!(code.contains("async fn route_with_erased_mounts"), "{code}");
        assert!(code.contains("DataEitherBody"), "{code}");
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
}
