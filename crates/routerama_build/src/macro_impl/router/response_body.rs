// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The generated response-body enum and the model it is built from.

use alloc::format;
use alloc::string::{String, ToString as _};
use alloc::vec::Vec;

use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::Type;

use super::binding::catcher_for;
use super::model::{
    Argument, CatcherPolicy, DispatchArm, DispatchKind, ExtractionKind, Handler, ResponseBodyModel, ResponseSource, RoutingPolicy,
    TransformMode,
};

pub(super) const ROUTING_RESPONSE_KEY: &str = "routerama:routing";

pub(super) const PREDICATE_RESPONSE_KEY: &str = "routerama:predicate";

pub(super) const FALLBACK_RESPONSE_KEY: &str = "routerama:fallback";

impl ResponseBodyModel {
    pub(super) fn variant(&self, key: &str) -> &Ident {
        &self
            .sources
            .iter()
            .find(|source| source.key == key)
            .expect("every emitted response source is registered before dispatch generation")
            .variant
    }
}

/// Describes an uncaught request-parts extractor's rejection response source.
fn parts_rejection_label(ty: &Type, name: &Ident, method: &Ident) -> String {
    format!("request-parts rejection `{}` first used by `{method}.{name}`", quote! { #ty })
}

/// Describes an uncaught request-body extractor's rejection response source.
fn body_rejection_label(ty: &Type, name: &Ident, body_input: &TokenStream2, method: &Ident) -> String {
    format!(
        "request-body rejection `{}` from `{body_input}` first used by `{method}.{name}`",
        quote! { #ty }
    )
}

pub(super) fn response_body_model(
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
                        let label = parts_rejection_label(ty, name, &handler.method);
                        add_response_source(&mut model, parts_rejection_key(ty), label);
                    }
                }
                Argument::Body(name, ty) => {
                    if let Some(catcher) = catcher_for(policy, ExtractionKind::Body, ty) {
                        add_catcher_response_source(&mut model, catcher);
                    } else {
                        let body_input = policy.body_input_for(&handler.method, &generated_body_type());
                        let label = body_rejection_label(ty, name, &body_input, &handler.method);
                        add_response_source(&mut model, body_rejection_key(ty, &body_input), label);
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
pub(super) fn generated_body_type() -> Ident {
    format_ident!("__RouteramaBody")
}

pub(super) fn interceptor_response_key(method: &Ident) -> String {
    format!("interceptor:{method}")
}

pub(super) fn transform_buffer_key(method: &Ident) -> String {
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

pub(super) fn handler_response_key(response_type: &Type) -> String {
    format!("handler:{}", quote! { #response_type })
}

pub(super) fn parts_rejection_key(extractor_type: &Type) -> String {
    format!("parts-rejection:{}", quote! { #extractor_type })
}

pub(super) fn body_rejection_key(extractor_type: &Type, body_input: &TokenStream2) -> String {
    format!("body-rejection:{}@{}", quote! { #extractor_type }, body_input)
}

pub(super) fn catcher_response_key(catcher: &CatcherPolicy) -> String {
    format!("catcher:{}", catcher.method)
}

#[expect(
    clippy::too_many_lines,
    reason = "the complete generated body and error sums are kept together so their variants cannot drift"
)]
pub(super) fn response_body_definition(model: &ResponseBodyModel, runtime: &TokenStream2, response_runtime: &TokenStream2) -> TokenStream2 {
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

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use quote::quote;
    use syn::parse_quote;

    use crate::macro_impl::router::test_support::{expand_fixed_router, expand_heterogeneous_generic_router, expand_router};

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
    fn a_parts_extraction_without_a_catcher_erases_the_rejection_body_under_generic_state() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, value: Query) -> StatusCode {
                    response(value)
                }
            }
        })
        .expect("a bare, uncaught parts extraction is valid under generic state");
        let code = generated.to_string();
        assert!(code.contains("SendBoxBody :: new (body)"), "{code}");
    }

    #[test]
    fn a_parts_extraction_without_a_catcher_keeps_the_rejection_body_under_fixed_state() {
        let generated = expand_fixed_router(
            parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self, value: Query) -> StatusCode {
                        response(value)
                    }
                }
            },
        )
        .expect("a bare, uncaught parts extraction is valid under fixed state");
        let code = generated.to_string();
        assert!(!code.contains("SendBoxBody :: new (body)"), "{code}");
    }

    #[test]
    fn a_streaming_transform_with_a_replacement_body_swaps_the_body_type() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self, #[body] data: Body) -> StatusCode {
                    response(data)
                }

                #[transform(stream, create)]
                async fn wrap<B>(&self, parts: &RequestParts, body: B) -> BodyTransform<Body, StatusCode>
                where
                    B: http_body::Body<Data = Bytes>,
                {
                    wrap(parts, body)
                }
            }
        })
        .expect("a streaming transform with a replacement is valid");
        let code = generated.to_string();
        assert!(code.contains("BodyTransform :: Replace"), "{code}");
        assert!(code.contains("BodyTransform :: Respond"), "{code}");
    }

    #[test]
    fn a_buffered_transform_with_a_replacement_body_swaps_the_body_type() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self, #[body] data: Body) -> StatusCode {
                    response(data)
                }

                #[transform(limit = 16, create)]
                async fn wrap(&self, parts: &RequestParts, body: Bytes) -> BodyTransform<Body, StatusCode> {
                    wrap(parts, body)
                }
            }
        })
        .expect("a buffered transform with a replacement is valid");
        let code = generated.to_string();
        assert!(code.contains("BodyTransform :: Replace"), "{code}");
    }

    #[test]
    fn a_parts_extraction_without_a_catcher_under_heterogeneous_data_and_generic_state_omits_the_data_bound() {
        let generated = expand_heterogeneous_generic_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, value: Query) -> StatusCode {
                    response(value)
                }
            }
        })
        .expect("a bare, uncaught parts extraction is valid under heterogeneous data and generic state");
        let code = generated.to_string();
        assert!(code.contains("self . home (value)"), "{code}");
    }

    // --- generated_ident collision-avoidance loop ---
}
