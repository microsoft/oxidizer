// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Neutral async service-trait and dispatcher code generation.
//!
//! Where [`Router`](crate::Router) only resolves a request to an RPC name, a
//! [`Service`] also generates:
//!
//! - a framework-neutral async service trait (one `impl Future`-returning
//!   method per RPC, returning `Result<Response, rest_over_grpc::Status>`), and
//! - an async `dispatch` function that resolves a request, transcodes the path
//!   variables + query + body into the request message, invokes the trait, and
//!   transcodes the response message back into an `rest_over_grpc::HttpResponse`.
//!
//! The trait is what a `tonic` (or any) server implementation adapts to; the
//! core never names a concrete web stack.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use crate::body::Body;
use crate::codegen::Router;
use crate::response_body::ResponseBody;
use crate::service_method::ServiceMethod;

/// Generates a neutral async service trait + dispatcher for a gRPC service.
///
/// # Examples
///
/// ```
/// use rest_over_grpc_build::{HttpMethod, HttpRule, Service, ServiceMethod};
///
/// let routes = HttpRule::new("CreateShelf", HttpMethod::Post, "/v1/shelves")
///     .lower()
///     .expect("valid path template");
/// let method = ServiceMethod::new(
///     "CreateShelf",
///     ("crate::pb::CreateShelfRequest", "crate::pb::Shelf"),
///     routes,
/// );
/// let service = Service::new("LibraryService", vec![method]);
/// let code = service.generate().to_string();
///
/// assert!(code.contains("pub trait LibraryService"));
/// assert!(code.contains("create_shelf"));
/// ```
#[derive(Debug)]
pub struct Service {
    trait_name: String,
    methods: Vec<ServiceMethod>,
}

impl Service {
    /// Creates a service named `trait_name` (the generated trait's identifier)
    /// from its `methods`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc_build::{HttpMethod, HttpRule, Service, ServiceMethod};
    ///
    /// let routes = HttpRule::new("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")
    ///     .lower()
    ///     .expect("valid path template");
    /// let method = ServiceMethod::new(
    ///     "GetShelf",
    ///     ("crate::pb::GetShelfRequest", "crate::pb::Shelf"),
    ///     routes,
    /// );
    /// let service = Service::new("LibraryService", vec![method]);
    ///
    /// assert!(service.generate().to_string().contains("LibraryService"));
    /// ```
    #[must_use]
    pub fn new(trait_name: impl Into<String>, methods: Vec<ServiceMethod>) -> Self {
        Self {
            trait_name: trait_name.into(),
            methods,
        }
    }

    /// Generates the `resolve` router, the service trait, and the async
    /// `dispatch` function as a single [`TokenStream`].
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc_build::{HttpMethod, HttpRule, Service, ServiceMethod};
    ///
    /// let routes = HttpRule::new("DeleteShelf", HttpMethod::Delete, "/v1/shelves/{shelf}")
    ///     .lower()
    ///     .expect("valid path template");
    /// let method = ServiceMethod::new(
    ///     "DeleteShelf",
    ///     ("crate::pb::DeleteShelfRequest", "crate::pb::Shelf"),
    ///     routes,
    /// );
    /// let tokens = Service::new("LibraryService", vec![method]).generate();
    /// let code = tokens.to_string();
    ///
    /// assert!(code.contains("pub trait LibraryService"));
    /// assert!(code.contains("pub async fn dispatch"));
    /// ```
    #[must_use]
    pub fn generate(&self) -> TokenStream {
        let resolve = self.router().generate();
        let service_trait = self.trait_tokens();
        let dispatch = self.dispatch_tokens();

        quote! {
            #resolve
            #service_trait
            #dispatch
        }
    }

    fn router(&self) -> Router {
        let routes = self.methods.iter().flat_map(|m| m.routes().iter().cloned()).collect();
        Router::new(routes)
    }

    fn trait_tokens(&self) -> TokenStream {
        let trait_ident = ident(&self.trait_name);
        let methods = self.methods.iter().map(|m| {
            let fn_ident = ident(&to_snake_case(m.rpc()));
            let req_ty = type_path(m.types().request());
            let resp_ty = type_path(m.types().response());
            let doc = format!(" Handles the `{}` RPC.", m.rpc());
            quote! {
                #[doc = #doc]
                fn #fn_ident(
                    &self,
                    request: #req_ty,
                ) -> impl ::core::future::Future<
                    Output = ::core::result::Result<#resp_ty, ::rest_over_grpc::Status>,
                >;
            }
        });

        let doc = format!(" Neutral async service trait for the `{}` service.", self.trait_name);
        quote! {
            #[doc = #doc]
            #[allow(
                clippy::all,
                clippy::pedantic,
                clippy::nursery,
                clippy::restriction,
                reason = "code generated by rest_over_grpc_build"
            )]
            pub trait #trait_ident {
                #(#methods)*
            }
        }
    }

    fn dispatch_tokens(&self) -> TokenStream {
        let trait_ident = ident(&self.trait_name);

        let arms = self.methods.iter().map(|m| {
            let rpc = m.rpc();
            let fn_ident = ident(&to_snake_case(m.rpc()));
            let req_ty = type_path(m.types().request());
            // Use the primary binding's body / response-body configuration.
            let primary = m.routes().first();
            let body_kind = primary.map_or_else(
                || quote! { ::rest_over_grpc::transcode::BodyKind::None },
                |r| body_kind_tokens(r.body()),
            );
            let resp_kind = primary.map_or_else(
                || quote! { ::rest_over_grpc::transcode::ResponseBodyKind::Whole },
                |r| response_kind_tokens(r.response_body()),
            );

            quote! {
                #rpc => {
                    async {
                        let request = ::rest_over_grpc::transcode::decode_request::<#req_ty>(
                            matched.bindings(),
                            &query_pairs,
                            body,
                            #body_kind,
                        )
                        .map_err(::rest_over_grpc::transcode::TranscodeError::into_status)?;

                        let response = service.#fn_ident(request).await?;

                        let bytes = ::rest_over_grpc::transcode::encode_response(
                            &response,
                            #resp_kind,
                        )
                        .map_err(::rest_over_grpc::transcode::TranscodeError::into_status)?;

                        ::core::result::Result::<
                            ::std::vec::Vec<u8>,
                            ::rest_over_grpc::Status,
                        >::Ok(bytes)
                    }
                    .await
                }
            }
        });

        quote! {
            /// Dispatches an HTTP request to the `service`, transcoding the
            /// request and response to and from JSON.
            ///
            /// `target` is the request path with an optional `?query`. Returns a
            /// fully-formed `rest_over_grpc::HttpResponse`; unmatched routes yield a
            /// `404`, and transcoding or handler failures yield the appropriate
            /// status. Generated by `rest_over_grpc_build`.
            #[allow(
                clippy::all,
                clippy::pedantic,
                clippy::nursery,
                clippy::restriction,
                dead_code,
                unused,
                reason = "code generated by rest_over_grpc_build"
            )]
            pub async fn dispatch<S: #trait_ident>(
                service: &S,
                method: &str,
                target: &str,
                body: &[u8],
            ) -> ::rest_over_grpc::HttpResponse {
                let (path, query) = ::rest_over_grpc::split_query(target);
                let query_pairs = query
                    .map(::rest_over_grpc::parse_query)
                    .unwrap_or_default();

                let matched = match resolve(method, path) {
                    ::core::option::Option::Some(matched) => matched,
                    ::core::option::Option::None => {
                        return ::rest_over_grpc::transcode::not_found_response();
                    }
                };

                let result: ::core::result::Result<::std::vec::Vec<u8>, ::rest_over_grpc::Status> =
                    match matched.rpc() {
                        #(#arms)*
                        _ => return ::rest_over_grpc::transcode::not_found_response(),
                    };

                match result {
                    ::core::result::Result::Ok(bytes) => {
                        ::rest_over_grpc::HttpResponse::ok_json(bytes)
                    }
                    ::core::result::Result::Err(status) => {
                        ::rest_over_grpc::transcode::status_response(&status)
                    }
                }
            }
        }
    }
}

fn body_kind_tokens(body: &Body) -> TokenStream {
    match body {
        Body::None => quote! { ::rest_over_grpc::transcode::BodyKind::None },
        Body::Whole => quote! { ::rest_over_grpc::transcode::BodyKind::Whole },
        Body::Field(field) => quote! { ::rest_over_grpc::transcode::BodyKind::Field(#field) },
    }
}

fn response_kind_tokens(response_body: &ResponseBody) -> TokenStream {
    match response_body {
        ResponseBody::Whole => quote! { ::rest_over_grpc::transcode::ResponseBodyKind::Whole },
        ResponseBody::Field(field) => {
            quote! { ::rest_over_grpc::transcode::ResponseBodyKind::Field(#field) }
        }
    }
}

fn ident(name: &str) -> Ident {
    Ident::new(name, Span::call_site())
}

/// Parses a fully-qualified Rust type path string into tokens, falling back to a
/// `compile_error!` invocation if the string is not a valid token sequence.
fn type_path(path: &str) -> TokenStream {
    if let Ok(tokens) = path.parse::<TokenStream>() {
        tokens
    } else {
        let message = format!("invalid type path generated for a service method: `{path}`");
        quote! { ::core::compile_error!(#message) }
    }
}

/// Converts a `PascalCase`/`camelCase` RPC name into a `snake_case` method identifier.
fn to_snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (idx, ch) in name.char_indices() {
        if ch.is_ascii_uppercase() {
            if idx != 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_method::HttpMethod;
    use crate::http_rule::HttpRule;

    fn method(rpc: &str, http: HttpMethod, pattern: &str, req: &str, resp: &str) -> ServiceMethod {
        let routes = HttpRule::new(rpc, http, pattern).lower().expect("valid");
        ServiceMethod::new(rpc, (req, resp), routes)
    }

    fn method_from_rule(rule: &HttpRule, req: &str, resp: &str) -> ServiceMethod {
        let rpc = rule.rpc().to_owned();
        ServiceMethod::new(rpc, (req, resp), rule.lower().expect("valid"))
    }

    #[test]
    fn snake_case_conversion() {
        assert_eq!(to_snake_case("GetShelf"), "get_shelf");
        assert_eq!(to_snake_case("ListBooksByAuthor"), "list_books_by_author");
        assert_eq!(to_snake_case("already_snake"), "already_snake");
    }

    #[test]
    fn generates_valid_service_code() {
        let service = Service::new(
            "Library",
            vec![
                method(
                    "GetShelf",
                    HttpMethod::Get,
                    "/v1/shelves/{shelf}",
                    "crate::pb::GetShelfRequest",
                    "crate::pb::Shelf",
                ),
                method(
                    "CreateShelf",
                    HttpMethod::Post,
                    "/v1/shelves",
                    "crate::pb::CreateShelfRequest",
                    "crate::pb::Shelf",
                ),
            ],
        );

        let file: syn::File = syn::parse2(service.generate()).expect("generated service must be valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("pub trait Library"));
        assert!(pretty.contains("fn get_shelf"));
        assert!(pretty.contains("pub async fn dispatch"));
        assert!(pretty.contains("GetShelfRequest"));
    }

    #[test]
    fn invalid_type_path_yields_compile_error() {
        let service = Service::new("S", vec![method("Run", HttpMethod::Get, "/v1/x", "crate::Resp (", "crate::Resp")]);
        let pretty = prettyplease::unparse(&syn::parse2(service.generate()).expect("still parses with compile_error!"));
        assert!(pretty.contains("compile_error!"));
    }

    #[test]
    fn response_body_and_body_field_kinds_are_generated() {
        let service = Service::new(
            "Library",
            vec![
                method_from_rule(
                    &HttpRule::new("CreateBook", HttpMethod::Post, "/v1/books")
                        .with_body(Body::Whole)
                        .with_response_body(ResponseBody::Field("book".into())),
                    "crate::pb::CreateBookRequest",
                    "crate::pb::Book",
                ),
                method_from_rule(
                    &HttpRule::new("UpdateBook", HttpMethod::Patch, "/v1/books/{book}").with_body(Body::Field("book".into())),
                    "crate::pb::UpdateBookRequest",
                    "crate::pb::Book",
                ),
            ],
        );

        let file: syn::File = syn::parse2(service.generate()).expect("generated service must be valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("BodyKind::Whole"));
        assert!(pretty.contains("BodyKind::Field(\"book\")"));
        assert!(pretty.contains("ResponseBodyKind::Field(\"book\")"));
    }

    #[test]
    fn invalid_response_type_path_yields_compile_error() {
        let service = Service::new("S", vec![method("Run", HttpMethod::Get, "/v1/x", "crate::Req", "crate::Resp (")]);
        let pretty = prettyplease::unparse(&syn::parse2(service.generate()).expect("still parses with compile_error!"));
        assert!(pretty.contains("invalid type path generated for a service method"));
    }

    #[test]
    fn empty_route_methods_use_default_transcoding_kinds() {
        let service = Service::new("S", vec![ServiceMethod::new("Run", ("crate::Req", "crate::Resp"), Vec::new())]);

        let pretty = prettyplease::unparse(&syn::parse2(service.generate()).expect("generated service must be valid Rust"));
        assert!(pretty.contains("BodyKind::None"));
        assert!(pretty.contains("ResponseBodyKind::Whole"));
    }
}
