// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared-state, generics, and mount-contract helpers.

use alloc::string::ToString as _;
use alloc::vec;
use alloc::vec::Vec;

use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::visit_mut::VisitMut as _;
use syn::{ImplItem, ItemImpl, Lifetime, Type};

use super::binding::{catcher_for, default_body_witness_rejection};
use super::contract::{RequestPartsLifetime, StaticAnonymousLifetimes, token_stream_contains_ident};
use super::model::{Argument, ExtractionKind, Handler, RoutingPolicy, SharedState, TransformMode};

pub(super) fn shared_state(fixed_state: Option<Type>) -> SharedState {
    if let Some(ty) = fixed_state {
        return SharedState { ty, generic: None };
    }

    let generic = Ident::new("__RouteramaState", Span::call_site());
    let ty = syn::parse_quote!(#generic);
    SharedState {
        ty,
        generic: Some(generic),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the fixed-state alias, parts/body witnesses, and response assertions form one compile-only contract"
)]
pub(super) fn fixed_state_validation(
    item: &ItemImpl,
    handlers: &[Handler],
    policy: &RoutingPolicy,
    shared_state: &SharedState,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> syn::Result<Option<ImplItem>> {
    if shared_state.generic.is_some() {
        return Ok(None);
    }

    let mut validation_method_name = "__routerama_validate_fixed_state".to_string();
    while item.items.iter().any(|impl_item| match impl_item {
        ImplItem::Const(constant) => constant.ident == validation_method_name,
        ImplItem::Fn(method) => method.sig.ident == validation_method_name,
        _ => false,
    }) {
        validation_method_name.insert(0, '_');
    }
    let validation_method = Ident::new(&validation_method_name, Span::call_site());
    let source_state_type = &shared_state.ty;
    let mut state_alias_name = "__RouteramaFixedStateContract".to_string();
    while token_stream_contains_ident(quote! { #source_state_type }, &state_alias_name)
        || handlers.iter().any(|handler| {
            handler
                .captures
                .iter()
                .map(|(_, ty)| ty)
                .chain(handler.arguments.iter().filter_map(|argument| match argument {
                    Argument::Parts(_, ty) | Argument::Body(_, ty) => Some(ty),
                    Argument::Capture(_) => None,
                }))
                .chain(core::iter::once(&handler.response_type))
                .any(|ty| token_stream_contains_ident(quote! { #ty }, &state_alias_name))
        })
        || policy
            .fallback
            .iter()
            .map(|fallback| &fallback.response_type)
            .chain(policy.catchers.iter().flat_map(|catcher| {
                core::iter::once(&catcher.rejection_type)
                    .chain(catcher.extractor_type.iter())
                    .chain(core::iter::once(&catcher.response_type))
            }))
            .any(|ty| token_stream_contains_ident(quote! { #ty }, &state_alias_name))
    {
        state_alias_name.insert(0, '_');
    }
    let state_alias = Ident::new(&state_alias_name, Span::call_site());
    let state_type = &shared_state.ty;
    let mut extractor_assertions = Vec::new();
    let mut rendered_extractors = Vec::new();
    for handler in handlers {
        for argument in &handler.arguments {
            let assertion = match argument {
                Argument::Parts(_, ty) => {
                    let lifetime = Lifetime::new("'__routerama_witness", Span::call_site());
                    let mut extractor_type = ty.clone();
                    RequestPartsLifetime::new(&lifetime).rewrite(&mut extractor_type)?;
                    let rejection = if let Some(catcher) = catcher_for(policy, ExtractionKind::Parts, ty) {
                        let rejection = &catcher.rejection_type;
                        quote! { #rejection }
                    } else {
                        quote! { _ }
                    };
                    quote! {
                        {
                            fn __routerama_assert_parts<__RouteramaRejection>()
                            where
                                __RouteramaRejection:
                                    #response_runtime::IntoResponse + 'static,
                                <__RouteramaRejection as #response_runtime::IntoResponse>::Body:
                                    'static,
                                for<#lifetime> #extractor_type:
                                    #runtime::FromRequestParts<
                                        #lifetime,
                                        #state_alias,
                                        Rejection = __RouteramaRejection,
                                    >,
                            {}

                            __routerama_assert_parts::<#rejection>();
                        }
                    }
                }
                Argument::Body(_, ty) => {
                    let transform = policy.transform_for(&handler.method);
                    if transform.is_some_and(|transform| matches!(transform.mode, TransformMode::Streaming)) {
                        // A streaming replacement names the generic transport
                        // body, so it cannot be checked eagerly here; the entry
                        // where-clause carries its `FromRequestBody` bound.
                        continue;
                    }
                    if let Some(replacement) = transform.and_then(|transform| transform.replacement_body.as_ref()) {
                        let mut extractor_type = ty.clone();
                        StaticAnonymousLifetimes::default().visit_type_mut(&mut extractor_type);
                        let mut replacement_type = replacement.clone();
                        StaticAnonymousLifetimes::default().visit_type_mut(&mut replacement_type);
                        quote! {
                            __routerama_assert_concrete_body::<#extractor_type, #state_alias, #replacement_type>();
                        }
                    } else {
                        let mut extractor_type = ty.clone();
                        StaticAnonymousLifetimes::default().visit_type_mut(&mut extractor_type);
                        let rejection = if let Some(catcher) = catcher_for(policy, ExtractionKind::Body, ty) {
                            let rejection = &catcher.rejection_type;
                            quote! { #rejection }
                        } else {
                            default_body_witness_rejection(ty, runtime)
                        };
                        quote! {
                            __routerama_assert_body::<#extractor_type, #state_alias, #rejection>();
                        }
                    }
                }
                Argument::Capture(_) => continue,
            };
            let text = assertion.to_string();
            if !rendered_extractors.contains(&text) {
                rendered_extractors.push(text);
                extractor_assertions.push(assertion);
            }
        }
    }

    let mut response_assertions = Vec::new();
    let mut rendered_responses = Vec::new();
    for response_type in handlers
        .iter()
        .map(|handler| &handler.response_type)
        .chain(policy.fallback.iter().map(|fallback| &fallback.response_type))
        .chain(policy.catchers.iter().map(|catcher| &catcher.response_type))
        .chain(policy.befores.iter().map(|before| &before.response_type))
        .chain(policy.transforms.iter().map(|transform| &transform.response_type))
    {
        let assertion = quote! {
            __routerama_assert_response::<#response_type>();
        };
        let text = assertion.to_string();
        if !rendered_responses.contains(&text) {
            rendered_responses.push(text);
            response_assertions.push(assertion);
        }
    }

    let validation: ImplItem = syn::parse2(quote! {
        #[allow(
            dead_code,
            private_bounds,
            reason = "the fixed-state alias and witnesses validate the complete handler contract when the service is defined"
        )]
        fn #validation_method() {
            type #state_alias = #state_type;

            fn __routerama_assert_body<
                __RouteramaExtractor,
                __RouteramaWitnessInput,
                __RouteramaRejection,
            >()
            where
                __RouteramaWitnessInput: ?::core::marker::Sized,
                __RouteramaRejection: #response_runtime::IntoResponse,
                __RouteramaExtractor:
                    #runtime::BodyStateWitness<
                        __RouteramaWitnessInput,
                        __RouteramaRejection,
                    >
                    + #runtime::FromRequestBody<
                        __RouteramaWitnessInput,
                        <__RouteramaExtractor as #runtime::BodyStateWitness<
                            __RouteramaWitnessInput,
                            __RouteramaRejection
                        >>::RequestBody,
                        Rejection = __RouteramaRejection,
                    >,
            {}

            fn __routerama_assert_response<__RouteramaResponse>()
            where
                __RouteramaResponse: #response_runtime::IntoResponse,
            {}

            fn __routerama_assert_concrete_body<
                __RouteramaExtractor,
                __RouteramaConcreteBodyState,
                __RouteramaReplacementBody,
            >()
            where
                __RouteramaConcreteBodyState: ?::core::marker::Sized,
                __RouteramaExtractor: #runtime::FromRequestBody<__RouteramaConcreteBodyState, __RouteramaReplacementBody>,
            {}

            #(#extractor_assertions)*
            #(#response_assertions)*
        }
    })
    .expect(
        "the generated validation function's identifiers, extractor/response witness assertions, and bounds are all \
         macro-controlled and always form a syntactically valid item",
    );

    Ok(Some(validation))
}

pub(super) fn route_generics(body_type: &Ident, shared_state: &SharedState) -> TokenStream2 {
    let mut parameters = vec![quote! { #body_type }];
    if let Some(state_type) = &shared_state.generic {
        parameters.push(quote! { #state_type: ?::core::marker::Sized });
    }
    quote! { <#(#parameters),*> }
}

/// The generic parameters of a generated mounted entry.
///
/// This is [`route_generics`] plus the [`MountDelegate`] parameter, which the
/// entry is generic over so one method serves every mount table.
/// Builds the erased-mount route's extra parameter list, beyond
/// `mounted_route_generics`'s always-present body and mount-delegate
/// parameters.
///
/// This is only taken for a generic/dynamic-state router; `expand()`
/// requires a fixed `state = StateType` contract whenever `erased_mounts` is
/// set (the only caller of `mounted_route_generics`), so this never runs.
#[cfg_attr(coverage_nightly, coverage(off))]
fn mounted_route_generic_state_parameter(shared_state: &SharedState) -> Option<TokenStream2> {
    shared_state
        .generic
        .as_ref()
        .map(|state_type| quote! { #state_type: ?::core::marker::Sized })
}

/// Builds the erased-mount route's extra generic parameter list.
pub(super) fn mounted_route_generics(body_type: &Ident, shared_state: &SharedState) -> TokenStream2 {
    let mounts_type = mount_delegate_type();
    let mut parameters = vec![quote! { #body_type }];
    parameters.extend(mounted_route_generic_state_parameter(shared_state));
    parameters.push(quote! { #mounts_type });
    quote! { <#(#parameters),*> }
}

pub(super) fn response_return_type(
    body_type: &Ident,
    shared_state: &SharedState,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> TokenStream2 {
    let mut captures = vec![quote! { #body_type }];
    if let Some(state_type) = &shared_state.generic {
        captures.push(quote! { #state_type });
    }
    if heterogeneous_data {
        quote! {
            #response_runtime::Response<
                impl #runtime::http_body::Body<
                    Error = impl ::core::error::Error + use<#(#captures),*>,
                > + use<#(#captures),*>
            >
        }
    } else {
        quote! {
            #response_runtime::Response<
                impl #runtime::http_body::Body<
                    Data = #runtime::bytes::Bytes,
                    Error = impl ::core::error::Error + use<#(#captures),*>,
                > + use<#(#captures),*>
            >
        }
    }
}

/// The generic mount-delegate parameter of a generated mounted entry.
///
/// Making the entry generic over [`MountDelegate`] rather than naming
/// `ErasedMountRouter` is what lets one generated method serve both the local
/// and the `Send` mount routers: the delegate's response body is an associated
/// type, so the auto traits of the opaque return type follow whichever router
/// the caller passed.
pub(super) fn mount_delegate_type() -> Ident {
    format_ident!("__RouteramaMounts")
}

/// The generated pieces a mounted entry needs to delegate through [`MountDelegate`].
pub(super) struct MountContract {
    /// The associated response body of the delegate.
    pub(super) body: TokenStream2,
    /// The where-clause predicates the delegate must satisfy.
    pub(super) bounds: Vec<TokenStream2>,
}

pub(super) fn mount_contract(
    body_type: &Ident,
    shared_state: &SharedState,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
) -> MountContract {
    let mounts_type = mount_delegate_type();
    let state_type = &shared_state.ty;
    let body = quote! {
        <#mounts_type as #runtime::MountDelegate<#body_type, #state_type>>::Body
    };
    let data_bound = if heterogeneous_data {
        TokenStream2::new()
    } else {
        quote! { Data = #runtime::bytes::Bytes, }
    };
    MountContract {
        bounds: vec![
            quote! { #mounts_type: #runtime::MountDelegate<#body_type, #state_type> },
            quote! {
                #body: #runtime::http_body::Body<
                    #data_bound
                    Error: ::core::error::Error + 'static,
                >
            },
        ],
        body,
    }
}

/// Builds the capture list `mounted_response_return_type` closes over.
///
/// The list is what its `use<..>` bounds name.
///
/// This is only reached on the erased-mounts path, where `expand()` already
/// requires a fixed `state = StateType` contract — so `shared_state.generic`
/// (set only for a generic/dynamic-state router) is always `None` here, and
/// the `Some(state_type)` branch below can never run.
#[cfg_attr(coverage_nightly, coverage(off))]
fn mounted_response_captures(body_type: &Ident, shared_state: &SharedState) -> Vec<TokenStream2> {
    let mounts_type = mount_delegate_type();
    let mut captures = vec![quote! { #body_type }];
    if let Some(state_type) = &shared_state.generic {
        captures.push(quote! { #state_type });
    }
    captures.push(quote! { #mounts_type });
    captures
}

/// Builds the erased-mount route's response return type.
///
/// Every generic parameter the response body depends on is captured.
pub(super) fn mounted_response_return_type(
    body_type: &Ident,
    shared_state: &SharedState,
    heterogeneous_data: bool,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> TokenStream2 {
    let mounts_type = mount_delegate_type();
    let state_type = &shared_state.ty;
    let captures = mounted_response_captures(body_type, shared_state);
    let mount_body = quote! {
        <#mounts_type as #runtime::MountDelegate<#body_type, #state_type>>::Body
    };

    if heterogeneous_data {
        quote! {
            #response_runtime::Response<
                #response_runtime::DataEitherBody<
                    impl #runtime::http_body::Body<
                        Error = impl ::core::error::Error + use<#(#captures),*>,
                    > + use<#(#captures),*>,
                    #mount_body,
                >
            >
        }
    } else {
        quote! {
            #response_runtime::Response<
                #response_runtime::EitherBody<
                    impl #runtime::http_body::Body<
                        Data = #runtime::bytes::Bytes,
                        Error = impl ::core::error::Error + use<#(#captures),*>,
                    > + use<#(#captures),*>,
                    #mount_body,
                >
            >
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use quote::quote;
    use syn::parse_quote;

    use super::*;
    use crate::macro_impl::router::expand;
    use crate::macro_impl::router::test_support::expand_fixed_router;

    #[test]
    fn generic_router_rejects_an_erased_mount_contract() {
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
            true,
            false,
        )
        .expect_err("erased mounts require fixed state");

        assert!(error.to_string().contains("requires a fixed `state"), "{error}");
    }

    #[test]
    fn fixed_state_specializes_the_public_signature_bounds_and_opaque_capture() {
        let state: Type = syn::parse_quote!(self::state::AppState<super::Dependency>);
        let generated = expand_fixed_router(
            state,
            quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self, projected: State<Projected>) -> Response {
                        response(projected)
                    }
                }
            },
        )
        .expect("a qualified concrete state type is valid");
        let code = generated.to_string();

        assert!(
            code.contains("state : & self :: state :: AppState < super :: Dependency >"),
            "{code}"
        );
        assert!(
            !code.contains("FromRequestParts < '__routerama_request"),
            "a fixed state resolves the extractor contract concretely: {code}"
        );
        assert!(
            code.contains("type __RouteramaFixedStateContract = self :: state :: AppState < super :: Dependency >"),
            "the impl-local state alias preserves both `self` and `super`: {code}"
        );
        assert!(
            code.contains("for < '__routerama_witness > State < Projected >")
                && code.contains("__RouteramaFixedStateContract , Rejection = __RouteramaRejection"),
            "the parts extractor is validated against the fixed alias: {code}"
        );
        assert!(!code.contains("__RouteramaState"), "{code}");
        assert!(
            code.contains("use < __RouteramaBody >"),
            "fixed state is not an inference-only opaque capture: {code}"
        );
        assert!(
            !code.contains("host_matches"),
            "state specialization adds no runtime policy: {code}"
        );
        assert!(!code.contains("BoxBody"), "state specialization alone adds no body erasure: {code}");
        assert_eq!(
            code.matches("__routerama_validate_fixed_state").count(),
            1,
            "the dead validation function is declared but never called by routing: {code}"
        );
    }

    #[test]
    fn configured_dynamic_route_uses_the_same_fixed_state_contract() {
        let generated = expand_fixed_router(
            syn::parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(dynamic)]
                    async fn plugin(&self, value: Extractor) -> Response {
                        response(value)
                    }
                }
            },
        )
        .expect("a fixed-state dynamic service is valid");
        let code = generated.to_string();

        assert!(code.contains("struct ApiRouter"), "{code}");
        assert!(code.contains("state : & AppState"), "{code}");
        assert!(
            code.contains("FromRequestParts < '__routerama_witness , __RouteramaFixedStateContract"),
            "{code}"
        );
        assert!(code.contains("FromRequestParts < '_ , AppState >"), "{code}");
        assert!(!code.contains("__RouteramaState"), "{code}");
    }

    #[test]
    fn fixed_state_validation_names_do_not_collide_with_service_items() {
        let generated = expand_fixed_router(
            syn::parse_quote!(__RouteramaFixedStateContract),
            quote! {
                impl Api {
                    fn __routerama_validate_fixed_state() {}

                    #[route(GET, "/")]
                    async fn home(&self, value: Extractor) -> Response {
                        response(value)
                    }
                }
            },
        )
        .expect("generated validation names are made unique");
        let code = generated.to_string();

        assert!(code.contains("fn __routerama_validate_fixed_state ()"), "{code}");
        assert!(code.contains("fn ___routerama_validate_fixed_state ()"), "{code}");
        assert!(
            code.contains("type ___RouteramaFixedStateContract = __RouteramaFixedStateContract"),
            "{code}"
        );
    }

    #[test]
    fn fixed_state_validation_skips_non_fn_items_when_choosing_a_validation_method_name() {
        // The impl carries a non-Fn, non-Const item (a type alias) ahead of a
        // `const` item that collides with the default validation-method
        // name, so the name-search loop exercises its `Const`, `Fn`, and
        // catch-all match arms in the same run.
        let generated = expand_fixed_router(
            syn::parse_quote!(AppState),
            quote! {
                impl Api {
                    type Marker = ();
                    const __routerama_validate_fixed_state: i32 = 0;

                    #[route(GET, "/")]
                    async fn home(&self) -> StatusCode {
                        StatusCode::NO_CONTENT
                    }
                }
            },
        )
        .expect("a colliding const item does not block validation-method naming");
        let code = generated.to_string();
        assert!(code.contains("fn ___routerama_validate_fixed_state ()"), "{code}");
    }

    #[test]
    fn fixed_state_validation_checks_policy_response_types_for_a_state_alias_collision() {
        // A fixed-state router with both a catcher and a fallback exercises
        // the state-alias-collision check's policy branch, which chains the
        // fallback, catcher rejection, catcher extractor, and catcher
        // response types together.
        let generated = expand_fixed_router(
            syn::parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self, value: Extractor) -> Response {
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
            },
        )
        .expect("a fixed-state router supports catchers and a fallback");
        let code = generated.to_string();
        assert!(code.contains("__RouteramaFixedStateContract"), "{code}");
        assert!(code.contains("self . catch"), "{code}");
        assert!(code.contains("self . fallback"), "{code}");
    }

    #[test]
    fn fixed_state_validation_skips_a_streaming_transform_body_witness() {
        // A streaming `#[transform(stream, ...)]` replaces the transport body
        // with a generic, so the fixed-state witness generation cannot check
        // it eagerly and instead skips it via `continue`.
        let generated = expand_fixed_router(
            syn::parse_quote!(AppState),
            quote! {
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
            },
        )
        .expect("a fixed-state router supports a streaming transform");
        let code = generated.to_string();
        assert!(code.contains("__RouteramaFixedStateContract"), "{code}");
    }

    #[test]
    fn fixed_state_validation_checks_a_buffered_transform_replacement_body_and_a_catcher() {
        // A buffered `#[transform(limit = N, ...)]` supplies a concrete
        // replacement body type under a fixed-state witness generation.
        let generated = expand_fixed_router(
            syn::parse_quote!(AppState),
            quote! {
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
            },
        )
        .expect("a fixed-state router supports a buffered transform");
        let code = generated.to_string();
        assert!(code.contains("__RouteramaFixedStateContract"), "{code}");
    }

    #[test]
    fn fixed_state_validation_checks_an_explicit_body_catcher() {
        // An explicit `#[catch(... from = ...)]` bound to a `#[body]`
        // extractor, with no transform diverting the body witness, exercises
        // the body-catcher lookup branch under a fixed-state witness
        // generation.
        let generated = expand_fixed_router(
            syn::parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(POST, "/")]
                    async fn create(&self, #[body] data: BytesBody<64>) -> StatusCode {
                        response(data)
                    }

                    #[catch(Rejection, from = BytesBody<64>)]
                    async fn catch(&self, rejection: Rejection) -> CatchResponse {
                        catch_response(rejection)
                    }
                }
            },
        )
        .expect("a fixed-state router supports an explicit body catcher");
        let code = generated.to_string();
        assert!(code.contains("__RouteramaFixedStateContract"), "{code}");
    }

    #[test]
    fn a_fixed_state_router_with_a_tuple_response_type_expands() {
        let generated = expand_fixed_router(
            parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self) -> (StatusCode, String) {
                        home()
                    }
                }
            },
        )
        .expect("a fixed-state router with a tuple response type is valid");
        let code = generated.to_string();
        assert!(code.contains("self . home ()"), "{code}");
    }

    // --- RequestPartsLifetime rewriting a for<..> higher-ranked trait bound extractor ---
}
