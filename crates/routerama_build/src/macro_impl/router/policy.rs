// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Parsing and validation of `#[fallback]` and `#[catch]` policy methods.

use alloc::format;
use alloc::vec::Vec;

use proc_macro2::Ident;
use quote::quote;
use syn::spanned::Spanned as _;
use syn::visit::Visit as _;
use syn::{Error, FnArg, GenericArgument, ImplItem, ImplItemFn, ItemImpl, Lifetime, PathArguments, Token, Type};

use super::binding::{type_diagnostic_span, type_key};
use super::interceptor::parse_interceptor;
use super::model::{CatcherPolicy, FallbackPolicy, RoutingPolicy};
use super::parse::{is_parameter_marker, parameter_pattern, response_type};

struct CatchAttr {
    rejection_type: Type,
    extractor_type: Option<Type>,
}

impl syn::parse::Parse for CatchAttr {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let rejection_type = input.parse()?;
        let mut extractor_type = None;
        if !input.is_empty() {
            let _comma: Token![,] = input.parse()?;
            if input.is_empty() {
                return Err(input.error("expected `from = ExtractorType` after the comma"));
            }
            let key: Ident = input.parse()?;
            if key != "from" {
                return Err(Error::new(key.span(), "expected `from = ExtractorType`"));
            }
            let _equals: Token![=] = input.parse()?;
            extractor_type = Some(input.parse()?);
            let _trailing_comma = input.parse::<Option<Token![,]>>()?;
        }
        if !input.is_empty() {
            return Err(input.error("unexpected catcher argument"));
        }
        Ok(Self {
            rejection_type,
            extractor_type,
        })
    }
}

pub(super) fn parse_policy(item: &ItemImpl) -> syn::Result<RoutingPolicy> {
    let mut policy = RoutingPolicy::default();
    for impl_item in &item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        if parse_interceptor(&mut policy, method)? {
            continue;
        }
        let fallback_attrs: Vec<_> = method
            .attrs
            .iter()
            .filter(|attribute| attribute.path().is_ident("fallback"))
            .collect();
        let catch_attrs: Vec<_> = method.attrs.iter().filter(|attribute| attribute.path().is_ident("catch")).collect();
        if fallback_attrs.is_empty() && catch_attrs.is_empty() {
            continue;
        }
        if fallback_attrs.len() > 1 || catch_attrs.len() > 1 {
            return Err(Error::new(
                method.sig.ident.span(),
                "a routing policy method may have only one `#[fallback]` or `#[catch(...)]` annotation",
            ));
        }
        if !fallback_attrs.is_empty() && !catch_attrs.is_empty() {
            return Err(Error::new(
                method.sig.ident.span(),
                "a routing policy method cannot be both a fallback and an extractor catcher",
            ));
        }

        let (argument_type, response_type) = validate_policy_signature(method)?;
        if let Some(attribute) = fallback_attrs.first() {
            if !matches!(attribute.meta, syn::Meta::Path(_)) {
                return Err(Error::new(attribute.path().span(), "`#[fallback]` does not accept arguments"));
            }
            validate_route_failure_type(argument_type)?;
            if let Some(previous) = &policy.fallback {
                let mut error = Error::new(
                    attribute.path().span(),
                    "a generated service may declare only one `#[fallback]` method",
                );
                error.combine(Error::new(previous.method.span(), "the first fallback is declared here"));
                return Err(error);
            }
            policy.fallback = Some(FallbackPolicy {
                method: method.sig.ident.clone(),
                response_type,
            });
        } else {
            let attribute = catch_attrs[0];
            let parsed: CatchAttr = attribute.parse_args()?;
            validate_concrete_rejection_type(&parsed.rejection_type)?;
            if type_key(&parsed.rejection_type) != type_key(argument_type) {
                return Err(Error::new(
                    type_diagnostic_span(argument_type),
                    "the `#[catch(RejectionType)]` argument must exactly match the catcher's by-value parameter type",
                ));
            }
            if let Some(previous) = policy
                .catchers
                .iter()
                .find(|catcher| type_key(&catcher.rejection_type) == type_key(&parsed.rejection_type))
            {
                let rejection_type = &parsed.rejection_type;
                let mut error = Error::new(
                    attribute.path().span(),
                    format!("duplicate catcher for rejection type `{}`", quote! { #rejection_type }),
                );
                error.combine(Error::new(previous.span, "the first catcher for this type is declared here"));
                return Err(error);
            }
            policy.catchers.push(CatcherPolicy {
                method: method.sig.ident.clone(),
                rejection_type: parsed.rejection_type,
                extractor_type: parsed.extractor_type,
                response_type,
                span: attribute.path().span(),
            });
        }
    }
    Ok(policy)
}

fn validate_policy_signature(method: &ImplItemFn) -> syn::Result<(&Type, Type)> {
    let policy_name = if method.attrs.iter().any(|attribute| attribute.path().is_ident("fallback")) {
        "fallback"
    } else {
        "catcher"
    };
    if method.sig.asyncness.is_none() {
        return Err(Error::new(
            method.sig.fn_token.span(),
            format!("routing {policy_name} methods must be async"),
        ));
    }
    if method.sig.constness.is_some() || matches!(method.sig.safety, syn::Safety::Unsafe(_)) || method.sig.abi.is_some() {
        return Err(Error::new(
            method.sig.span(),
            format!("routing {policy_name} methods cannot be const, unsafe, or extern functions"),
        ));
    }
    if !method.sig.generics.params.is_empty() || method.sig.generics.where_clause.is_some() {
        let span = method
            .sig
            .generics
            .params
            .first()
            .map_or_else(|| method.sig.generics.span(), syn::spanned::Spanned::span);
        return Err(Error::new(
            span,
            format!("routing {policy_name} methods cannot have generic parameters"),
        ));
    }
    if let Some(attribute) = method
        .attrs
        .iter()
        .find(|attribute| attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr"))
    {
        return Err(Error::new(
            attribute.span(),
            "routing policy methods cannot be conditionally compiled",
        ));
    }
    let mut inputs = method.sig.inputs.iter();
    let Some(FnArg::Receiver(receiver)) = inputs.next() else {
        return Err(Error::new(
            method.sig.inputs.span(),
            format!("routing {policy_name} methods must begin with `&self`"),
        ));
    };
    if !matches!(receiver.kind, syn::ReceiverKind::Reference(_, _, None)) || receiver.mutability.is_some() {
        return Err(Error::new(
            receiver.span(),
            format!("routing {policy_name} methods must begin with `&self`"),
        ));
    }
    if let Some(attribute) = receiver.attrs.iter().find(|attribute| is_parameter_marker(attribute)) {
        return Err(Error::new(
            attribute.span(),
            "routing policy receivers cannot use route extraction markers",
        ));
    }
    let Some(FnArg::Typed(argument)) = inputs.next() else {
        return Err(Error::new(
            method.sig.inputs.span(),
            format!("routing {policy_name} methods require exactly one policy argument after `&self`"),
        ));
    };
    if inputs.next().is_some() {
        return Err(Error::new(
            method.sig.inputs.span(),
            format!("routing {policy_name} methods accept only `&self` and one policy argument"),
        ));
    }
    parameter_pattern(&argument.pat)?;
    if let Some(attribute) = argument.attrs.iter().find(|attribute| is_parameter_marker(attribute)) {
        return Err(Error::new(
            attribute.path().span(),
            "routing policy arguments cannot use `#[body]` or `#[capture]`; catchers cannot recursively extract a request",
        ));
    }
    if let Type::Reference(reference) = argument.ty.as_ref() {
        return Err(Error::new(
            reference.and_token.span(),
            "routing policy arguments are passed by value",
        ));
    }
    let response_type = response_type(method)?;
    Ok((argument.ty.as_ref(), response_type))
}

fn validate_route_failure_type(ty: &Type) -> syn::Result<()> {
    let Type::Path(path) = ty else {
        return Err(Error::new(
            ty.span(),
            "a `#[fallback]` argument must be `RouteFailure<'_>` by value",
        ));
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
        return Err(Error::new(
            ty.span(),
            "a `#[fallback]` argument must be `RouteFailure<'_>` by value",
        ));
    };
    if segment.ident != "RouteFailure"
        || arguments.args.len() != 1
        || !matches!(arguments.args.first(), Some(GenericArgument::Lifetime(lifetime)) if lifetime.ident == "_")
    {
        return Err(Error::new(
            ty.span(),
            "a `#[fallback]` argument must be `RouteFailure<'_>` by value",
        ));
    }
    Ok(())
}

#[derive(Default)]
struct ConcreteRejectionType {
    error: Option<Error>,
}

impl<'ast> syn::visit::Visit<'ast> for ConcreteRejectionType {
    fn visit_type_reference(&mut self, i: &'ast syn::TypeReference) {
        if self.error.is_none() {
            self.error = Some(Error::new(
                i.span(),
                "catcher rejection types must be owned and cannot depend on a borrowed lifetime",
            ));
        }
    }

    fn visit_type_infer(&mut self, i: &'ast syn::TypeInfer) {
        if self.error.is_none() {
            self.error = Some(Error::new(
                i.span(),
                "catcher rejection types must be fully concrete and cannot contain `_`",
            ));
        }
    }

    fn visit_type_impl_trait(&mut self, i: &'ast syn::TypeImplTrait) {
        if self.error.is_none() {
            self.error = Some(Error::new(
                i.span(),
                "catcher rejection types must be concrete and cannot use `impl Trait`",
            ));
        }
    }

    fn visit_type_macro(&mut self, i: &'ast syn::TypeMacro) {
        if self.error.is_none() {
            self.error = Some(Error::new(i.span(), "catcher rejection types cannot be produced by a type macro"));
        }
    }

    fn visit_lifetime(&mut self, i: &'ast Lifetime) {
        if i.ident != "static" && self.error.is_none() {
            self.error = Some(Error::new(
                i.span(),
                "catcher rejection types cannot depend on a non-static lifetime",
            ));
        }
    }
}

fn validate_concrete_rejection_type(ty: &Type) -> syn::Result<()> {
    let mut visitor = ConcreteRejectionType::default();
    visitor.visit_type(ty);
    match visitor.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use quote::quote;

    use crate::macro_impl::router::test_support::expand_router;

    #[test]
    fn a_policy_method_rejects_both_fallback_and_catch_attributes() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                #[catch(Rejection)]
                async fn policy(&self, value: Rejection) -> StatusCode {
                    policy(value)
                }
            }
        })
        .expect_err("a method cannot be both a fallback and a catcher");
        assert!(
            error.to_string().contains("cannot be both a fallback and an extractor catcher"),
            "{error}"
        );
    }

    #[test]
    fn a_fallback_method_rejects_arguments_on_the_attribute() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback(extra)]
                async fn fallback(&self, failure: RouteFailure<'_>) -> StatusCode {
                    fallback(failure)
                }
            }
        })
        .expect_err("`#[fallback]` takes no arguments");
        assert!(error.to_string().contains("does not accept arguments"), "{error}");
    }

    #[test]
    fn a_duplicate_fallback_is_rejected() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                async fn first(&self, failure: RouteFailure<'_>) -> StatusCode {
                    first(failure)
                }

                #[fallback]
                async fn second(&self, failure: RouteFailure<'_>) -> StatusCode {
                    second(failure)
                }
            }
        })
        .expect_err("only one fallback is permitted");
        assert!(error.to_string().contains("only one `#[fallback]` method"), "{error}");
    }

    #[test]
    fn a_catch_argument_must_match_the_catcher_parameter_type() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, value: Extractor) -> StatusCode {
                    response(value)
                }

                #[catch(OtherRejection)]
                async fn catch(&self, rejection: Rejection) -> StatusCode {
                    catch(rejection)
                }
            }
        })
        .expect_err("a catch argument mismatch is rejected");
        assert!(
            error
                .to_string()
                .contains("must exactly match the catcher's by-value parameter type"),
            "{error}"
        );
    }

    #[test]
    fn a_duplicate_catcher_for_the_same_rejection_type_is_rejected() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/", produces = "text/plain")]
                async fn home(&self, value: Extractor) -> StatusCode {
                    response(value)
                }

                #[catch(Rejection)]
                async fn first(&self, rejection: Rejection) -> StatusCode {
                    first(rejection)
                }

                #[catch(Rejection)]
                async fn second(&self, rejection: Rejection) -> StatusCode {
                    second(rejection)
                }
            }
        })
        .expect_err("duplicate catchers for one rejection type are rejected");
        assert!(error.to_string().contains("duplicate catcher for rejection type"), "{error}");
    }

    // --- interceptor attribute conflicts and scope parsing ---

    #[test]
    fn a_policy_method_must_be_async() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                fn fallback(&self, failure: RouteFailure<'_>) -> StatusCode {
                    fallback(failure)
                }
            }
        })
        .expect_err("a synchronous policy method is rejected");
        assert!(error.to_string().contains("routing fallback methods must be async"), "{error}");
    }

    #[test]
    fn a_policy_method_cannot_be_const_unsafe_or_extern() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                async unsafe fn fallback(&self, failure: RouteFailure<'_>) -> StatusCode {
                    fallback(failure)
                }
            }
        })
        .expect_err("an unsafe policy method is rejected");
        assert!(
            error.to_string().contains("cannot be const, unsafe, or extern functions"),
            "{error}"
        );
    }

    #[test]
    fn a_policy_method_cannot_declare_generic_parameters() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                async fn fallback<T>(&self, failure: RouteFailure<'_>) -> StatusCode {
                    fallback::<T>(failure)
                }
            }
        })
        .expect_err("a generic policy method is rejected");
        assert!(
            error
                .to_string()
                .contains("routing fallback methods cannot have generic parameters"),
            "{error}"
        );
    }

    #[test]
    fn a_policy_method_cannot_be_conditionally_compiled() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                #[cfg(test)]
                async fn fallback(&self, failure: RouteFailure<'_>) -> StatusCode {
                    fallback(failure)
                }
            }
        })
        .expect_err("a conditionally compiled policy method is rejected");
        assert!(
            error
                .to_string()
                .contains("routing policy methods cannot be conditionally compiled"),
            "{error}"
        );
    }

    #[test]
    fn a_policy_method_must_begin_with_a_shared_self_receiver() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                async fn fallback(&mut self, failure: RouteFailure<'_>) -> StatusCode {
                    fallback(failure)
                }
            }
        })
        .expect_err("a mutable self receiver on a policy method is rejected");
        assert!(
            error.to_string().contains("routing fallback methods must begin with `&self`"),
            "{error}"
        );
    }

    #[test]
    fn a_policy_method_requires_exactly_one_argument() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                async fn fallback(&self) -> StatusCode {
                    fallback()
                }
            }
        })
        .expect_err("a policy method with no argument is rejected");
        assert!(
            error.to_string().contains("require exactly one policy argument after `&self`"),
            "{error}"
        );
    }

    #[test]
    fn a_policy_method_rejects_a_second_argument() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                async fn fallback(&self, failure: RouteFailure<'_>, extra: u32) -> StatusCode {
                    fallback(failure, extra)
                }
            }
        })
        .expect_err("a policy method with two arguments is rejected");
        assert!(error.to_string().contains("accept only `&self` and one policy argument"), "{error}");
    }

    #[test]
    fn a_policy_method_argument_cannot_use_extraction_markers() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                async fn fallback(&self, #[capture] failure: RouteFailure<'_>) -> StatusCode {
                    fallback(failure)
                }
            }
        })
        .expect_err("policy method extraction markers are rejected");
        assert!(
            error.to_string().contains("catchers cannot recursively extract a request"),
            "{error}"
        );
    }

    #[test]
    fn a_policy_method_argument_is_passed_by_value() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                async fn fallback(&self, failure: &RouteFailure<'_>) -> StatusCode {
                    fallback(failure)
                }
            }
        })
        .expect_err("a by-reference policy argument is rejected");
        assert!(
            error.to_string().contains("routing policy arguments are passed by value"),
            "{error}"
        );
    }

    // --- `#[fallback]` argument type: `validate_route_failure_type` ---

    #[test]
    fn a_fallback_argument_must_be_a_named_path_type() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                async fn fallback(&self, failure: (u32, u32)) -> StatusCode {
                    fallback(failure)
                }
            }
        })
        .expect_err("a tuple type is not `RouteFailure<'_>`");
        assert!(error.to_string().contains("must be `RouteFailure<'_>` by value"), "{error}");
    }

    #[test]
    fn a_fallback_argument_must_have_angle_bracketed_generics() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                async fn fallback(&self, failure: RouteFailure) -> StatusCode {
                    fallback(failure)
                }
            }
        })
        .expect_err("`RouteFailure` without generics is rejected");
        assert!(error.to_string().contains("must be `RouteFailure<'_>` by value"), "{error}");
    }

    #[test]
    fn a_fallback_argument_must_use_the_elided_lifetime() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                async fn fallback(&self, failure: RouteFailure<'static>) -> StatusCode {
                    fallback(failure)
                }
            }
        })
        .expect_err("`RouteFailure<'static>` with a named lifetime is rejected");
        assert!(error.to_string().contains("must be `RouteFailure<'_>` by value"), "{error}");
    }

    // --- catcher rejection type concreteness ---

    #[test]
    fn a_catcher_rejection_type_cannot_be_a_reference() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[catch(Wrapper<&'static str>)]
                async fn catch(&self, rejection: Wrapper<&'static str>) -> StatusCode {
                    catch(rejection)
                }
            }
        })
        .expect_err("a reference rejection type is rejected");
        assert!(
            error.to_string().contains("must be owned and cannot depend on a borrowed lifetime"),
            "{error}"
        );
    }

    #[test]
    fn a_catcher_rejection_type_cannot_contain_an_inferred_type() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[catch(Vec<_>)]
                async fn catch(&self, rejection: Vec<_>) -> StatusCode {
                    catch(rejection)
                }
            }
        })
        .expect_err("an inferred type in a rejection type is rejected");
        assert!(
            error.to_string().contains("must be fully concrete and cannot contain `_`"),
            "{error}"
        );
    }

    #[test]
    fn a_catcher_rejection_type_cannot_use_impl_trait() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[catch(Box<impl core::fmt::Debug>)]
                async fn catch(&self, rejection: Box<impl core::fmt::Debug>) -> StatusCode {
                    catch(rejection)
                }
            }
        })
        .expect_err("`impl Trait` in a rejection type is rejected");
        assert!(
            error.to_string().contains("must be concrete and cannot use `impl Trait`"),
            "{error}"
        );
    }

    #[test]
    fn a_catcher_rejection_type_cannot_be_produced_by_a_type_macro() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[catch(some_macro!())]
                async fn catch(&self, rejection: some_macro!()) -> StatusCode {
                    catch(rejection)
                }
            }
        })
        .expect_err("a type macro in a rejection type is rejected");
        assert!(error.to_string().contains("cannot be produced by a type macro"), "{error}");
    }

    #[test]
    fn a_catcher_rejection_type_cannot_depend_on_a_non_static_lifetime() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[catch(Box<dyn for<'a> Tagged<'a>>)]
                async fn catch(&self, rejection: Box<dyn for<'a> Tagged<'a>>) -> StatusCode {
                    catch(rejection)
                }
            }
        })
        .expect_err("a non-static lifetime in a rejection type is rejected");
        assert!(error.to_string().contains("cannot depend on a non-static lifetime"), "{error}");
    }

    // --- bind_catchers: ambiguity and unused catchers ---

    #[test]
    fn a_catch_attribute_requires_from_after_a_trailing_comma() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[catch(Rejection,)]
                async fn catch(&self, rejection: Rejection) -> StatusCode {
                    catch(rejection)
                }
            }
        })
        .expect_err("a trailing comma without `from = ...` is rejected");
        assert!(
            error.to_string().contains("expected `from = ExtractorType` after the comma"),
            "{error}"
        );
    }

    #[test]
    fn a_catch_attribute_requires_the_from_keyword() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[catch(Rejection, extractor = Extractor)]
                async fn catch(&self, rejection: Rejection) -> StatusCode {
                    catch(rejection)
                }
            }
        })
        .expect_err("a key other than `from` is rejected");
        assert!(error.to_string().contains("expected `from = ExtractorType`"), "{error}");
    }

    #[test]
    fn a_catch_attribute_rejects_a_trailing_extra_argument() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[catch(Rejection, from = Extractor, extra)]
                async fn catch(&self, rejection: Rejection) -> StatusCode {
                    catch(rejection)
                }
            }
        })
        .expect_err("a trailing extra argument is rejected");
        assert!(error.to_string().contains("unexpected catcher argument"), "{error}");
    }

    // --- interceptor argument-count / receiver edge cases ---

    #[test]
    fn a_policy_method_rejects_two_fallback_attributes() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                #[fallback]
                async fn fallback(&self, failure: RouteFailure<'_>) -> StatusCode {
                    fallback(failure)
                }
            }
        })
        .expect_err("two `#[fallback]` attributes on one method are rejected");
        assert!(
            error
                .to_string()
                .contains("may have only one `#[fallback]` or `#[catch(...)]` annotation"),
            "{error}"
        );
    }

    #[test]
    fn a_policy_method_rejects_two_catch_attributes() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[catch(Rejection)]
                #[catch(OtherRejection)]
                async fn catch(&self, rejection: Rejection) -> StatusCode {
                    catch(rejection)
                }
            }
        })
        .expect_err("two `#[catch(...)]` attributes on one method are rejected");
        assert!(
            error
                .to_string()
                .contains("may have only one `#[fallback]` or `#[catch(...)]` annotation"),
            "{error}"
        );
    }

    // --- transform grammar: no handler names ---

    #[test]
    fn a_policy_method_without_any_arguments_is_rejected() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                async fn fallback() -> StatusCode {
                    fallback()
                }
            }
        })
        .expect_err("a fallback with zero arguments is rejected");
        assert!(
            error.to_string().contains("routing fallback methods must begin with `&self`"),
            "{error}"
        );
    }

    #[test]
    fn a_policy_receiver_cannot_use_a_route_extraction_marker() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> StatusCode {
                    response()
                }

                #[fallback]
                async fn fallback(#[body] &self, failure: RouteFailure<'_>) -> StatusCode {
                    fallback(failure)
                }
            }
        })
        .expect_err("a marker attribute on the policy receiver is rejected");
        assert!(
            error
                .to_string()
                .contains("routing policy receivers cannot use route extraction markers"),
            "{error}"
        );
    }

    // --- bind_catchers matching and ambiguity edge cases ---
}
