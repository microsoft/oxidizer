// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Lowering of a selected handler into the generated dispatch body.

use alloc::vec::Vec;

use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::Lifetime;

use super::binding::catcher_for;
use super::dispatch::{OverlapPredicatePlan, PredicateKind, media_type_expression};
use super::model::{
    Argument, BeforeInterceptor, DispatchBoundary, DispatchResponse, ExtractionKind, Handler, ResponseBodyModel, RoutingPolicy,
    SharedState, TransformMode,
};
use super::response_body::{
    FALLBACK_RESPONSE_KEY, PREDICATE_RESPONSE_KEY, ROUTING_RESPONSE_KEY, body_rejection_key, catcher_response_key, handler_response_key,
    interceptor_response_key, parts_rejection_key, transform_buffer_key,
};
use crate::macro_impl::StaticHeaderOperation;

/// The label of the block wrapping a dispatch observed by an `#[after]` interceptor.
///
/// Only a generated-wide `#[after]` interceptor observes such responses.
pub(super) fn dispatch_label() -> Lifetime {
    Lifetime::new("'__routerama_dispatch", Span::call_site())
}

#[expect(
    clippy::too_many_arguments,
    reason = "generated direct dispatch names each owned request component and concrete response boundary"
)]
pub(super) fn emit_direct_dispatch(
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

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "overlap dispatch keeps the priority decision separate from all extraction owners"
)]
pub(super) fn emit_overlap_dispatch(
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

/// Emits one direct `#[before]` interceptor call.
///
/// The emitted call either proceeds or short-circuits with a mapped response.
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
pub(super) fn emit_before_call(
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

pub(super) fn resolve_failure_response(
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

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use quote::quote;
    use syn::parse_quote;

    use crate::macro_impl::router::test_support::{expand_mounted_heterogeneous_router, expand_mounted_router, expand_router};

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
    fn overlap_predicate_failure_routes_through_a_declared_fallback() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/items/{id}", host = "one.example", priority = 2)]
                async fn one(&self, id: u32) -> StatusCode {
                    response(id)
                }

                #[route(GET, "/items/{id}", host = "two.example", priority = 1)]
                async fn two(&self, id: u32) -> StatusCode {
                    response(id)
                }

                #[fallback]
                async fn fallback(&self, failure: RouteFailure<'_>) -> FallbackResponse {
                    fallback_response(failure)
                }
            }
        })
        .expect("every overlap candidate has an explicit distinct priority");
        let code = generated.to_string();
        assert!(
            code.contains("self . fallback (") && code.contains(". await"),
            "a predicate mismatch with no default candidate routes through the declared fallback: {code}"
        );
        assert!(code.contains("HostMismatch"), "{code}");
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
    fn fallback_routes_resolution_failures_and_predicate_mismatches() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/items/{id}", host = "one.example")]
                async fn one(&self, id: u32) -> StatusCode {
                    response(id)
                }

                #[fallback]
                async fn fallback(&self, failure: RouteFailure<'_>) -> FallbackResponse {
                    fallback_response(failure)
                }
            }
        })
        .expect("a fallback is a valid policy method");
        let code = generated.to_string();
        assert!(code.contains("self . fallback ("), "{code}");
        assert!(code.contains("route_failure"), "{code}");
        assert!(code.contains("host_matches"), "{code}");
    }

    #[test]
    fn router_wide_before_short_circuits_with_a_mapped_response() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before]
                async fn guard(&self, context: &mut BeforeContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect("a router-wide before is valid");
        let code = generated.to_string();
        assert!(code.contains("self . guard"), "{code}");
        assert!(code.contains("Before :: Next"), "{code}");
        assert!(code.contains("Before :: Respond"), "{code}");
    }

    #[test]
    fn per_handler_before_takes_a_selected_context() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[before(home)]
                async fn guard(&self, context: &mut SelectedContext<'_>) -> Before<StatusCode> {
                    guard(context)
                }
            }
        })
        .expect("a per-handler before is valid");
        let code = generated.to_string();
        assert!(code.contains("SelectedContext :: new"), "{code}");
    }

    #[test]
    fn generated_wide_after_wraps_the_response_epilogue() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[after]
                async fn observe(&self, context: &mut AfterContext<'_>) -> () {
                    observe(context)
                }
            }
        })
        .expect("a generated-wide after is valid");
        let code = generated.to_string();
        assert!(code.contains("AfterContext :: new"), "{code}");
        assert!(code.contains("into_parts"), "{code}");
    }

    #[test]
    fn per_handler_after_wraps_only_its_handler() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[after(home)]
                async fn observe(&self, context: &mut AfterContext<'_>) -> () {
                    observe(context)
                }
            }
        })
        .expect("a per-handler after is valid");
        let code = generated.to_string();
        assert!(code.contains("AfterContext :: new"), "{code}");
    }

    #[test]
    fn buffered_transform_short_circuits_on_the_rejection() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self) -> StatusCode {
                    response()
                }

                #[transform(limit = 16, create)]
                async fn shrink(&self, parts: &RequestParts, body: Bytes) -> BodyConsumed<StatusCode> {
                    shrink(parts, body)
                }
            }
        })
        .expect("a buffered transform without a replacement is valid");
        let code = generated.to_string();
        assert!(code.contains("BodyConsumed :: Consumed"), "{code}");
        assert!(code.contains("BodyConsumed :: Respond"), "{code}");
    }

    #[test]
    fn a_static_header_insert_and_append_are_emitted() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/", headers(insert("x-a", "1"), append("x-b", "2")))]
                async fn home(&self) -> StatusCode {
                    response()
                }
            }
        })
        .expect("static headers are valid");
        let code = generated.to_string();
        assert!(code.contains("headers_mut () . insert"), "{code}");
        assert!(code.contains("headers_mut () . append"), "{code}");
    }

    #[test]
    fn a_produces_predicate_inserts_the_content_type_header() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/", produces = "application/json")]
                async fn home(&self) -> StatusCode {
                    response()
                }
            }
        })
        .expect("a produces predicate is valid");
        let code = generated.to_string();
        assert!(code.contains("CONTENT_TYPE"), "{code}");
    }

    #[test]
    fn mounted_dispatch_wraps_the_response_in_an_either_body() {
        let generated = expand_mounted_router(
            parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self) -> StatusCode {
                        response()
                    }
                }
            },
        )
        .expect("a fixed-state mounted router is valid");
        let code = generated.to_string();
        assert!(code.contains("route_with_erased_mounts"), "{code}");
        assert!(code.contains("EitherBody :: Left"), "{code}");
    }

    #[test]
    fn mounted_heterogeneous_dispatch_wraps_the_response_in_a_data_either_body() {
        let generated = expand_mounted_heterogeneous_router(
            parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self) -> ViewResponse {
                        response()
                    }
                }
            },
        )
        .expect("a fixed-state mounted heterogeneous router is valid");
        let code = generated.to_string();
        assert!(code.contains("DataEitherBody :: Left"), "{code}");
    }

    // --- validate_impl / service_name ---

    #[test]
    fn a_dynamic_route_handler_is_dispatched_directly() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(dynamic)]
                async fn any(&self) -> StatusCode {
                    any()
                }
            }
        })
        .expect("a dynamic-only router is valid");
        let code = generated.to_string();
        assert!(code.contains("any"), "{code}");
    }

    #[test]
    fn a_consumes_predicate_mismatch_falls_through_to_the_fallback() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(POST, "/", consumes = "application/json")]
                async fn create(&self) -> StatusCode {
                    response()
                }
            }
        })
        .expect("a consumes predicate is valid");
        let code = generated.to_string();
        assert!(code.contains("content_type_matches_parsed"), "{code}");
    }

    // --- route_contract bound generation ---
}
