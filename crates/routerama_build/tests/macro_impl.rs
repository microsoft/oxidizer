// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Procedural macro expansion public API tests.

#![cfg(feature = "codegen")]

use quote::quote;
#[cfg(feature = "route")]
use routerama_build::macro_impl::router;
use routerama_build::macro_impl::{derive_from_query, derive_to_query, resolver};

#[test]
fn from_query_expands_a_decoder() {
    let expanded = derive_from_query(quote! {
        struct Request {
            value: String,
        }
    })
    .to_string();

    assert!(expanded.contains("DecodeFields"), "{expanded}");
}

#[test]
fn to_query_expands_an_encoder() {
    let expanded = derive_to_query(quote! {
        struct Request {
            value: String,
        }
    })
    .to_string();

    assert!(expanded.contains("EncodeFields"), "{expanded}");
}

#[test]
fn query_derives_preserve_generics_and_where_clauses() {
    let input = quote! {
        struct Request<'q, 'marker, T, const N: usize>
        where
            T: Clone,
        {
            value: &'q str,
            generic: T,
            #[query(skip)]
            marker: core::marker::PhantomData<(&'marker (), [T; N])>,
        }
    };
    let decoded = derive_from_query(input.clone()).to_string();
    let encoded = derive_to_query(input).to_string();

    for expanded in [&decoded, &encoded] {
        assert!(expanded.contains("const N : usize"), "{expanded}");
        assert!(expanded.contains("T : Clone"), "{expanded}");
        assert!(expanded.contains("'marker"), "{expanded}");
    }
    assert!(decoded.contains("T : :: core :: str :: FromStr"), "{decoded}");
    assert!(encoded.contains("T : :: core :: fmt :: Display"), "{encoded}");
}

#[test]
fn static_resolver_expands_an_infallible_constructor_without_a_builder() {
    let expanded = resolver(
        quote! {},
        quote! {
            enum Route {
                #[route(GET, "/")]
                Home,
            }
        },
    )
    .to_string();

    assert!(expanded.contains("fn resolver"), "{expanded}");
    assert!(expanded.contains("RouteResolver"), "{expanded}");
    assert!(!expanded.contains("RouteResolverBuilder"), "{expanded}");
}

#[test]
fn dynamic_resolver_expands_a_builder() {
    let expanded = resolver(
        quote! {},
        quote! {
            enum Route {
                #[route(dynamic)]
                Home,
            }
        },
    )
    .to_string();

    assert!(expanded.contains("RouteResolver"), "{expanded}");
    assert!(expanded.contains("RouteResolverBuilder"), "{expanded}");
    assert!(expanded.contains("fn builder"), "{expanded}");
}

#[test]
fn resolver_accepts_an_explicit_type_name() {
    let expanded = resolver(
        quote! { name = ApiResolver },
        quote! {
            enum Route {
                #[route(dynamic)]
                Home,
            }
        },
    )
    .to_string();

    assert!(expanded.contains("struct ApiResolver"), "{expanded}");
    assert!(expanded.contains("struct ApiResolverBuilder"), "{expanded}");
}

#[test]
fn resolver_rejects_unknown_naming_arguments() {
    let expanded = resolver(
        quote! { type_name = ApiResolver },
        quote! {
            enum Route {
                #[route(GET, "/")]
                Home,
            }
        },
    )
    .to_string();

    assert!(expanded.contains("expected `name = ResolverType`"), "{expanded}");
}

#[test]
#[cfg(feature = "route")]
fn router_reports_duplicate_body_markers_as_a_build_error() {
    let expanded = router(
        quote! {},
        quote! {
            impl Api {
                #[route(POST, "/")]
                async fn create(
                    &self,
                    #[body] first: Vec<u8>,
                    #[body] second: Vec<u8>,
                ) -> Response {
                    response(first, second)
                }
            }
        },
    )
    .to_string();

    assert!(expanded.contains("compile_error"), "{expanded}");
    assert!(expanded.contains("at most one `#[body]`"), "{expanded}");
}

#[test]
#[cfg(feature = "route")]
fn router_reports_invalid_handler_signatures_as_build_errors() {
    let expanded = router(
        quote! {},
        quote! {
            impl Api {
                #[route(GET, "/")]
                fn home(&self) -> Response {
                    response()
                }
            }
        },
    )
    .to_string();

    assert!(expanded.contains("compile_error"), "{expanded}");
    assert!(expanded.contains("must be async"), "{expanded}");
}

#[test]
#[cfg(feature = "route")]
fn router_fixed_state_specializes_the_generated_entry() {
    let expanded = router(
        quote! { state = self::AppState },
        quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self, state: State<Projected>) -> Response {
                    response(state)
                }
            }
        },
    )
    .to_string();

    assert!(expanded.contains("state : & self :: AppState"), "{expanded}");
    assert!(expanded.contains("__RouteramaFixedStateContract"), "{expanded}");
    assert!(!expanded.contains("__RouteramaState : ? :: core :: marker :: Sized"), "{expanded}");
}

#[test]
#[cfg(feature = "route")]
fn router_rejects_unknown_fixed_state_arguments() {
    let expanded = router(
        quote! { context = AppState },
        quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> Response {
                    response()
                }
            }
        },
    )
    .to_string();

    assert!(expanded.contains("compile_error"), "{expanded}");
    assert!(
        expanded.contains("expected `state = StateType`, `erased_mounts`, `tower`, or `heterogeneous_data`"),
        "{expanded}"
    );
}

#[test]
#[cfg(feature = "route")]
fn generated_relaxed_bounds_qualify_sized() {
    let router_expansion = router(
        quote! {},
        quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(&self) -> Response {
                    response()
                }
            }
        },
    )
    .to_string();
    let resolver_expansion = resolver(
        quote! {},
        quote! {
            enum Route {
                #[route(GET, "/")]
                Home,
            }
        },
    )
    .to_string();

    for expanded in [&router_expansion, &resolver_expansion] {
        assert!(expanded.contains("? :: core :: marker :: Sized"), "{expanded}");
        assert!(!expanded.contains("? Sized"), "{expanded}");
    }
}

#[test]
#[cfg(feature = "route")]
fn router_accepts_higher_ranked_extractor_lifetimes() {
    let expanded = router(
        quote! {},
        quote! {
            impl Api {
                #[route(GET, "/")]
                async fn home(
                    &self,
                    tagged: Box<dyn for<'a> Tagged<'a>>,
                    ranked: Wrapper<for<'a> fn(&'a str)>,
                ) -> Response {
                    response(tagged, ranked)
                }
            }
        },
    )
    .to_string();

    assert!(!expanded.contains("compile_error"), "{expanded}");
    assert!(expanded.contains("for < 'a > Tagged < 'a >"), "{expanded}");
}

#[test]
#[cfg(feature = "route")]
fn router_rejects_free_named_extractor_lifetimes() {
    for extractor in [quote! { Wrapper<'a> }, quote! { Box<dyn for<'b> Tagged<'b, 'a>> }] {
        let expanded = router(
            quote! {},
            quote! {
                impl Api {
                    #[route(GET, "/")]
                    async fn home(&self, borrowed: #extractor) -> Response {
                        response(borrowed)
                    }
                }
            },
        )
        .to_string();

        assert!(expanded.contains("compile_error"), "{expanded}");
        assert!(expanded.contains("request-parts extractor lifetimes must be elided"), "{expanded}");
    }
}

#[test]
#[cfg(feature = "route")]
fn router_rejects_a_template_deeper_than_the_segment_limit() {
    let (accepted, rejected) = depth_limit_templates();

    let expanded = router(quote! {}, router_item(&rejected)).to_string();
    assert!(expanded.contains("compile_error"), "{expanded}");
    assert!(
        expanded.contains("path template declares 257 segments, but a route may declare at most 256"),
        "{expanded}"
    );

    let expanded = router(quote! {}, router_item(&accepted)).to_string();
    assert!(!expanded.contains("compile_error"), "{expanded}");
}

#[test]
fn resolver_rejects_a_template_deeper_than_the_segment_limit() {
    let (accepted, rejected) = depth_limit_templates();

    let expanded = resolver(quote! {}, resolver_item(&rejected)).to_string();
    assert!(expanded.contains("compile_error"), "{expanded}");
    assert!(
        expanded.contains("path template declares 257 segments, but a route may declare at most 256"),
        "{expanded}"
    );

    let expanded = resolver(quote! {}, resolver_item(&accepted)).to_string();
    assert!(!expanded.contains("compile_error"), "{expanded}");
}

/// A template at the segment limit and the first one past it.
fn depth_limit_templates() -> (String, String) {
    ("/a".repeat(256), "/a".repeat(257))
}

fn router_item(path: &str) -> proc_macro2::TokenStream {
    let path = syn::LitStr::new(path, proc_macro2::Span::call_site());
    quote! {
        impl Api {
            #[route(GET, #path)]
            async fn home(&self) -> Response {
                response()
            }
        }
    }
}

fn resolver_item(path: &str) -> proc_macro2::TokenStream {
    let path = syn::LitStr::new(path, proc_macro2::Span::call_site());
    quote! {
        enum Route {
            #[route(GET, #path)]
            Home,
        }
    }
}
