// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http_path_template::{PathTemplate, Segment};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use crate::build::emit::generate_router_with_visibility;
use crate::build::http_rule::HttpRule;
use crate::build::request_body::RequestBody;
use crate::build::response_body::ResponseBody;
use crate::build::route::Route;
use crate::build::service_method::ServiceMethod;

/// Defines an individual REST service and its mapping to an existing gRPC service.
///
/// Build one by hand with [`new`](Self::new) + [`add_method`](Self::add_method),
/// or decode a batch from a `.proto` file descriptor set with
/// [`from_fds`](Self::from_fds).
///
/// # Examples
///
/// ```
/// use rest_over_grpc::build::{Generator, HttpMethod, HttpRule, ServiceDefinition};
///
/// let rule = HttpRule::new(
///     "CreateShelf",
///     HttpMethod::Post,
///     "/v1/shelves".parse().expect("valid path template"),
/// );
///
/// let mut library = ServiceDefinition::new("LibraryService", None);
/// library.add_method(
///     rule,
///     "crate::pb::CreateShelfRequest",
///     "crate::pb::Shelf",
///     None,
/// );
///
/// let (_transcoder, generated) = Generator::new().add(library).generate();
/// assert!(
///     generated[0]
///         .r#trait()
///         .to_string()
///         .contains("pub trait LibraryService")
/// );
/// ```
#[derive(Debug, Clone)]
pub struct ServiceDefinition {
    trait_name: String,
    module: String,
    doc: Option<String>,
    methods: Vec<ServiceMethod>,
    // OpenAPI schema state, populated only by `from_fds` (from a
    // `FileDescriptorSet`); there is no manual API to set it.
    #[cfg(feature = "build-openapi")]
    openapi: Option<crate::build::openapi::Builder>,
}

impl ServiceDefinition {
    /// Creates an empty definition for the service whose generated trait is named
    /// `trait_name`.
    ///
    /// `doc` documents the generated trait, applied verbatim (one `#[doc]` per
    /// line); `None` emits no doc comment.
    ///
    /// The output module defaults to the snake-cased trait name; override it with
    /// [`module`](Self::module).
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::build::ServiceDefinition;
    ///
    /// let library = ServiceDefinition::new("LibraryService", None);
    /// assert_eq!(library.trait_name(), "LibraryService");
    /// ```
    #[must_use]
    pub fn new(trait_name: impl Into<String>, doc: Option<String>) -> Self {
        let trait_name = trait_name.into();
        let module = to_snake_case(&trait_name);
        Self {
            trait_name,
            module,
            doc,
            methods: Vec::new(),
            #[cfg(feature = "build-openapi")]
            openapi: None,
        }
    }

    /// Overrides the module name used to group this service's generated code into
    /// an output file (`{module}.rest.rs`).
    ///
    /// Defaults to the snake-cased trait
    /// name.
    ///
    /// Returns `&mut Self` so calls can be chained.
    pub fn module(&mut self, module: impl Into<String>) -> &mut Self {
        self.module = module.into();
        self
    }

    /// The name of the generated service trait.
    #[must_use]
    pub fn trait_name(&self) -> &str {
        &self.trait_name
    }

    /// The module name used to group this service's output file.
    #[must_use]
    pub fn module_name(&self) -> &str {
        &self.module
    }

    /// Stores this service's OpenAPI schema state, built from a
    /// `FileDescriptorSet` by [`from_fds`](Self::from_fds). There is no public
    /// API to set this; it is available only for descriptor-decoded services.
    #[cfg(feature = "build-openapi")]
    pub(crate) fn set_openapi(&mut self, openapi: crate::build::openapi::Builder) {
        self.openapi = Some(openapi);
    }

    /// Renders this service's OpenAPI 3.1 document (as pretty-printed JSON) using
    /// `info` for the title, version, and servers, or `None` if the service
    /// carries no OpenAPI state (i.e. it was not decoded from a descriptor).
    #[cfg(feature = "build-openapi")]
    pub(crate) fn openapi_spec(&self, info: &crate::build::OpenApiInfo) -> Option<String> {
        self.openapi.as_ref().map(|builder| builder.render(info))
    }

    /// Registers one REST RPC on the service.
    ///
    /// You specify the specific REST API via the supplied rule, indicate the
    /// names of the types for the gRPC request and responses, and an optional
    /// documentation comment to apply to the generated method in the service trait.
    ///
    /// Returns `&mut Self` so calls can be chained or issued in a loop.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::build::{Generator, HttpMethod, HttpRule, ServiceDefinition};
    ///
    /// let rule = HttpRule::new(
    ///     "GetShelf",
    ///     HttpMethod::Get,
    ///     "/v1/shelves/{shelf}".parse().expect("valid path template"),
    /// );
    /// let mut generator = ServiceDefinition::new("LibraryService", None);
    /// generator.add_method(rule, "crate::pb::GetShelfRequest", "crate::pb::Shelf", None);
    ///
    /// let (_transcoder, generated) = Generator::new().add(generator).generate();
    /// assert!(generated[0].r#trait().to_string().contains("get_shelf"));
    /// ```
    pub fn add_method(
        &mut self,
        rule: HttpRule,
        request_type: impl Into<String>,
        response_type: impl Into<String>,
        doc: Option<String>,
    ) -> &mut Self {
        let name = rule.name().to_owned();
        let enum_fields = rule.enum_path_fields().to_vec();
        self.methods.push(ServiceMethod::new(
            name,
            request_type,
            response_type,
            rule.lower(),
            false,
            doc,
            enum_fields,
        ));
        self
    }

    /// Registers one server-streaming REST RPC on the service.
    ///
    /// Registers one server-streaming RPC: like [`add_method`](Self::add_method),
    /// but its generated trait method is two-phase — it awaits initiation and
    /// yields a `ResponseStream`, which the transcoder renders as a
    /// `StreamingResponse` whose frames are encoded per the negotiated
    /// `StreamEncoding`. `doc` documents the generated trait method as in
    /// [`add_method`](Self::add_method).
    ///
    /// Returns `&mut Self` so calls can be chained or issued in a loop.
    pub fn add_server_streaming_method(
        &mut self,
        rule: HttpRule,
        request_type: impl Into<String>,
        response_type: impl Into<String>,
        doc: Option<String>,
    ) -> &mut Self {
        let name = rule.name().to_owned();
        let enum_fields = rule.enum_path_fields().to_vec();
        self.methods.push(ServiceMethod::new(
            name,
            request_type,
            response_type,
            rule.lower(),
            true,
            doc,
            enum_fields,
        ));
        self
    }

    /// Decodes every annotated service in a `.proto` file descriptor set into a
    /// [`ServiceDefinition`].
    ///
    /// # Errors
    ///
    /// Returns a [`DescriptorError`](crate::build::DescriptorError) if there was an issue
    /// processing the descriptor set.
    #[cfg(feature = "build")]
    pub fn from_fds(
        descriptor_set: impl AsRef<[u8]>,
        options: &crate::build::DescriptorOptions,
    ) -> Result<Vec<Self>, crate::build::DescriptorError> {
        crate::build::descriptor::definitions_from_descriptor(descriptor_set.as_ref(), options)
    }

    /// Renders the combined trait + transcoder and (when
    /// `options.emit_tonic`) the tonic bridge, as one token stream. Test-only
    /// helper; production code uses [`trait_code`](Self::trait_code) and
    /// [`tonic_bridge`](Self::tonic_bridge) separately via the [`Generator`](crate::build::Generator).
    #[cfg(test)]
    pub(crate) fn generate(&self, options: crate::build::generator::CodegenOptions) -> TokenStream {
        let trait_code = self.trait_code();
        let bridge = if options.emit_tonic {
            self.tonic_bridge()
        } else {
            quote! {}
        };
        // Append the single-service transcoder so tests can inspect the routing
        // and transcoding that live on the `Transcoder`.
        let transcoder = generate_transcoder(std::slice::from_ref(self));
        quote! {
            #trait_code
            #bridge
            #transcoder
        }
    }

    /// Emits a blanket `impl <trait> for T where T: <tonic server trait>`, so a
    /// service implemented once against `tonic`'s generated server trait also
    /// satisfies this service's REST handler trait (and so can be routed by the
    /// generated `Transcoder`).
    ///
    /// The generated code is `include!`d into the same module as the messages
    /// (and `tonic`'s output), so `tonic`'s server module is referenced by its
    /// package-relative path `{service_snake}_server::{Service}` — the layout
    /// `tonic-build` produces. The emitted code references `::tonic` and
    /// `::rest_over_grpc::handling::{Code, Status}`; those must resolve in the consumer.
    ///
    /// Unary methods forward directly, converting `tonic::Status` to
    /// [`rest_over_grpc::handling::Status`](rest_over_grpc). Server-streaming methods are
    /// bridged too: both the `tonic` method and the generated trait method are
    /// two-phase (`async fn -> Result<Stream, Status>`), so the bridge awaits
    /// initiation, maps the initiation `tonic::Status`, and boxes the response
    /// stream (`tonic`'s stream is `Send + 'static`) with per-item errors mapped
    /// via `rest_over_grpc::codegen_helpers::map_stream_status`.
    pub(crate) fn tonic_bridge(&self) -> TokenStream {
        let our_trait = ident(&self.trait_name);
        let snake = to_snake_case(&self.trait_name);
        let tonic_trait = type_path(&format!("{snake}_server::{}", self.trait_name));

        let arms = self.methods.iter().map(|m| {
            let fn_ident = ident(&to_snake_case(m.rpc()));
            let req_ty = type_path(m.request_type());
            let resp_ty = type_path(m.response_type());

            if m.server_streaming() {
                // Both the `tonic` method and the generated trait method are
                // two-phase (`async fn -> Result<Stream, Status>`): await
                // initiation, map the `tonic::Status` (initiation and per-item) to
                // `rest_over_grpc::handling::Status`, box the `Send + 'static` response
                // stream, and seed the request metadata from `cx`'s headers.
                return quote! {
                    async fn #fn_ident(
                        &self,
                        request: #req_ty,
                        cx: &mut ::rest_over_grpc::handling::Context,
                    ) -> ::core::result::Result<
                        ::rest_over_grpc::handling::ResponseStream<#resp_ty>,
                        ::rest_over_grpc::handling::Status,
                    > {
                        fn __convert_status(status: ::tonic::Status) -> ::rest_over_grpc::handling::Status {
                            ::rest_over_grpc::handling::Status::new(
                                ::rest_over_grpc::handling::Code::from_i32(status.code() as i32)
                                    .unwrap_or(::rest_over_grpc::handling::Code::Unknown),
                                status.message(),
                            )
                        }
                        let mut __request = ::tonic::Request::new(request);
                        *__request.metadata_mut() =
                            ::tonic::metadata::MetadataMap::from_headers(cx.take_request_headers());
                        let __stream = <Self as #tonic_trait>::#fn_ident(self, __request)
                            .await
                            .map_err(__convert_status)?
                            .into_inner();
                        ::core::result::Result::Ok(::std::boxed::Box::pin(
                            ::rest_over_grpc::codegen_helpers::map_stream_status(__stream, __convert_status),
                        ))
                    }
                };
            }

            quote! {
                async fn #fn_ident(
                    &self,
                    request: #req_ty,
                    cx: &mut ::rest_over_grpc::handling::Context,
                ) -> ::core::result::Result<#resp_ty, ::rest_over_grpc::handling::Status> {
                    let mut __request = ::tonic::Request::new(request);
                    *__request.metadata_mut() =
                        ::tonic::metadata::MetadataMap::from_headers(cx.take_request_headers());
                    match <Self as #tonic_trait>::#fn_ident(self, __request).await {
                        ::core::result::Result::Ok(response) => {
                            let (__metadata, __message, _) = response.into_parts();
                            cx.merge_response_headers(__metadata.into_headers());
                            ::core::result::Result::Ok(__message)
                        }
                        ::core::result::Result::Err(status) => {
                            ::core::result::Result::Err(::rest_over_grpc::handling::Status::new(
                                ::rest_over_grpc::handling::Code::from_i32(status.code() as i32)
                                    .unwrap_or(::rest_over_grpc::handling::Code::Unknown),
                                status.message(),
                            ))
                        }
                    }
                }
            }
        });

        let doc = format!(
            " Bridges `tonic`'s `{0}` server trait to [`{0}`], so a single `tonic`\n implementation also serves REST via `transcode`.",
            self.trait_name
        );
        quote! {
            #[doc = #doc]
            #[allow(
                clippy::all,
                clippy::pedantic,
                clippy::nursery,
                clippy::restriction,
                dead_code,
                unused,
                reason = "code generated by rest_over_grpc::build"
            )]
            impl<T> #our_trait for T
            where
                T: #tonic_trait,
            {
                #(#arms)*
            }
        }
    }

    /// Renders this definition's service trait (its RPC handler methods only;
    /// routing/transcoding live on the `Transcoder`). The handler futures are
    /// `Send`-bounded and the trait gains a `Send + Sync` supertrait. Does not
    /// include the tonic bridge (see [`tonic_bridge`](Self::tonic_bridge)).
    pub(crate) fn trait_code(&self) -> TokenStream {
        let trait_ident = ident(&self.trait_name);
        let send_bound = Self::send_bound();
        let supertrait = quote! { : ::core::marker::Send + ::core::marker::Sync };

        let methods = self.methods.iter().map(|m| {
            let fn_ident = ident(&to_snake_case(m.rpc()));
            let req_ty = type_path(m.request_type());
            let resp_ty = type_path(m.response_type());
            // Emit the RPC's proto documentation when present (one `#[doc]` per
            // line, preserving its structure); otherwise emit no doc comment.
            let doc_attrs = m.doc().map(|doc| {
                let lines = doc.split('\n').map(|line| quote! { #[doc = #line] });
                quote! { #(#lines)* }
            });
            let ret = if m.server_streaming() {
                quote! {
                    impl ::core::future::Future<
                        Output = ::core::result::Result<
                            ::rest_over_grpc::handling::ResponseStream<#resp_ty>,
                            ::rest_over_grpc::handling::Status,
                        >,
                    > #send_bound
                }
            } else {
                quote! {
                    impl ::core::future::Future<
                        Output = ::core::result::Result<#resp_ty, ::rest_over_grpc::handling::Status>,
                    > #send_bound
                }
            };
            quote! {
                #doc_attrs
                fn #fn_ident(&self, request: #req_ty, cx: &mut ::rest_over_grpc::handling::Context) -> #ret;
            }
        });

        // The service's `doc` documents the trait verbatim (one `#[doc]` per
        // line); when absent, no doc comment is emitted.
        let doc_attrs = self.doc.as_ref().map(|doc| {
            let lines = doc.split('\n').map(|line| quote! { #[doc = #line] });
            quote! { #(#lines)* }
        });
        quote! {
            #doc_attrs
            #[allow(
                clippy::all,
                clippy::pedantic,
                clippy::nursery,
                clippy::restriction,
                reason = "code generated by rest_over_grpc::build"
            )]
            pub trait #trait_ident #supertrait {
                #(#methods)*
            }
        }
    }

    /// The ` + Send` bound appended to generated handler return types.
    fn send_bound() -> TokenStream {
        quote! { + ::core::marker::Send }
    }
}

/// Renders a single top-level `Transcoder` that routes an incoming request to
/// the right handler across all `services`. A transcoder is always emitted; a
/// service-less `services` yields one that matches nothing and answers every
/// request with a `404`.
///
/// The transcoder is generic over each service's handler type (the traits return
/// `impl Future`, so they are not dyn-compatible; generics keep transcode
/// monomorphized and zero-cost). It owns the handlers, is constructed with
/// `Transcoder::new(...)`, and implements
/// [`Transcode`](::rest_over_grpc::transcoding::Transcode) (`try_transcode` /
/// `transcode`), whose `transcode` returns a
/// [`TranscodeResponse`](::rest_over_grpc::transcoding::TranscodeResponse) — a unary
/// RPC's buffered response or a server-streaming RPC's live frame stream. A
/// single merged router lowers every service's routes into one match, so a
/// conflicting route across services surfaces as a `compile_error!`.
///
/// The transcoder references each service by its module-qualified path
/// (`{module}::Trait` and `{module}::RequestType`), so the generated
/// `transcoder.rest.rs` must be `include!`d at the scope where each service's
/// `{module}.rest.rs` is a sibling module.
pub(crate) fn generate_transcoder(services: &[ServiceDefinition]) -> TokenStream {
    let send_bound = ServiceDefinition::send_bound();

    let generics: Vec<Ident> = (0..services.len()).map(|i| ident(&format!("T{i}"))).collect();
    let fields: Vec<Ident> = (0..services.len()).map(|i| ident(&transcoder_field_name(services, i))).collect();
    let bounds: Vec<TokenStream> = services
        .iter()
        .zip(&generics)
        .map(|(svc, g)| {
            let trait_path = qualified_type_path(&svc.trait_name, &svc.module);
            quote! { #g: #trait_path }
        })
        .collect();

    // Merged router over every service's routes, each tagged uniquely so the
    // transcode arms can key on the resolved `(service, rpc)`. Every method emits
    // one arm producing a `TranscodeResponse` (a unary RPC's buffered response or
    // a server-streaming RPC's live frame stream), so a single `transcode` serves
    // both shapes.
    let mut merged_routes: Vec<Route> = Vec::new();
    let mut arms: Vec<TokenStream> = Vec::new();
    for (i, svc) in services.iter().enumerate() {
        let receiver = {
            let field = &fields[i];
            quote! { self.#field }
        };
        for method in &svc.methods {
            let base = route_name(i, method.rpc());
            let req_ty = qualified_type_path(method.request_type(), &svc.module);
            let (variants, routes) = assign_method_variants(&base, method, &svc.module);
            merged_routes.extend(routes);
            arms.push(transcode_arm(&variants, &receiver, &req_ty, method));
        }
    }

    let resolve = generate_router_with_visibility(&merged_routes, &quote! {});
    let try_transcode_method = transcoder_try_transcode_method(&arms, &send_bound);

    // Omit the `<…>` / `where` entirely for a service-less generator so the
    // emitted struct/impl stay valid Rust.
    let generic_params = if generics.is_empty() {
        quote! {}
    } else {
        quote! { <#(#generics),*> }
    };
    let where_clause = if bounds.is_empty() {
        quote! {}
    } else {
        quote! { where #(#bounds,)* }
    };
    let transcode_impl = transcoder_transcode_impl(&try_transcode_method, &generic_params, &where_clause);

    quote! {
        // The merged router is emitted once at module scope here, rather than
        // inlined into the `try_transcode` method body.
        #resolve

        /// Routes incoming REST requests across all generated services to the
        /// right handler. Generated by `rest_over_grpc::build`.
        #[derive(Clone)]
        #[allow(
            clippy::all,
            clippy::pedantic,
            clippy::nursery,
            clippy::restriction,
            dead_code,
            unused,
            missing_debug_implementations,
            reason = "code generated by rest_over_grpc::build"
        )]
        pub struct Transcoder #generic_params {
            #(#fields: #generics,)*
        }

        #[allow(
            clippy::all,
            clippy::pedantic,
            clippy::nursery,
            clippy::restriction,
            dead_code,
            unused,
            reason = "code generated by rest_over_grpc::build"
        )]
        impl #generic_params Transcoder #generic_params #where_clause {
            /// Creates a transcoder owning the given service handlers.
            pub fn new(#(#fields: #generics,)*) -> Self {
                Self { #(#fields,)* }
            }
        }

        #transcode_impl
    }
}

/// The generated `Transcoder`'s
/// [`Transcode`](::rest_over_grpc::transcoding::Transcode) impl (`try_transcode`,
/// with `transcode` defaulted).
fn transcoder_transcode_impl(try_transcode_method: &TokenStream, generic_params: &TokenStream, where_clause: &TokenStream) -> TokenStream {
    quote! {
        #[allow(
            clippy::all,
            clippy::pedantic,
            clippy::nursery,
            clippy::restriction,
            dead_code,
            unused,
            reason = "code generated by rest_over_grpc::build"
        )]
        impl #generic_params ::rest_over_grpc::transcoding::Transcode for Transcoder #generic_params #where_clause {
            #try_transcode_method
        }
    }
}

/// The `try_transcode` method body for the generated `Transcoder`'s
/// [`Transcode`](::rest_over_grpc::transcoding::Transcode) impl. It calls the
/// module-level `resolve` router emitted alongside it and yields a
/// [`TranscodeResponse`](::rest_over_grpc::transcoding::TranscodeResponse) so a
/// single transcode call serves both unary and server-streaming RPCs.
/// `transcode` is a default trait method.
fn transcoder_try_transcode_method(arms: &[TokenStream], send_bound: &TokenStream) -> TokenStream {
    quote! {
        /// Transcodes an HTTP request to the matching service, returning
        /// [`None`](::core::option::Option::None) if no route matches the method
        /// and path.
        ///
        /// Yields a
        /// [`TranscodeResponse`](::rest_over_grpc::transcoding::TranscodeResponse): a
        /// unary RPC's buffered response, or a server-streaming RPC's live frame
        /// stream. Generated by `rest_over_grpc::build`.
        #[allow(
            clippy::all,
            clippy::pedantic,
            clippy::nursery,
            clippy::restriction,
            dead_code,
            unused,
            reason = "code generated by rest_over_grpc::build"
        )]
        fn try_transcode(
            &self,
            method: &str,
            target: &str,
            headers: ::rest_over_grpc::codegen_helpers::HeaderMap,
            body: &[u8],
        ) -> impl ::core::future::Future<Output = ::core::option::Option<::rest_over_grpc::transcoding::TranscodeResponse>> #send_bound {
            async move {
                let (path, query) = ::rest_over_grpc::codegen_helpers::split_query(target);
                let query_pairs = query
                    .map(::rest_over_grpc::codegen_helpers::parse_query)
                    .unwrap_or_default();

                let matched = match Route::resolve(method, path) {
                    ::core::option::Option::Some(matched) => matched,
                    ::core::option::Option::None => {
                        return ::core::option::Option::None;
                    }
                };

                let mut cx = ::rest_over_grpc::handling::Context::new(headers);

                let mut response: ::rest_over_grpc::transcoding::TranscodeResponse = match matched {
                    #(#arms)*
                    #[allow(unreachable_patterns, reason = "the route enum match is exhaustive; this is a defensive fallback")]
                    _ => return ::core::option::Option::None,
                };

                let __response_headers = cx.into_response_headers();
                match &mut response {
                    ::rest_over_grpc::transcoding::TranscodeResponse::Unary(__unary) => {
                        __unary.merge_headers(__response_headers);
                    }
                    ::rest_over_grpc::transcoding::TranscodeResponse::Streaming(__streaming) => {
                        __streaming.merge_headers(__response_headers);
                    }
                }
                ::core::option::Option::Some(response)
            }
        }
    }
}

/// The transcoder field / constructor-parameter name for the service at `index`
/// — the snake-cased trait name, suffixed with the index only if another service
/// shares that name.
fn transcoder_field_name(services: &[ServiceDefinition], index: usize) -> String {
    let base = to_snake_case(&services[index].trait_name);
    let unique = services
        .iter()
        .enumerate()
        .all(|(i, other)| i == index || to_snake_case(&other.trait_name) != base);
    if unique { base } else { format!("{base}_{index}") }
}

/// A globally-unique transcode name for the `rpc` of the service at `index`, used
/// as the router leaf key and the route enum variant so routing resolves
/// directly to `(service, rpc)`. Suffixing the RPC with the service index keeps
/// it a valid, collision-free Rust identifier even when two services share an
/// RPC name.
fn route_name(index: usize, rpc: &str) -> String {
    format!("{rpc}_{index}")
}

/// One route enum variant a transcode arm keys on: its name (see [`route_name`])
/// and its capture signature — the ordered dotted field paths of the path
/// variables it captures (empty for a unit variant). The signature drives both
/// the destructuring pattern and the field pokes the arm emits. `enum_types` is
/// aligned with `signature`: it holds the (module-qualified) generated Rust enum
/// type for each capture that targets a proto `enum` field, and `None` for every
/// other capture (handled by type inference through `parse_path_field`).
struct RouteVariant {
    name: String,
    signature: Vec<Vec<String>>,
    enum_types: Vec<Option<TokenStream>>,
}

/// The ordered capture signature of a path template: the dotted field path of
/// each `{variable}`/affix segment, in order. Two routes share a route enum
/// variant only if their signatures are identical (the variant carries a fixed
/// set of fields), so this groups a method's bindings into distinct variants.
fn capture_signature(template: &PathTemplate) -> Vec<Vec<String>> {
    template
        .segments()
        .iter()
        .filter_map(|segment| match segment {
            Segment::Variable(variable) => Some(variable.field_path().to_vec()),
            Segment::Affix { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// Groups a method's bindings (primary + `additional_bindings`) by capture
/// signature, assigning each distinct signature its own route enum variant. The
/// first signature keeps `base`; each further one gets a `{base}_bN` name. A
/// variant carries a fixed field set, so bindings that capture different path
/// variables must map to different variants — the transcode arm then matches them
/// all with an or-pattern. Returns the variants (for the arm) and the renamed
/// routes (for the router).
fn assign_method_variants(base: &str, method: &ServiceMethod, module: &str) -> (Vec<RouteVariant>, Vec<Route>) {
    let mut variants: Vec<RouteVariant> = Vec::new();
    let mut signatures: Vec<Vec<Vec<String>>> = Vec::new();
    let mut routes: Vec<Route> = Vec::new();
    for route in method.routes() {
        let signature = capture_signature(route.template());
        let name = if let Some(index) = signatures.iter().position(|existing| *existing == signature) {
            variants[index].name.clone()
        } else {
            let name = if variants.is_empty() {
                base.to_owned()
            } else {
                format!("{base}_b{}", variants.len())
            };
            let enum_types = signature.iter().map(|path| enum_type_for(method, path, module)).collect();
            variants.push(RouteVariant {
                name: name.clone(),
                signature: signature.clone(),
                enum_types,
            });
            signatures.push(signature);
            name
        };
        routes.push(rename_route(route, &name));
    }
    (variants, routes)
}

/// The module-qualified generated Rust enum type for the capture at `path`, or
/// `None` if that capture does not target a proto `enum` field. The raw type
/// comes from the method's [`enum_fields`](ServiceMethod::enum_fields) and is
/// resolved into the transcoder's scope via [`qualified_type_path`] (the same
/// resolution the request type uses).
fn enum_type_for(method: &ServiceMethod, path: &[String], module: &str) -> Option<TokenStream> {
    method
        .enum_fields()
        .iter()
        .find(|(field_path, _)| field_path.as_slice() == path)
        .map(|(_, enum_type)| qualified_type_path(enum_type, module))
}

/// The `Route` enum field identifier for a captured path variable, built from
/// [`routerama_build::route_field_name`] — the single source of truth for the
/// generated field-naming scheme — so patterns bind exactly the names `resolve`
/// fills in. `path` is the capture's (possibly dotted) name segments, rejoined
/// into the dotted name the mapping expects.
fn capture_field_ident(path: &[String]) -> Ident {
    Ident::new(&routerama_build::route_field_name(path.join(".")), Span::call_site())
}

/// The transcoder-local binding a route variant's captured path variable is
/// destructured into, by capture position. A reserved `__cap{n}` name so it can
/// never collide with the transcoder's own locals (`request`, `body`, `cx`,
/// `query_pairs`, …) even when a proto field bound in the path is named after
/// one of them — the captured field name only appears on the left of the pattern
/// (`field: __capN`), never as the binding referenced by the pokes.
fn capture_binding_ident(index: usize) -> Ident {
    Ident::new(&format!("__cap{index}"), Span::call_site())
}

/// The destructuring `match` pattern for one route variant: `Route::Name { .. }`
/// binding each captured path variable to a reserved `__cap{n}` local (keyed by
/// [`capture_binding_ident`]), or a unit `Route::Name` for a capture-less route.
/// `Route` is the enum emitted by the router codegen alongside `resolve`; the
/// field names come from [`capture_field_ident`], so the pattern reads exactly
/// the names `resolve` fills in while the bindings stay collision-free.
fn variant_pattern(variant: &RouteVariant) -> TokenStream {
    let ident = Ident::new(&variant.name, Span::call_site());
    if variant.signature.is_empty() {
        quote! { Route::#ident }
    } else {
        let fields = variant.signature.iter().enumerate().map(|(index, path)| {
            let field = capture_field_ident(path);
            let binding = capture_binding_ident(index);
            quote! { #field: #binding }
        });
        quote! { Route::#ident { #(#fields),* } }
    }
}

/// The message field-access identifier for one dotted-path segment (e.g. `type`
/// → `r#type`), matching how `prost` names fields that collide with a Rust
/// keyword.
fn field_segment_ident(segment: &str) -> Ident {
    if is_raw_keyword(segment) {
        Ident::new_raw(segment, Span::call_site())
    } else {
        Ident::new(segment, Span::call_site())
    }
}

/// Whether `word` is a Rust keyword that `prost` emits as a raw identifier
/// (`r#word`) when it is a proto field name. The handful of keywords that cannot
/// be raw identifiers (`self`, `Self`, `super`, `crate`, `extern`) are excluded;
/// they are not valid field names in generated message structs anyway.
fn is_raw_keyword(word: &str) -> bool {
    matches!(
        word,
        "as" | "break"
            | "const"
            | "continue"
            | "else"
            | "enum"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "static"
            | "struct"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
            | "try"
    )
}

/// The statements that poke a route variant's captured path variables directly
/// into the decoded request message `request`, one per capture. Each reads the
/// destructured `&str` from the matched variant and assigns it into the message
/// field the capture targets — for a dotted path (`shelf.id`) the non-leaf
/// segments are materialized with `get_or_insert_with(Default::default)` (which
/// works whether `prost` wrapped the nested message in `Option<_>` or
/// `Option<Box<_>>`). A scalar/`bytes`/`optional` leaf is parsed via
/// [`parse_path_field`](rest_over_grpc::codegen_helpers::parse_path_field), whose
/// target type is inferred from the field. An `enum` leaf — a bare `i32`, so not
/// distinguishable by type — is parsed via
/// [`parse_path_enum_value`](rest_over_grpc::codegen_helpers::parse_path_enum_value)
/// with the field's concrete enum type (from `variant.enum_types`), so the value
/// can be given by name or number; the resulting `i32` is `.into()`-ed to fit
/// either a plain `i32` or an `optional` enum's `Option<i32>` field. Path
/// variables take highest precedence, so these run after the body/query decode.
/// `set_status` maps a parse failure to a status error via `?`.
fn variant_pokes(variant: &RouteVariant) -> TokenStream {
    let statements = variant
        .signature
        .iter()
        .zip(&variant.enum_types)
        .enumerate()
        .map(|(capture_index, (path, enum_type))| {
            let capture = capture_binding_ident(capture_index);
            // Build the lvalue: `request` then each segment, materializing every
            // non-leaf (message) segment so the leaf assignment has somewhere to go.
            let mut lvalue = quote! { request };
            for (index, segment) in path.iter().enumerate() {
                let seg = field_segment_ident(segment);
                if index + 1 < path.len() {
                    lvalue = quote! { #lvalue.#seg.get_or_insert_with(::core::default::Default::default) };
                } else {
                    lvalue = quote! { #lvalue.#seg };
                }
            }
            if let Some(enum_type) = enum_type {
                quote! {
                    #lvalue = ::rest_over_grpc::codegen_helpers::parse_path_enum_value(#capture, #enum_type::from_str_name)
                        .map_err(::rest_over_grpc::codegen_helpers::TranscodeError::into_status)?
                        .into();
                }
            } else {
                quote! {
                    #lvalue = ::rest_over_grpc::codegen_helpers::parse_path_field(#capture)
                        .map_err(::rest_over_grpc::codegen_helpers::TranscodeError::into_status)?;
                }
            }
        });
    quote! { #(#statements)* }
}

/// Clones `route` with its RPC replaced by the transcode `name`.
fn rename_route(route: &Route, name: &str) -> Route {
    Route::new(
        name.to_owned(),
        route.method().clone(),
        route.template().clone(),
        route.body().clone(),
        route.response_body().clone(),
    )
}

/// Qualifies a generated type/trait path for use from the transcoder's scope
/// (which sits outside the per-package service modules).
///
/// A rooted path (`::`, `crate`, `self`, `$crate`) — an extern override or a
/// manual crate-relative path — is already absolute and used as-is. Otherwise
/// the path is package-relative (as produced by `relative_type_path`): it is
/// resolved into an absolute path by consuming each leading `super::` against a
/// segment of the service's `module` (proto package, dotted → `::`) and
/// prefixing the remaining package segments. This yields the correct path for
/// same-package, nested, sub-package, parent, and sibling-package types alike.
fn qualified_type_path(path: &str, module: &str) -> TokenStream {
    let trimmed = path.trim_start();
    if trimmed.starts_with("::")
        || trimmed == "crate"
        || trimmed.starts_with("crate::")
        || trimmed == "self"
        || trimmed.starts_with("self::")
        || trimmed.starts_with("$crate")
    {
        return type_path(path);
    }

    let mut segments: Vec<&str> = if module.is_empty() {
        Vec::new()
    } else {
        module.split('.').collect()
    };
    let mut remainder = trimmed;
    while let Some(rest) = remainder.strip_prefix("super::") {
        // Each `super::` steps up one package segment toward the common root.
        segments.pop();
        remainder = rest;
    }

    let mut absolute = String::new();
    for segment in segments {
        absolute.push_str(segment);
        absolute.push_str("::");
    }
    absolute.push_str(remainder);
    type_path(&absolute)
}

/// The `let __stream_encoding = …;` binding for a server-streaming arm. It
/// negotiates the encoding from the request `Context`'s `Accept` header, so only
/// a matched streaming route pays the lookup (unary routes skip it entirely).
fn stream_encoding_binding() -> TokenStream {
    quote! {
        let __stream_encoding = ::rest_over_grpc::codegen_helpers::StreamEncoding::from_accept(
            cx.request_headers()
                .get("accept")
                .and_then(|value| value.to_str().ok())
                .unwrap_or(""),
        );
    }
}

/// The expression that decodes the request body + query into `req_ty` and then
/// pokes this `variant`'s captured path variables into it, yielding a
/// `Result<#req_ty, Status>`. Wrapped in an immediately-invoked closure so the
/// `?` operator on the decode and each poke can early-return a status.
fn decode_and_poke(variant: &RouteVariant, req_ty: &TokenStream, body_kind: &TokenStream) -> TokenStream {
    let pokes = variant_pokes(variant);
    quote! {
        (|| -> ::core::result::Result<#req_ty, ::rest_over_grpc::handling::Status> {
            let mut request = ::rest_over_grpc::codegen_helpers::decode_request::<#req_ty>(&query_pairs, body, #body_kind)
                .map_err(::rest_over_grpc::codegen_helpers::TranscodeError::into_status)?;
            #pokes
            ::core::result::Result::Ok(request)
        })()
    }
}

/// Builds the `match matched` arms (one per route enum variant) for one method —
/// variants differ when `additional_bindings` capture different path variables,
/// since each carries a distinct field set and so cannot share a destructuring
/// pattern. Each arm destructures the matched variant, decodes the request
/// (body + query), and pokes the variant's captured path variables into it;
/// `receiver` is the handler field (e.g. `self.svc_0`), `req_ty` its
/// (module-qualified) request type.
///
/// Each arm evaluates to a
/// [`TranscodeResponse`](rest_over_grpc::transcoding::TranscodeResponse). A
/// server-streaming method yields a
/// [`TranscodeResponse::Streaming`](rest_over_grpc::transcoding::TranscodeResponse::Streaming)
/// on success (an initiation/decode failure stays a unary status response); a
/// unary method yields a
/// [`TranscodeResponse::Unary`](rest_over_grpc::transcoding::TranscodeResponse::Unary).
fn transcode_arm(variants: &[RouteVariant], receiver: &TokenStream, req_ty: &TokenStream, m: &ServiceMethod) -> TokenStream {
    let fn_ident = ident(&to_snake_case(m.rpc()));
    // A service method always has at least one route (each `HttpRule` lowers to
    // its primary binding plus any additional bindings).
    let primary = m
        .routes()
        .first()
        .expect("a service method always has at least one route from its HttpRule");
    let body_kind = body_kind_tokens(primary.body());

    if m.server_streaming() {
        let stream_encoding = stream_encoding_binding();
        let arms = variants.iter().map(|variant| {
            let pattern = variant_pattern(variant);
            let decoded = decode_and_poke(variant, req_ty, &body_kind);
            quote! {
                #pattern => {
                    #stream_encoding
                    match #decoded {
                        ::core::result::Result::Ok(request) => {
                            match #receiver.#fn_ident(request, &mut cx).await {
                                ::core::result::Result::Ok(stream) => {
                                    ::rest_over_grpc::transcoding::TranscodeResponse::Streaming(
                                        ::rest_over_grpc::transcoding::StreamingResponse::encode(
                                            stream,
                                            __stream_encoding,
                                        ),
                                    )
                                }
                                ::core::result::Result::Err(status) => {
                                    ::rest_over_grpc::transcoding::TranscodeResponse::Unary(
                                        ::rest_over_grpc::transcoding::HttpResponse::from_status(&status),
                                    )
                                }
                            }
                        }
                        ::core::result::Result::Err(status) => {
                            ::rest_over_grpc::transcoding::TranscodeResponse::Unary(
                                ::rest_over_grpc::transcoding::HttpResponse::from_status(&status),
                            )
                        }
                    }
                }
            }
        });
        return quote! { #(#arms)* };
    }

    let resp_kind = response_kind_tokens(primary.response_body());
    let arms = variants.iter().map(|variant| {
        let pattern = variant_pattern(variant);
        let decoded = decode_and_poke(variant, req_ty, &body_kind);
        quote! {
            #pattern => {
                let result: ::core::result::Result<::std::vec::Vec<u8>, ::rest_over_grpc::handling::Status> = async {
                    let request = #decoded?;

                    let response = #receiver.#fn_ident(request, &mut cx).await?;

                    let bytes = ::rest_over_grpc::codegen_helpers::encode_response(&response, #resp_kind)
                        .map_err(::rest_over_grpc::codegen_helpers::TranscodeError::into_status)?;

                    ::core::result::Result::<::std::vec::Vec<u8>, ::rest_over_grpc::handling::Status>::Ok(bytes)
                }
                .await;

                ::rest_over_grpc::transcoding::TranscodeResponse::Unary(match result {
                    ::core::result::Result::Ok(bytes) => ::rest_over_grpc::transcoding::HttpResponse::ok_json(bytes),
                    ::core::result::Result::Err(status) => ::rest_over_grpc::transcoding::HttpResponse::from_status(&status),
                })
            }
        }
    });
    quote! { #(#arms)* }
}

fn body_kind_tokens(body: &RequestBody) -> TokenStream {
    match body {
        RequestBody::None => quote! { ::rest_over_grpc::codegen_helpers::RequestBodyKind::None },
        RequestBody::Whole => quote! { ::rest_over_grpc::codegen_helpers::RequestBodyKind::Whole },
        RequestBody::Field(field) => quote! { ::rest_over_grpc::codegen_helpers::RequestBodyKind::Field(#field) },
    }
}

fn response_kind_tokens(response_body: &ResponseBody) -> TokenStream {
    match response_body {
        ResponseBody::Whole => quote! { ::rest_over_grpc::codegen_helpers::ResponseBodyKind::Whole },
        ResponseBody::Field(field) => {
            quote! { ::rest_over_grpc::codegen_helpers::ResponseBodyKind::Field(#field) }
        }
    }
}

/// Builds an identifier, raw-escaping (`r#name`) any name that is a reserved
/// Rust keyword. RPC method names go through here after `snake_case`ing, so a
/// proto RPC named `Match`/`Move`/`Type`/`Loop` becomes `r#match`/`r#move`/…
/// rather than a bare `fn match` that would not compile. This mirrors what
/// `tonic-prost-build` emits for the server trait, keeping the generated REST
/// trait and its `tonic` bridge in lockstep. Non-keyword names (`PascalCase`
/// trait names, `T0` generics, `__invoke_*` helpers) are unaffected.
fn ident(name: &str) -> Ident {
    if is_raw_keyword(name) {
        Ident::new_raw(name, Span::call_site())
    } else {
        Ident::new(name, Span::call_site())
    }
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
/// Converts a proto identifier to `snake_case`, matching the `heck`-based
/// conversion that `prost`/`tonic` use for module and field names — so
/// generated nested-type module paths and the `tonic` server-module name stay
/// consistent with them even for acronym-bearing names (e.g. `GetHTTPConfig` →
/// `get_http_config`, not `get_h_t_t_p_config`).
pub(crate) fn to_snake_case(name: &str) -> String {
    heck::AsSnakeCase(name).to_string()
}

#[cfg(test)]
#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "assertions match generated `{field:__capN}` destructuring patterns verbatim, not format args"
)]
mod tests {
    use http_path_template::Grammar;
    use routerama_build::HttpMethod;

    use super::*;
    use crate::build::generator::CodegenOptions;

    fn template(pattern: &str) -> PathTemplate {
        PathTemplate::parse(pattern, Grammar::default()).expect("valid path template")
    }

    fn rule(rpc: &str, http: HttpMethod, pattern: &str) -> HttpRule {
        HttpRule::new(rpc, http, template(pattern))
    }

    #[test]
    fn snake_case_conversion() {
        assert_eq!(to_snake_case("GetShelf"), "get_shelf");
        assert_eq!(to_snake_case("ListBooksByAuthor"), "list_books_by_author");
        assert_eq!(to_snake_case("already_snake"), "already_snake");
        // Acronyms match prost/tonic (heck) casing, not a naive per-capital split.
        assert_eq!(to_snake_case("GetHTTPConfig"), "get_http_config");
        assert_eq!(to_snake_case("MyHTTPService"), "my_http_service");
    }

    #[test]
    fn qualified_type_path_resolves_relative_and_rooted_paths() {
        let q = |path: &str, module: &str| qualified_type_path(path, module).to_string();

        // Same-package and nested types are prefixed with the package module.
        assert_eq!(q("GetShelfRequest", "library"), "library :: GetShelfRequest");
        assert_eq!(q("outer::Inner", "library"), "library :: outer :: Inner");
        // Multi-segment package.
        assert_eq!(q("Foo", "a.b"), "a :: b :: Foo");
        // An empty module leaves the (unrooted) path unqualified.
        assert_eq!(q("Foo", ""), "Foo");
        // `super::` steps up toward the common root: a parent-package type in `a.b`.
        assert_eq!(q("super::Foo", "a.b"), "a :: Foo");
        // A sibling-package type: two `super::`s then the type's own package path.
        assert_eq!(q("super::super::x::y::Foo", "a.b"), "x :: y :: Foo");
        // Rooted paths (extern overrides, crate-relative manual paths) pass through.
        assert_eq!(q("::prost_types::Empty", "library"), ":: prost_types :: Empty");
        assert_eq!(q("crate::pb::Shelf", "library_service"), "crate :: pb :: Shelf");
        // Every rooted prefix passes through unchanged — one per branch of the
        // disjunction so no single `||`/`==`/`starts_with` can be dropped silently.
        assert_eq!(q("crate", "library"), "crate");
        assert_eq!(q("self", "library"), "self");
        assert_eq!(q("self::Inner", "library"), "self :: Inner");
        // A `$crate::…` path is rooted, so it is never prefixed with the module.
        assert!(
            !q("$crate::Macro", "library").contains("library"),
            "$crate path must pass through unprefixed"
        );
    }

    #[test]
    fn route_name_is_a_unique_identifier() {
        // The name suffixes the RPC with the service index, keeping it a valid,
        // collision-free Rust identifier (it names a route enum variant).
        assert_eq!(route_name(0, "GetShelf"), "GetShelf_0");
        assert_eq!(route_name(3, "ListBooks"), "ListBooks_3");
    }

    #[test]
    fn a_service_less_transcoder_omits_generics_and_where_clause() {
        // With no services the transcoder has no generic type parameters and no
        // `where` clause (the empty-`generics`/`bounds` branches).
        let tokens = generate_transcoder(&[]);
        let file: syn::File = syn::parse2(tokens).expect("service-less transcoder is valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("struct Transcoder"), "{pretty}");
        // Scope the checks to the transcoder itself: the module-level `resolve`
        // helper (emitted earlier) carries its own generic `AsRef<str>` bounds.
        let transcoder = &pretty[pretty.find("struct Transcoder").expect("transcoder struct present")..];
        assert!(!transcoder.contains("T0"), "no generic parameters: {transcoder}");
        assert!(!transcoder.contains("where"), "no where clause: {transcoder}");
    }

    #[test]
    fn transcoder_field_name_suffixes_only_on_collision() {
        // Distinct trait names keep their plain snake_case field name.
        let distinct = vec![ServiceDefinition::new("Library", None), ServiceDefinition::new("Catalog", None)];
        assert_eq!(transcoder_field_name(&distinct, 0), "library");
        assert_eq!(transcoder_field_name(&distinct, 1), "catalog");

        // Two services with the same trait name are disambiguated by index.
        let colliding = vec![ServiceDefinition::new("Library", None), ServiceDefinition::new("Library", None)];
        assert_eq!(transcoder_field_name(&colliding, 0), "library_0");
        assert_eq!(transcoder_field_name(&colliding, 1), "library_1");
    }

    #[test]
    fn additional_bindings_with_different_captures_split_into_separate_arms() {
        // One RPC bound to two paths that capture different variables cannot share
        // a single field-carrying route enum variant, so each becomes its own
        // variant and its own destructuring transcode arm.
        let mut generator = ServiceDefinition::new("Library", None);
        generator.add_method(
            rule("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}").add_binding(crate::build::binding::Binding::new(
                HttpMethod::Get,
                template("/v1/shelves/{shelf}/books/{book}"),
            )),
            "crate::pb::GetShelfRequest",
            "crate::pb::Shelf",
            None,
        );

        let file: syn::File = syn::parse2(generator.generate(CodegenOptions::default())).expect("generated service must be valid Rust");
        let flat = prettyplease::unparse(&file).replace(' ', "");
        // Two distinct-capture bindings yield two struct variants.
        assert!(flat.contains("GetShelf_0{shelf:&'pstr}"), "{flat}");
        assert!(flat.contains("GetShelf_0_b1{shelf:&'pstr,book:&'pstr}"), "{flat}");
        // Each variant gets its own destructuring arm binding its fields to
        // reserved `__cap{n}` locals.
        assert!(flat.contains("Route::GetShelf_0{shelf:__cap0}=>"), "{flat}");
        assert!(flat.contains("Route::GetShelf_0_b1{shelf:__cap0,book:__cap1}=>"), "{flat}");
    }

    #[test]
    fn nested_path_variable_pokes_through_get_or_insert() {
        // A dotted path variable `{shelf.id}` pokes into a nested message field:
        // the non-leaf segment is materialized with `get_or_insert_with` and the
        // leaf is parsed via `parse_path_field`, with the value read from the
        // matched variant's `shelf_id` field.
        let mut generator = ServiceDefinition::new("Library", None);
        generator.add_method(
            rule("GetBook", HttpMethod::Get, "/v1/shelves/{shelf.id}/book"),
            "crate::pb::GetBookRequest",
            "crate::pb::Book",
            None,
        );

        let file: syn::File = syn::parse2(generator.generate(CodegenOptions::default())).expect("generated service must be valid Rust");
        let pretty = prettyplease::unparse(&file);
        // Collapse all whitespace so the (line-wrapped) poke chain matches.
        let flat: String = pretty.split_whitespace().collect();
        // The variant reads the `shelf_id` field into a reserved `__cap0` binding.
        assert!(flat.contains("Route::GetBook_0{shelf_id:__cap0}=>"), "{pretty}");
        // The poke walks `request.shelf.get_or_insert_with(..).id` and parses the
        // captured value into it.
        assert!(
            flat.contains("request.shelf.get_or_insert_with(::core::default::Default::default).id=::rest_over_grpc::codegen_helpers::parse_path_field(__cap0"),
            "{pretty}"
        );
    }

    #[test]
    fn keyword_named_rpc_emits_a_raw_identifier_method() {
        // A proto RPC whose `snake_case` name is a Rust keyword (e.g. `Match` →
        // `match`) must be emitted as a raw identifier (`fn r#match`); a bare
        // a bare `fn match` would not compile. `syn::parse2` here would fail on the
        // unescaped form, so this both round-trips the generated code and pins
        // the raw form in the pretty output.
        let mut generator = ServiceDefinition::new("Library", None);
        generator.add_method(
            rule("Match", HttpMethod::Get, "/v1/match"),
            "crate::pb::MatchRequest",
            "crate::pb::MatchResponse",
            None,
        );

        let file: syn::File =
            syn::parse2(generator.generate(CodegenOptions::default())).expect("keyword-named RPC must produce valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("fn r#match"), "{pretty}");
        assert!(!pretty.contains("fn match("), "keyword must be raw-escaped: {pretty}");
    }

    #[test]
    fn enum_path_variable_pokes_via_parse_path_enum_value() {
        // A path variable declared as an enum field is poked via
        // `parse_path_enum_value` with the field's concrete enum type (so it
        // accepts the value by name or number), with the `i32` `.into()`-ed to
        // fit the field; a non-declared capture stays on `parse_path_field`.
        let mut generator = ServiceDefinition::new("Library", None);
        generator.add_method(
            rule("ListByState", HttpMethod::Get, "/v1/books/state/{state}").path_field_enum("state", "crate::pb::BookState"),
            "crate::pb::ListByStateRequest",
            "crate::pb::ListByStateResponse",
            None,
        );

        let file: syn::File = syn::parse2(generator.generate(CodegenOptions::default())).expect("generated service must be valid Rust");
        let pretty = prettyplease::unparse(&file);
        let flat: String = pretty.split_whitespace().collect();
        assert!(
            flat.contains(
                "request.state=::rest_over_grpc::codegen_helpers::parse_path_enum_value(__cap0,crate::pb::BookState::from_str_name"
            ),
            "{pretty}"
        );
        assert!(flat.contains(".into();"), "{pretty}");
    }

    #[test]
    fn path_variable_named_like_a_transcoder_local_does_not_collide() {
        // A path variable whose proto field name equals a transcoder local
        // (`request`, `body`, `cx`, `query_pairs`) must be destructured into a
        // reserved `__cap{n}` binding, so it cannot shadow that local and break
        // the generated `decode_request`/poke code.
        let mut generator = ServiceDefinition::new("Library", None);
        generator.add_method(
            rule("Get", HttpMethod::Get, "/v1/{request}"),
            "crate::pb::GetRequest",
            "crate::pb::Resp",
            None,
        );

        let file: syn::File = syn::parse2(generator.generate(CodegenOptions::default())).expect("generated service must be valid Rust");
        let flat: String = prettyplease::unparse(&file).split_whitespace().collect();
        // The `request` field is read into `__cap0` (not a bare `request` binding
        // that would shadow the decoded message local).
        assert!(flat.contains("{request:__cap0}=>"), "{flat}");
        assert!(
            flat.contains("request.request=::rest_over_grpc::codegen_helpers::parse_path_field(__cap0"),
            "{flat}"
        );
    }

    #[test]
    fn keyword_path_segment_pokes_through_a_raw_identifier() {
        // A dotted path variable whose segment is a Rust keyword (`type`) is poked
        // through a raw identifier (`r#type`) so the generated code compiles.
        let mut generator = ServiceDefinition::new("Library", None);
        generator.add_method(
            rule("GetTyped", HttpMethod::Get, "/v1/{msg.type}"),
            "crate::pb::GetTypedRequest",
            "crate::pb::Resp",
            None,
        );

        let file: syn::File = syn::parse2(generator.generate(CodegenOptions::default())).expect("generated service must be valid Rust");
        let flat: String = prettyplease::unparse(&file).split_whitespace().collect();
        assert!(
            flat.contains(".r#type=::rest_over_grpc::codegen_helpers::parse_path_field(__cap0"),
            "{flat}"
        );
    }

    #[test]
    fn affix_path_segment_is_captured_and_poked() {
        // An affix segment (`img-{id}.png`, from the extended template grammar)
        // captures its field like a plain variable; the transcoder destructures
        // and pokes it. Exercises the affix arm of `capture_signature`.
        let template =
            PathTemplate::parse("/v1/images/img-{id}.png", Grammar::default().with_segment_affixes()).expect("valid extended template");
        let mut generator = ServiceDefinition::new("Library", None);
        generator.add_method(
            HttpRule::new("GetImage", HttpMethod::Get, template),
            "crate::pb::GetImageRequest",
            "crate::pb::Resp",
            None,
        );

        let file: syn::File = syn::parse2(generator.generate(CodegenOptions::default())).expect("generated service must be valid Rust");
        let flat: String = prettyplease::unparse(&file).split_whitespace().collect();
        assert!(
            flat.contains("request.id=::rest_over_grpc::codegen_helpers::parse_path_field(__cap0"),
            "{flat}"
        );
    }

    #[test]
    fn generates_valid_service_code() {
        let mut generator = ServiceDefinition::new("Library", None);
        generator
            .add_method(
                rule("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}"),
                "crate::pb::GetShelfRequest",
                "crate::pb::Shelf",
                None,
            )
            .add_method(
                rule("CreateShelf", HttpMethod::Post, "/v1/shelves"),
                "crate::pb::CreateShelfRequest",
                "crate::pb::Shelf",
                None,
            );

        let file: syn::File = syn::parse2(generator.generate(CodegenOptions::default())).expect("generated service must be valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("pub trait Library"));
        assert!(pretty.contains("fn get_shelf"));
        assert!(pretty.contains("fn try_transcode"));
        assert!(pretty.contains("GetShelfRequest"));
    }

    #[test]
    fn service_doc_documents_the_trait() {
        // A `Some(doc)` documents the trait verbatim (one `#[doc]` per line);
        // `None` emits no doc comment.
        let documented = ServiceDefinition::new("Library", Some(" A library service.\n Second line.".to_owned()))
            .trait_code()
            .to_string();
        assert!(documented.contains("A library service."), "{documented}");
        assert!(documented.contains("Second line."), "{documented}");

        let undocumented = ServiceDefinition::new("Library", None).trait_code().to_string();
        assert!(!undocumented.contains("#[doc"), "{undocumented}");
        assert!(!undocumented.contains("doc ="), "{undocumented}");
    }

    #[test]
    fn generates_streaming_transcode_for_server_streaming_methods() {
        // A server-streaming method makes the generated `Transcoder`'s
        // `try_transcode` emit a streaming arm; a unary sibling exercises
        // the unary arm of the same method too.
        let mut generator = ServiceDefinition::new("Library", None);
        generator
            .add_server_streaming_method(
                rule("StreamShelves", HttpMethod::Get, "/v1/shelves:stream"),
                "crate::pb::ListShelvesRequest",
                "crate::pb::Shelf",
                None,
            )
            .add_method(
                rule("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}"),
                "crate::pb::GetShelfRequest",
                "crate::pb::Shelf",
                None,
            );

        let file: syn::File =
            syn::parse2(generator.generate(CodegenOptions::default())).expect("generated streaming service must be valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("fn stream_shelves"));
        // The transcoder exposes a single `try_transcode` returning a
        // `TranscodeResponse`; there is no separate streaming method.
        assert!(pretty.contains("fn try_transcode"));
        assert!(!pretty.contains("fn try_transcode_streaming"));
        // The streaming arm frames the handler's stream — a token unique to the
        // streaming arm (the unary arm produces a buffered `HttpResponse`).
        assert!(pretty.replace(' ', "").contains("StreamingResponse::encode"));
    }

    #[test]
    fn tonic_bridge_streams_server_streaming_methods() {
        let mut generator = ServiceDefinition::new("Library", None);
        generator.add_server_streaming_method(
            rule("StreamShelves", HttpMethod::Get, "/v1/shelves:stream"),
            "crate::pb::ListShelvesRequest",
            "crate::pb::Shelf",
            None,
        );

        let bridge = generator.tonic_bridge();
        let _: syn::File = syn::parse2(bridge.clone()).expect("streaming bridge must be valid Rust");
        let flat = bridge.to_string().replace(' ', "");

        // The bridged method is two-phase: an `async fn` returning
        // `Result<ResponseStream<_>, Status>`.
        assert!(flat.contains(
            "asyncfnstream_shelves(&self,request:crate::pb::ListShelvesRequest,cx:&mut::rest_over_grpc::handling::Context,)->::core::result::Result<::rest_over_grpc::handling::ResponseStream<crate::pb::Shelf>,::rest_over_grpc::handling::Status,>"
        ));
        // It awaits the tonic call, boxes the response stream, and maps per-item
        // errors via the runtime helper.
        assert!(flat.contains("::rest_over_grpc::codegen_helpers::map_stream_status("));
        assert!(flat.contains(".into_inner()"));
        assert!(flat.contains("__convert_status"));
        // The request headers seed the tonic request metadata (moved, not cloned).
        assert!(flat.contains("::tonic::metadata::MetadataMap::from_headers(cx.take_request_headers())"));
    }

    #[test]
    fn tonic_bridge_forwards_to_server_trait() {
        let mut generator = ServiceDefinition::new("Library", None);
        generator.add_method(
            rule("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}"),
            "crate::pb::GetShelfRequest",
            "crate::pb::Shelf",
            None,
        );

        let bridge = generator.tonic_bridge();
        let file: syn::File = syn::parse2(bridge).expect("bridge must be valid Rust");
        let pretty = prettyplease::unparse(&file);

        assert!(pretty.contains("impl<T> Library for T"));
        assert!(pretty.contains("T: library_server::Library"));
        assert!(pretty.contains("tonic::Request::new(request)"));
        // The request headers seed the tonic request metadata.
        assert!(pretty.contains("MetadataMap::from_headers"));
        assert!(pretty.contains("request_headers()"));
        assert!(pretty.contains("response.into_parts()"));
        assert!(pretty.contains("merge_response_headers"));
        assert!(pretty.contains("rest_over_grpc::handling::Status::new"));
        assert!(pretty.contains("Code::from_i32"));
    }

    #[test]
    fn generate_gates_the_tonic_bridge_on_the_flag() {
        let mut definition = ServiceDefinition::new("Library", None);
        definition.add_method(
            rule("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}"),
            "crate::pb::GetShelfRequest",
            "crate::pb::Shelf",
            None,
        );

        // With `emit_tonic` unset, `generate` emits no bridge `impl`.
        assert!(
            !definition
                .generate(CodegenOptions::default())
                .to_string()
                .replace(' ', "")
                .contains("library_server::Library")
        );

        // With `emit_tonic` set, the bridge `impl` is included alongside the trait.
        assert!(
            definition
                .generate(CodegenOptions { emit_tonic: true })
                .to_string()
                .replace(' ', "")
                .contains("library_server::Library")
        );
    }

    #[test]
    fn generate_always_emits_send_bounds() {
        let mut definition = ServiceDefinition::new("Library", None);
        definition.add_method(
            rule("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}"),
            "crate::pb::GetShelfRequest",
            "crate::pb::Shelf",
            None,
        );

        // The service trait always gains a `Send + Sync` supertrait and per-future
        // `+ Send` bounds so the output works on multi-threaded executors.
        let code = definition.generate(CodegenOptions::default()).to_string();
        assert!(code.contains("Send + :: core :: marker :: Sync") || code.contains("Send + ::core::marker::Sync"));
        assert!(code.matches("marker :: Send").count() >= 2);
    }

    #[test]
    fn invalid_type_path_yields_compile_error() {
        let mut generator = ServiceDefinition::new("S", None);
        generator.add_method(rule("Run", HttpMethod::Get, "/v1/x"), "crate::Resp (", "crate::Resp", None);
        let pretty =
            prettyplease::unparse(&syn::parse2(generator.generate(CodegenOptions::default())).expect("still parses with compile_error!"));
        assert!(pretty.contains("compile_error!"));
    }

    #[test]
    fn response_body_and_body_field_kinds_are_generated() {
        let create = HttpRule::new("CreateBook", HttpMethod::Post, template("/v1/books"))
            .request_body(RequestBody::Whole)
            .response_body(ResponseBody::Field("book".into()));
        let update =
            HttpRule::new("UpdateBook", HttpMethod::Patch, template("/v1/books/{book}")).request_body(RequestBody::Field("book".into()));

        let mut generator = ServiceDefinition::new("Library", None);
        generator
            .add_method(create, "crate::pb::CreateBookRequest", "crate::pb::Book", None)
            .add_method(update, "crate::pb::UpdateBookRequest", "crate::pb::Book", None);

        let tokens = generator.generate(CodegenOptions::default());
        let _: syn::File = syn::parse2(tokens.clone()).expect("generated service must be valid Rust");
        let flat = tokens.to_string().replace(' ', "");
        assert!(flat.contains("RequestBodyKind::Whole"));
        assert!(flat.contains("RequestBodyKind::Field(\"book\")"));
        assert!(flat.contains("ResponseBodyKind::Field(\"book\")"));
    }

    #[test]
    fn invalid_response_type_path_yields_compile_error() {
        let mut generator = ServiceDefinition::new("S", None);
        generator.add_method(rule("Run", HttpMethod::Get, "/v1/x"), "crate::Req", "crate::Resp (", None);
        let pretty =
            prettyplease::unparse(&syn::parse2(generator.generate(CodegenOptions::default())).expect("still parses with compile_error!"));
        assert!(pretty.contains("invalid type path generated for a service method"));
    }

    #[test]
    fn default_bodies_use_default_transcoding_kinds() {
        let mut generator = ServiceDefinition::new("S", None);
        generator.add_method(rule("Run", HttpMethod::Get, "/v1/x"), "crate::Req", "crate::Resp", None);

        let tokens = generator.generate(CodegenOptions::default());
        let _: syn::File = syn::parse2(tokens.clone()).expect("generated service must be valid Rust");
        let flat = tokens.to_string().replace(' ', "");
        assert!(flat.contains("RequestBodyKind::None"));
        assert!(flat.contains("ResponseBodyKind::Whole"));
    }
}
