// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Catcher and interceptor binding plus type-inspection utilities.

use alloc::format;
use alloc::string::{String, ToString as _};
use alloc::vec::Vec;

use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::quote;
use syn::spanned::Spanned as _;
use syn::{Error, LitStr, Type};

use super::model::{Argument, CatchBinding, CatcherPolicy, ExtractionKind, Handler, RoutingPolicy, TransformMode};
use crate::macro_impl::RouteTarget;

/// Returns the terminal type name behind `&mut Name<...>`.
pub(super) fn mutable_reference_name(ty: &Type) -> Option<String> {
    let Type::Reference(reference) = ty else {
        return None;
    };
    reference.mutability?;
    terminal_type_name(&reference.elem)
}

/// Returns the terminal type name behind `&Name<...>`.
pub(super) fn shared_reference_name(ty: &Type) -> Option<String> {
    let Type::Reference(reference) = ty else {
        return None;
    };
    if reference.mutability.is_some() {
        return None;
    }
    terminal_type_name(&reference.elem)
}

pub(super) fn bind_catchers(policy: &mut RoutingPolicy, handlers: &[Handler]) -> syn::Result<()> {
    let sites: Vec<(ExtractionKind, &Type)> = handlers
        .iter()
        .flat_map(|handler| &handler.arguments)
        .filter_map(|argument| match argument {
            Argument::Parts(_, ty) => Some((ExtractionKind::Parts, ty)),
            Argument::Body(_, ty) => Some((ExtractionKind::Body, ty)),
            Argument::Capture(_) => None,
        })
        .collect();

    for (catcher_index, catcher) in policy.catchers.iter().enumerate() {
        let mut matched = false;
        for (kind, extractor) in &sites {
            let applies = if let Some(explicit) = &catcher.extractor_type {
                type_key(explicit) == type_key(extractor)
            } else {
                inferred_catcher_applies(&catcher.rejection_type, extractor)
            };
            if !applies {
                continue;
            }
            matched = true;
            let extractor_key = type_key(extractor);
            if let Some(previous) = policy
                .bindings
                .iter()
                .find(|binding| binding.kind == *kind && binding.extractor_key == extractor_key)
            {
                if previous.catcher != catcher_index {
                    return Err(Error::new(
                        catcher.span,
                        format!(
                            "catchers for `{}` are ambiguous; use `from = ExtractorType` to select one exact rejection policy",
                            quote! { #extractor }
                        ),
                    ));
                }
                continue;
            }
            policy.bindings.push(CatchBinding {
                kind: *kind,
                extractor_key,
                catcher: catcher_index,
            });
        }
        if !matched {
            let detail = catcher.extractor_type.as_ref().map_or_else(
                || {
                    "no built-in extractor in this service has that rejection type; custom extractors require `from = ExtractorType`"
                        .to_string()
                },
                |extractor| format!("the service has no extractor of type `{}`", quote! { #extractor }),
            );
            return Err(Error::new(catcher.span, format!("unused extractor catcher: {detail}")));
        }
    }
    Ok(())
}

pub(super) fn bind_interceptors(policy: &RoutingPolicy, handlers: &[Handler]) -> syn::Result<()> {
    let handler_for = |name: &Ident| handlers.iter().find(|handler| &handler.method == name);

    for before in &policy.befores {
        let Some(names) = &before.handlers else {
            continue;
        };
        for name in names {
            if handler_for(name).is_none() {
                return Err(Error::new(
                    name.span(),
                    format!("`#[before]` names `{name}`, which is not a `#[route]` handler in this service"),
                ));
            }
        }
    }

    for after in &policy.afters {
        if let Some(names) = &after.handlers {
            for name in names {
                if handler_for(name).is_none() {
                    return Err(Error::new(
                        name.span(),
                        format!("`#[after]` names `{name}`, which is not a `#[route]` handler in this service"),
                    ));
                }
            }
        }
    }

    let mut covered: Vec<(&Ident, &Ident)> = Vec::new();
    for transform in &policy.transforms {
        for name in &transform.handlers {
            let Some(handler) = handler_for(name) else {
                return Err(Error::new(
                    name.span(),
                    format!("`#[transform]` names `{name}`, which is not a `#[route]` handler in this service"),
                ));
            };
            if let Some((_, previous)) = covered.iter().find(|(covered_handler, _)| *covered_handler == name) {
                let mut error = Error::new(
                    name.span(),
                    format!("handler `{name}` is transformed more than once; a request body can be replaced only once"),
                );
                error.combine(Error::new(previous.span(), "the first transform for this handler is declared here"));
                return Err(error);
            }
            covered.push((name, &transform.method));
            let has_body = handler.arguments.iter().any(|argument| matches!(argument, Argument::Body(_, _)));
            if has_body && transform.replacement_body.is_none() {
                let replacement = match transform.mode {
                    TransformMode::Buffered { .. } => {
                        "return `BodyTransform<B, R>` with a concrete replacement body for `#[body]` extraction"
                    }
                    TransformMode::Streaming => {
                        "return `BodyTransform<Wrapper<B>, R>` with a replacement that wraps the transport body for `#[body]` extraction"
                    }
                };
                return Err(Error::new(
                    transform.span,
                    format!(
                        "handler `{name}` declares a `#[body]` parameter, but its `#[transform]` consumes the body without a \
                         replacement, so there is nothing left to extract; {replacement}"
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn catcher_for<'a>(policy: &'a RoutingPolicy, kind: ExtractionKind, extractor: &Type) -> Option<&'a CatcherPolicy> {
    let key = type_key(extractor);
    policy
        .bindings
        .iter()
        .find(|binding| binding.kind == kind && binding.extractor_key == key)
        .map(|binding| &policy.catchers[binding.catcher])
}

fn inferred_catcher_applies(rejection: &Type, extractor: &Type) -> bool {
    let Some(rejection) = terminal_type_name(rejection) else {
        return false;
    };
    let Some(extractor) = terminal_type_name(extractor) else {
        return false;
    };
    matches!(
        (rejection.as_str(), extractor.as_str()),
        ("MissingExtension", "ExtensionRef" | "ClonedExtension")
            | ("QueryRejection", "Query")
            | ("BodyRejection", "BytesBody" | "TextBody")
            | ("JsonRejection", "Json")
            | ("FormRejection", "Form")
    )
}

pub(super) fn terminal_type_name(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    path.path.segments.last().map(|segment| segment.ident.to_string())
}

pub(super) fn default_body_witness_rejection(extractor: &Type, runtime: &TokenStream2) -> TokenStream2 {
    match terminal_type_name(extractor).as_deref() {
        Some("RawBody") => quote! { ::core::convert::Infallible },
        Some("BytesBody" | "TextBody") => {
            quote! { #runtime::BodyRejection<::core::convert::Infallible> }
        }
        Some("Json") => {
            quote! { #runtime::JsonRejection<::core::convert::Infallible> }
        }
        Some("Form") => {
            quote! { #runtime::FormRejection<::core::convert::Infallible> }
        }
        Some(_) | None => quote! { _ },
    }
}

pub(super) fn type_key(ty: &Type) -> String {
    quote! { #ty }.to_string()
}

/// Picks a diagnostic span for a `#[catch(RejectionType)]` mismatch error.
///
/// This is only called on a catcher/fallback argument type that
/// `validate_policy_receiver`-style validation already required to be passed
/// by value, rejecting `Type::Reference` up front with "routing policy
/// arguments are passed by value". So the `Type::Reference` arm here is
/// unreachable from its only call site; it is kept only so this function
/// stays total over every `Type` variant that could ever be passed to it.
#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn type_diagnostic_span(ty: &Type) -> Span {
    match ty {
        Type::Path(path) => path.path.segments.last().map_or_else(|| ty.span(), |segment| segment.ident.span()),
        Type::Reference(reference) => reference.and_token.span(),
        _ => ty.span(),
    }
}

/// Destructures a `RouteTarget` known to belong to a `HandlerKind::Static` handler.
///
/// `route_declaration` (called by `route_kind` while building every `Handler`)
/// only ever returns `HandlerKind::Static` when none of the handler's
/// `#[route]` attributes parsed to `RouteTarget::Dynamic` — a lone or combined
/// `RouteTarget::Dynamic` either yields `HandlerKind::Dynamic` or is rejected
/// outright with "`#[route(dynamic)]` cannot be combined with another route
/// attribute". So every call site that already established `kind ==
/// HandlerKind::Static` can never observe `RouteTarget::Dynamic` here; the
/// `Dynamic` arm is unreachable and kept only to avoid a non-exhaustive match.
#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn static_route_target(target: RouteTarget, span: Span, message: &str) -> syn::Result<(String, LitStr)> {
    match target {
        RouteTarget::Static { method, path } => Ok((method, path)),
        RouteTarget::Dynamic => Err(Error::new(span, message)),
    }
}

pub(super) fn combine_error(slot: &mut Option<Error>, next: Error) {
    if let Some(error) = slot {
        error.combine(next);
    } else {
        *slot = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use quote::quote;
    use syn::parse_quote;

    use super::*;
    use crate::macro_impl::router::test_support::{expand_mounted_heterogeneous_router, expand_router};

    #[test]
    fn a_catcher_matches_a_parts_extraction_rejection() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, value: Query) -> StatusCode {
                    response(value)
                }

                #[catch(QueryRejection, from = Query)]
                async fn catch(&self, rejection: QueryRejection) -> CatchResponse {
                    catch_response(rejection)
                }
            }
        })
        .expect("a matching catcher is valid");
        let code = generated.to_string();
        assert!(code.contains("self . catch (rejection) . await"), "{code}");
    }

    #[test]
    fn a_catcher_matches_a_body_extraction_rejection() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self, #[body] value: Json) -> StatusCode {
                    response(value)
                }

                #[catch(JsonRejection, from = Json)]
                async fn catch(&self, rejection: JsonRejection) -> CatchResponse {
                    catch_response(rejection)
                }
            }
        })
        .expect("a matching body catcher is valid");
        let code = generated.to_string();
        assert!(code.contains("self . catch (rejection) . await"), "{code}");
    }

    #[test]
    fn an_unused_catcher_without_from_is_rejected() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[catch(Rejection)]
                async fn catch(&self, rejection: Rejection) -> StatusCode {
                    catch(rejection)
                }
            }
        })
        .expect_err("a catcher with no matching extractor is rejected");
        assert!(
            error
                .to_string()
                .contains("no built-in extractor in this service has that rejection type"),
            "{error}"
        );
    }

    #[test]
    fn an_unused_catcher_with_from_is_rejected() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[catch(Rejection, from = Extractor)]
                async fn catch(&self, rejection: Rejection) -> StatusCode {
                    catch(rejection)
                }
            }
        })
        .expect_err("a catcher with an unmatched extractor is rejected");
        assert!(
            error.to_string().contains("the service has no extractor of type `Extractor`"),
            "{error}"
        );
    }

    // --- direct-call tests for small pure helpers ---

    #[test]
    fn type_diagnostic_span_handles_every_type_variant() {
        let path_ty: Type = parse_quote!(RouteFailure<'_>);
        let _ = type_diagnostic_span(&path_ty);

        let reference_ty: Type = parse_quote!(&'static str);
        let _ = type_diagnostic_span(&reference_ty);

        let tuple_ty: Type = parse_quote!((u32, u32));
        let _ = type_diagnostic_span(&tuple_ty);
    }

    #[test]
    fn static_route_target_destructures_both_variants() {
        let path: LitStr = parse_quote!("/books");
        let target = RouteTarget::Static {
            method: "GET".to_string(),
            path: path.clone(),
        };
        let (method, resolved_path) = static_route_target(target, path.span(), "unreachable").expect("a static target resolves");
        assert_eq!(method, "GET");
        assert_eq!(resolved_path.value(), "/books");

        let Err(error) = static_route_target(RouteTarget::Dynamic, path.span(), "dynamic targets are rejected here") else {
            panic!("a dynamic target is rejected");
        };
        assert!(error.to_string().contains("dynamic targets are rejected here"), "{error}");
    }

    #[test]
    fn terminal_type_name_returns_none_for_non_path_types() {
        let tuple_ty: Type = parse_quote!((u32, u32));
        assert_eq!(terminal_type_name(&tuple_ty), None);

        let path_ty: Type = parse_quote!(alloc::string::String);
        assert_eq!(terminal_type_name(&path_ty).as_deref(), Some("String"));
    }

    #[test]
    fn inferred_catcher_applies_matches_every_known_pair_and_rejects_others() {
        let pairs = [
            ("MissingExtension", "ExtensionRef", true),
            ("MissingExtension", "ClonedExtension", true),
            ("QueryRejection", "Query", true),
            ("BodyRejection", "BytesBody", true),
            ("BodyRejection", "TextBody", true),
            ("JsonRejection", "Json", true),
            ("FormRejection", "Form", true),
            ("QueryRejection", "Json", false),
            ("Unrelated", "Query", false),
        ];
        for (rejection, extractor, expected) in pairs {
            let rejection_ty: Type = syn::parse_str(rejection).expect("a valid rejection type name");
            let extractor_ty: Type = syn::parse_str(extractor).expect("a valid extractor type name");
            assert_eq!(
                inferred_catcher_applies(&rejection_ty, &extractor_ty),
                expected,
                "{rejection} / {extractor}"
            );
        }
        let non_path: Type = parse_quote!((u32, u32));
        let query_ty: Type = parse_quote!(Query);
        assert!(!inferred_catcher_applies(&non_path, &query_ty));
        assert!(!inferred_catcher_applies(&query_ty, &non_path));
    }

    #[test]
    fn default_body_witness_rejection_covers_every_known_extractor() {
        let runtime = quote! { ::routerama::__private::route };
        for (extractor, expected) in [
            ("RawBody", "Infallible"),
            ("BytesBody", "BodyRejection"),
            ("TextBody", "BodyRejection"),
            ("Json", "JsonRejection"),
            ("Form", "FormRejection"),
            ("SomethingElse", "_"),
        ] {
            let ty: Type = syn::parse_str(extractor).expect("a valid extractor type name");
            let rejection = default_body_witness_rejection(&ty, &runtime).to_string();
            assert!(rejection.contains(expected), "{extractor}: {rejection}");
        }
    }

    #[test]
    fn type_key_renders_the_type_tokens() {
        let ty: Type = parse_quote!(Vec<u32>);
        assert_eq!(type_key(&ty), quote! { Vec<u32> }.to_string());
    }

    #[test]
    fn catcher_for_returns_none_without_a_matching_binding() {
        let policy = RoutingPolicy::default();
        let ty: Type = parse_quote!(Query);
        assert!(catcher_for(&policy, ExtractionKind::Parts, &ty).is_none());
    }

    // --- handler parameter parsing errors ---

    #[test]
    fn a_body_extraction_without_a_catcher_binds_the_default_rejection() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(&self, #[body] data: BytesBody<64>) -> StatusCode {
                    response(data)
                }
            }
        })
        .expect("an uncaught body extraction is valid");
        let code = generated.to_string();
        assert!(code.contains("FromRequestBody"), "{code}");
    }

    #[test]
    fn a_heterogeneous_mounted_router_binds_body_and_parts_extraction() {
        let generated = expand_mounted_heterogeneous_router(
            parse_quote!(AppState),
            quote! {
                impl Api {
                    #[route(POST, "/")]
                    async fn create(&self, value: Query, #[body] data: BytesBody<64>) -> ViewResponse {
                        response(value, data)
                    }
                }
            },
        )
        .expect("a heterogeneous mounted router with body and parts extraction is valid");
        let code = generated.to_string();
        assert!(code.contains("FromRequestBody"), "{code}");
        assert!(code.contains("FromRequestParts"), "{code}");
    }

    // --- service_name and CatchAttr grammar ---

    #[test]
    fn an_inferred_catcher_without_from_matches_by_rejection_name() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, value: Query) -> StatusCode {
                    response(value)
                }

                #[catch(QueryRejection)]
                async fn catch(&self, rejection: QueryRejection) -> CatchResponse {
                    catch_response(rejection)
                }
            }
        })
        .expect("an inferred catcher without `from` matches the `Query` extractor by name");
        let code = generated.to_string();
        assert!(code.contains("self . catch (rejection) . await"), "{code}");
    }

    #[test]
    fn a_catcher_that_does_not_apply_to_every_site_is_skipped_for_the_others() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/query")]
                async fn query(&self, value: Query) -> StatusCode {
                    response(value)
                }

                #[route(POST, "/json")]
                async fn create(&self, value: Json) -> StatusCode {
                    response(value)
                }

                #[catch(QueryRejection, from = Query)]
                async fn catch(&self, rejection: QueryRejection) -> CatchResponse {
                    catch_response(rejection)
                }
            }
        })
        .expect("a catcher not applying to one of several extraction sites is skipped for that site");
        let code = generated.to_string();
        assert!(code.contains("self . catch (rejection) . await"), "{code}");
    }

    #[test]
    fn the_same_catcher_applying_to_two_identical_extraction_sites_is_bound_once() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/one")]
                async fn one(&self, value: Query) -> StatusCode {
                    response(value)
                }

                #[route(GET, "/two")]
                async fn two(&self, value: Query) -> StatusCode {
                    response(value)
                }

                #[catch(QueryRejection, from = Query)]
                async fn catch(&self, rejection: QueryRejection) -> CatchResponse {
                    catch_response(rejection)
                }
            }
        })
        .expect("the same catcher reused across two identical extraction sites binds once without conflict");
        let code = generated.to_string();
        assert!(code.contains("self . catch (rejection) . await"), "{code}");
    }

    #[test]
    fn two_catchers_matching_the_same_extraction_site_are_ambiguous() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, value: Query) -> StatusCode {
                    response(value)
                }

                #[catch(QueryRejection, from = Query)]
                async fn catch_one(&self, rejection: QueryRejection) -> CatchResponse {
                    catch_response(rejection)
                }

                #[catch(AnotherQueryRejection, from = Query)]
                async fn catch_two(&self, rejection: AnotherQueryRejection) -> CatchResponse {
                    catch_response(rejection)
                }
            }
        })
        .expect_err("two distinct catchers both matching the same extraction site are ambiguous");
        assert!(error.to_string().contains("are ambiguous; use `from = ExtractorType`"), "{error}");
    }

    // --- transform body-consumption / replacement-body edge cases ---
}
