// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP request extraction, response composition, and generated handler routing.
//!
//! [`router`] turns annotated inherent methods into a static or startup-built
//! router. Generated entry points consume an [`http::Request`], resolve its
//! method and URI path, extract request metadata and one explicitly marked
//! body, call the selected method directly, and return a
//! [`Response`](crate::response::Response).
//!
//! Static path captures remain ordinary handler parameters whose names are
//! checked against the template. Every other unmarked parameter implements
//! [`FromRequestParts`]. A single position-independent `#[body]` parameter
//! implements [`FromRequestBody`]; duplicate body markers are rejected by the
//! macro. All parts extractors run before that body moves, while the direct
//! handler call retains its declared argument order. Dynamic captures use an
//! explicit `#[capture]` marker.
//! Bare `#[router]` keeps the supplied shared state type generic. Optional
//! `#[router(state = AppState)]` instead fixes the service contract and checks
//! its state-dependent extractors when the annotated impl is defined.
//! The separately enabled `mount` feature adds
//! [`mount`](https://docs.rs/routerama/latest/routerama/route/mount/), whose
//! explicitly
//! erased services are parameterized by one request-body and state type.
//! `#[router(state = AppState, erased_mounts)]` explicitly generates
//! `route_with_erased_mounts`; generated static and configured-dynamic routes
//! run first, and only a complete miss delegates. The returned structural
//! `EitherBody<Generated, BoxBody>` leaves the generated branch unboxed. This
//! opt-in entry does not change the ordinary `route` signature or generated
//! body, and enabling the feature alone changes no generated service.
//!
//! Built-in parts extraction covers owned [`Method`], [`Uri`], [`Version`],
//! and [`HeaderMap`] values; zero-copy references to those values,
//! [`Extensions`], and [`RequestParts`]; [`State`]; and typed
//! [`ExtensionRef`] or [`ClonedExtension`] values. Owned URI and header-map
//! extraction clones explicitly, while version is copied and method is
//! cloned. `Query<T>` is present only when `query` is enabled alongside
//! `route` and may borrow from the URI. Parts extraction is synchronous. Body
//! extraction returns a concrete future through return-position `impl Trait`,
//! without boxing or a mandatory [`Send`] bound.
//!
//! Handler results retain [`IntoResponse::Body`](crate::response::IntoResponse::Body).
//! Strings, bytes, status codes, and other built-ins use the efficient
//! zero-or-one-frame [`Body`](crate::response::Body), while
//! `http::Response<B>` retains a concrete streaming `B`. Each service gets one
//! private generated sum over its handler, extractor-rejection, and routing
//! bodies; static and configured-dynamic branches use the same sum. Generated
//! entry points expose it as
//! `Response<impl http_body::Body<Data = bytes::Bytes, Error = impl Error>>`.
//! Rust 2024 precise captures retain only the request-body and state *types*,
//! not the borrowed service/state values, so transport adapters can require a
//! `'static` response when every variant supports it. No private generated
//! symbol leaks through the public API.
//!
//! Polling delegates directly to the active body. Data frames, trailers,
//! end-of-stream state, size hints, and body errors are preserved without a
//! per-response allocation or dynamic dispatch. Source errors are retained in
//! an opaque generated error sum whose `Display` identifies the response
//! source. Auto traits such as `Send` and `Sync` reflect every body/error
//! variant in the service; transport adapters add their required bounds.
//! [`BoxBody`](crate::response::BoxBody) is the explicit allocating and
//! dynamically dispatched escape hatch for body sets unknown to the macro.
//!
//! # Generic and fixed shared state
//!
//! Bare `#[router]` generates `route<B, S>(request, &S)` and is appropriate for
//! reusable services whose handlers work with multiple state types.
//! `#[router(state = AppState)]` generates `route<B>(request, &AppState)`.
//! Configured dynamic routers use the same distinction for their
//! `router.route(&service, request, state)` entry.
//!
//! A fixed contract makes every request-parts extractor obligation concrete.
//! A generated private associated function also instantiates each extractor
//! across its request lifetimes, which forces rustc to prove the otherwise
//! higher-ranked contract at the service definition. This validates direct
//! `State<AppState>` cloning, every [`FromRef`] projection, custom borrowed
//! parts extractors, and catcher-associated extractor bounds even if no route
//! call exists:
//!
//! ```
//! use routerama::route::{FromRef, Request, State, StatusCode, router};
//!
//! #[derive(Clone)]
//! struct AppState {
//!     label: &'static str,
//!     revision: u32,
//! }
//!
//! #[derive(Clone, Copy)]
//! struct Revision(u32);
//!
//! impl FromRef<AppState> for Revision {
//!     fn from_ref(state: &AppState) -> Self {
//!         Self(state.revision)
//!     }
//! }
//!
//! struct Api;
//!
//! #[router(state = AppState)]
//! impl Api {
//!     #[route(GET, "/")]
//!     async fn home(&self, state: State<AppState>, revision: State<Revision>) -> String {
//!         format!("{}:{}", state.label, revision.0.0)
//!     }
//!
//!     #[route(GET, "/health")]
//!     async fn health(&self) -> StatusCode {
//!         StatusCode::NO_CONTENT
//!     }
//! }
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() {
//!     let request = Request::get("/").body(()).expect("valid request");
//!     let state = AppState {
//!         label: "fixed",
//!         revision: 7,
//!     };
//!     let response = Api.route(request, &state).await;
//!     assert_eq!(response.status(), StatusCode::OK);
//! }
//! ```
//!
//! A custom body extractor has an additional type variable: the transport
//! request body `B`. Rust cannot prove an existential
//! `FromRequestBody<AppState, B>` obligation from a generic route bound alone.
//! Such an extractor therefore implements
//! [`BodyStateWitness<AppState, Rejection>`](BodyStateWitness) and names one
//! supported [`BodyStateWitness::RequestBody`]. The generated private assertion
//! proves the corresponding [`FromRequestBody`] implementation and exact
//! rejection at definition time; each call still proves the real request-body
//! type. Built-in raw, bytes, text, JSON, and form extractors provide their own
//! witnesses.
//! Bare routers require no witness and retain their existing generic output.
//!
//! The attribute accepts optional `state = Type` and the `erased_mounts`
//! marker, which requires fixed state and the `mount` feature. The state type
//! may be a qualified or generic concrete type. References require an explicit
//! `'static` lifetime, and trait objects require `dyn Trait + 'static`.
//! Anonymous `'_`, inferred `_`, `impl Trait`, type macros, trailing junk, and
//! unknown arguments are rejected. `Self` is rejected because generated
//! configured-router methods have a different `Self`; name the service or use
//! a fully qualified associated type. A private generated type alias also makes
//! omitted lifetime parameters and other ill-formed types fail at the
//! annotation. Slices, `str`, and explicit `'static` trait objects remain
//! valid because generated entries borrow state and extraction traits accept
//! `S: ?Sized`.
//!
//! Fixed-state validation adds only dead compile-time witness items. The route
//! body performs the same direct extraction and handler calls as a
//! monomorphized bare router: there is no runtime lookup, registry, allocation,
//! branch, or type-map access. See the runnable `required_state` example for
//! multiple projections and a custom state-dependent borrowed extractor.
//!
//! # Host and media predicates
//!
//! Static and configured-dynamic handlers can declare request predicates after
//! their method/path or `dynamic` marker:
//!
//! ```
//! use http::header::{ACCEPT, CONTENT_TYPE, HOST};
//! use routerama::route::{Request, StatusCode, router};
//!
//! struct Items;
//!
//! #[router]
//! impl Items {
//!     #[route(
//!         POST,
//!         "/items",
//!         host = "api.example",
//!         consumes = "application/json",
//!         produces = "application/json"
//!     )]
//!     async fn create(&self) -> &'static str {
//!         r#"{"created":true}"#
//!     }
//! }
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() {
//!     let request = Request::post("/items")
//!         .header(HOST, "API.EXAMPLE")
//!         .header(CONTENT_TYPE, "Application/JSON; charset=utf-8")
//!         .header(ACCEPT, "application/*")
//!         .body(())
//!         .expect("static request metadata is valid");
//!     let response = Items.route(request, &()).await;
//!     assert_eq!(response.status(), StatusCode::OK);
//!     assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
//! }
//! ```
//!
//! Predicate keys may appear in any order and may have a trailing comma.
//! `#[route(dynamic, host = "...", ...)]` applies the same contract to every
//! configured registration of that handler. Static aliases on one handler
//! must declare exactly the same predicate values because resolution produces
//! one shared route variant. These predicates belong only to [`router`];
//! [`resolver`](https://docs.rs/routerama/latest/routerama/resolve/attr.resolver.html)
//! declarations match method and path and reject predicate arguments.
//!
//! Host matching uses the URI authority when present and otherwise requires
//! exactly one `Host` field. The complete authority, including an explicit
//! port, is compared ASCII-case-insensitively without default-port or percent
//! normalization. IPv6 addresses use brackets (`[2001:db8::1]`) and an
//! optional decimal port in the `0..=65535` range. IPv6 zone identifiers use
//! URI `%25` form; `IPvFuture` literals are also accepted. Empty hosts, schemes,
//! paths, userinfo, whitespace, IPv6 without brackets, invalid ports, missing
//! values, and duplicate `Host` fields reject with `404 Not Found`.
//!
//! `consumes` and `produces` declarations are compile-time-validated concrete
//! `type/subtype` values: wildcards and parameters are not allowed.
//! `consumes` requires exactly one valid `Content-Type`. Its type and subtype
//! compare ASCII-case-insensitively, while legal OWS and parameters are
//! validated and ignored. Missing, duplicate, malformed, or non-matching values
//! reject with `415 Unsupported Media Type`; the body is never inspected.
//!
//! A missing `Accept` is acceptable. Otherwise every field line and
//! comma-separated media range is parsed without allocation, including type
//! and global wildcards, parameters, quoted values, extensions, and
//! `q=0..1`. The most specific matching range determines quality, so an exact
//! `q=0` overrides an acceptable broader wildcard. Because a declared
//! representation has no parameters, a media range with representation
//! parameters does not match it; extensions after `q` do not constrain it.
//! Malformed fields or no range with nonzero quality reject with
//! `406 Not Acceptable`.
//!
//! After path/capture resolution, checks run in fixed host, consumes, produces
//! order before every request-parts extractor, body extractor, and handler.
//! A handler reached through `produces` has its response `Content-Type`
//! replaced with the declared static value after `IntoResponse` conversion;
//! headers, streaming data, trailers, and body errors otherwise remain
//! unchanged. Arbitrary `IntoResponse` implementations erase whether an
//! application value represented success or error, so this deterministic rule
//! applies to every response returned by an invoked handler, regardless of its
//! status. Routing, predicate, and extractor rejections are returned before
//! that mutation and never receive produced metadata. Handlers with no
//! predicates emit no predicate calls, header lookups, or predicate branches.
//!
//! See the runnable `request_predicates` example for the complete 404/415/406
//! ladder and for the way `priority`, not `Accept` quality, chooses between
//! overlapping candidates.
//!
//! # Priority, fallbacks, and extractor catchers
//!
//! `priority = <i32>` makes an otherwise conflicting method/path-template
//! shape intentional. Every declaration in an overlap must state a distinct
//! priority and use the same capture names, positions, and concrete Rust
//! types. Higher values are considered first. Each candidate evaluates host,
//! consumes, and produces in that order; a mismatch continues to the next
//! candidate, and no request-parts or body extractor runs until one candidate
//! is selected. If none succeeds, the deepest stage reached by any candidate
//! deterministically yields host `404`, consumes `415`, or produces `406`.
//!
//! ```rust
//! use routerama::route::{Request, RouteFailure, StatusCode, router};
//!
//! struct Items;
//!
//! #[router]
//! impl Items {
//!     #[route(GET, "/items/{id}", host = "api.example", priority = 20)]
//!     async fn api(&self, id: u32) -> (StatusCode, String) {
//!         (StatusCode::OK, format!("api:{id}"))
//!     }
//!
//!     #[route(GET, "/items/{id}", host = "admin.example", priority = 10)]
//!     async fn admin(&self, id: u32) -> (StatusCode, String) {
//!         (StatusCode::ACCEPTED, format!("admin:{id}"))
//!     }
//!
//!     #[fallback]
//!     async fn fallback(&self, failure: RouteFailure<'_>) -> StatusCode {
//!         failure.status()
//!     }
//! }
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() {
//!     let selected = Request::get("/items/7")
//!         .header("host", "ADMIN.EXAMPLE")
//!         .body(())
//!         .expect("static request metadata is valid");
//!     assert_eq!(
//!         Items.route(selected, &()).await.status(),
//!         StatusCode::ACCEPTED
//!     );
//!
//!     let missing = Request::get("/missing")
//!         .body(())
//!         .expect("static request metadata is valid");
//!     assert_eq!(
//!         Items.route(missing, &()).await.status(),
//!         StatusCode::NOT_FOUND
//!     );
//! }
//! ```
//!
//! One `#[fallback]` method may receive [`RouteFailure<'_>`](RouteFailure) by
//! value. It covers not found, malformed path, capture conversion, and all
//! three predicate classes. Without it, [`RouteFailure::status`] supplies the
//! defaults above (capture and malformed-path failures are `400`). The value
//! borrows path diagnostics and retains static capture names without
//! allocation.
//!
//! An extractor catcher has the form
//! `#[catch(RejectionType)] async fn catch(&self, rejection:
//! RejectionType) -> R`. Built-in query, extension, bounded-body, JSON, and
//! form rejection families are associated automatically. Because a procedural
//! macro cannot resolve an arbitrary extractor's associated type, custom
//! extractors use
//! `#[catch(RejectionType, from = ExtractorType)]`. The annotation and
//! by-value parameter must be the exact same concrete, request-independent
//! type. Duplicate, ambiguous, unused, generic, and lifetime-dependent
//! catchers are rejected. A concrete bounded-body/JSON/form catcher also makes
//! the transport error in its rejection type part of the generated route
//! bound; for example, `BodyRejection<Infallible>` accepts request bodies whose
//! body error is `Infallible`.
//!
//! Catchers replace only the selected extraction site's rejection conversion;
//! uncaught rejections retain their ordinary
//! [`IntoResponse`](crate::response::IntoResponse) behavior. Both
//! catcher and fallback futures are awaited directly and need not be `Send`.
//! Their concrete response bodies and errors enter the same private
//! service-specific sum as handler bodies, preserving frames and trailers
//! without boxing.
//!
//! Ordinary routes retain their existing direct match arm and contain no
//! candidate state, table, allocation, or indirect call. Static aliases are
//! grouped independently: only an alias whose method/template shape collides
//! participates in candidate selection. Configured dynamic handlers may use
//! predicates and catchers/fallbacks, but reject `priority`; dynamic/dynamic
//! and static/dynamic collisions fail router construction rather than adding a
//! runtime candidate layer.
//!
//! # Generated interceptors
//!
//! Three method attributes add cross-cutting behavior that the macro calls
//! directly, with no boxed future or service and no per-request allocation.
//! They complement extractors: extractors produce handler parameters, while
//! interceptors express request enrichment, guarding, response mutation, and
//! request-body ownership.
//!
//! ## `#[before]`: enrich, guard, or short-circuit a request
//!
//! A [`#[before]`](macro@router) method returns [`Before<R>`](Before) and
//! either continues with [`Before::Next`] or short-circuits with
//! [`Before::Respond`], whose `R` enters the same concrete response body sum as
//! any handler response. Its *scope* selects its context type:
//!
//! - a bare `#[before]` is **router-wide**. It takes a mutable
//!   [`BeforeContext`], runs at every generated entry *before* route
//!   resolution, and owns the whole request head, so it may rewrite the method
//!   or URI and change routing. Because it runs before resolution, it also
//!   enriches and guards mounted delegation;
//! - a `#[before(handler, ...)]` is **per-handler**. It takes a mutable
//!   [`SelectedContext`] and runs inside the selected dispatch arm, after
//!   predicate selection and before extraction. That context borrows the
//!   request head by field — the method, URI, and version are readable, the
//!   headers and extensions are mutable — so a guard composes with handlers
//!   that take zero-copy borrowed `&str` captures and [`ExtensionRef`]
//!   parameters. Rewriting the URI after selection could not change routing
//!   anyway.
//!
//! ## `#[after]`: mutate a generated response
//!
//! An [`#[after]`](macro@router) method takes a mutable [`AfterContext`]
//! (immutable request head, mutable response head) and returns `()`. It can
//! change the response status, headers, and extensions. It never sees either
//! body, so streaming responses keep their frames, trailers, and error type.
//!
//! Its scope is exact and deliberately narrow in one place only:
//!
//! - a bare `#[after]` observes **every response this router generates**:
//!   handler responses, `#[before]` and `#[transform]` short-circuits,
//!   request-parts and request-body extractor rejections, `#[catch]` responses,
//!   route predicate rejections, and routing failures or `#[fallback]`
//!   responses;
//! - a bare `#[after]` does **not** observe a response produced by a *mounted*
//!   service, because `route_with_erased_mounts` moves the request head into
//!   that service and the context borrows it. A mount-table miss (`404`) is
//!   likewise produced by the mount router, not by generated code;
//! - an `#[after(handler, ...)]` observes only the responses its named handlers
//!   returned, and runs before any bare `#[after]`.
//!
//! ## `#[transform]`: the terminal request-body owner
//!
//! A [`#[transform]`](macro@router) method takes a shared [`RequestParts`]
//! reference plus the request body, and returns
//! [`BodyTransform<B, R>`](BodyTransform) (a replacement body `B` for `#[body]`
//! extraction, or a short-circuit `R`) or [`BodyConsumed<R>`](BodyConsumed) (no
//! replacement). It owns the request body of the handlers it names, in one of
//! two explicit modes:
//!
//! - `#[transform(limit = N, handler, ...)]` **buffers**. The generated entry
//!   collects the request body into [`bytes::Bytes`](bytes), bounded by the
//!   explicit `N`, exactly like the bounded body extractors, and hands it to an
//!   interceptor that returns a concrete replacement body. Use it for terminal
//!   processing that genuinely needs the whole body;
//! - `#[transform(stream, handler, ...)]` **streams**. The interceptor is
//!   generic over the transport body and receives it by value:
//!
//!   ```text
//!   #[transform(stream, handler)]
//!   async fn wrap<B>(&self, parts: &RequestParts, body: B) -> BodyTransform<Wrapper<B>, R>
//!   where
//!       B: http_body::Body<Data = Bytes>;
//!   ```
//!
//!   The macro substitutes the router's transport body type for `B`, so the
//!   handler's `#[body]` parameter is checked against `Wrapper<TransportBody>`.
//!   Nothing is buffered, boxed, or allocated by the framework, and the call
//!   stays a direct concrete call on a monomorphized future, so a `!Send` body
//!   or future is fine. This is the mode for decompression, signature
//!   verification, metering, and other pass-through wrappers.
//!
//! Only a `#[transform]` may observe or consume the request body; `#[before]`
//! and `#[after]` are parts-only and cannot touch it.
//!
//! ## Ordering
//!
//! Ordering is deterministic: router-wide `#[before]` methods run first in
//! declaration order, then per-handler `#[before]`, then the handler's
//! `#[transform]`, then extraction and the handler; afterwards per-handler
//! `#[after]` run in declaration order, then any bare `#[after]`. A `#[before]`
//! or `#[transform]` short-circuit skips the handler, the extractors, and every
//! per-handler `#[after]`, but a bare `#[after]` still observes the
//! short-circuit response.
//!
//! ## Request-body ownership
//!
//! Request-body ownership is one compile-time plan:
//!
//! - each handler is named by at most one `#[transform]`; a second one is a
//!   compile error;
//! - a transform that returns [`BodyTransform::Replace`] hands its replacement
//!   to that handler's `#[body]` extraction, which is bound to the replacement
//!   type rather than to the transport body;
//! - a transform that returns [`BodyConsumed`] is the terminal consumer, so a
//!   handler it names must not declare `#[body]`; that combination is a compile
//!   error naming the fix;
//! - a handler with no transform extracts `#[body]` straight from the transport
//!   body, and a route with neither never touches the body at all.
//!
//! ```
//! use bytes::Bytes;
//! use routerama::response::Body;
//! use routerama::route::{
//!     AfterContext, Before, BeforeContext, BodyTransform, BytesBody, ClonedExtension, Request,
//!     RequestParts, SelectedContext, StatusCode, router,
//! };
//!
//! #[derive(Clone, Copy)]
//! struct Caller(u32);
//!
//! /// A streaming wrapper that counts request bytes without buffering them.
//! struct Metered<B> {
//!     inner: B,
//! }
//!
//! impl<B> http_body::Body for Metered<B>
//! where
//!     B: http_body::Body<Data = Bytes> + Unpin,
//! {
//!     type Data = Bytes;
//!     type Error = B::Error;
//!
//!     fn poll_frame(
//!         self: std::pin::Pin<&mut Self>,
//!         cx: &mut std::task::Context<'_>,
//!     ) -> std::task::Poll<Option<Result<http_body::Frame<Bytes>, B::Error>>> {
//!         std::pin::Pin::new(&mut self.get_mut().inner).poll_frame(cx)
//!     }
//! }
//!
//! struct Api;
//!
//! #[router]
//! impl Api {
//!     #[route(GET, "/me")]
//!     async fn me(&self, caller: ClonedExtension<Caller>) -> String {
//!         format!("caller {}", caller.0.0)
//!     }
//!
//!     #[route(POST, "/notes/{slug}")]
//!     async fn create(&self, slug: &str, #[body] note: BytesBody<64>) -> String {
//!         format!("{slug}:{}", note.as_bytes().len())
//!     }
//!
//!     /// Router-wide: runs before routing and owns the whole request head.
//!     #[before]
//!     async fn authenticate(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
//!         match ctx.headers().get("authorization") {
//!             Some(_) => {
//!                 ctx.insert_extension(Caller(7));
//!                 Before::Next
//!             }
//!             None => Before::Respond(StatusCode::UNAUTHORIZED),
//!         }
//!     }
//!
//!     /// Per-handler: runs after selection, so `create` keeps its borrowed
//!     /// `slug` capture while this guard mutates headers and extensions.
//!     #[before(create)]
//!     async fn guard(&self, ctx: &mut SelectedContext<'_>) -> Before<StatusCode> {
//!         if ctx.uri().path().ends_with("/private") {
//!             return Before::Respond(StatusCode::FORBIDDEN);
//!         }
//!         ctx.insert_extension(Caller(9));
//!         Before::Next
//!     }
//!
//!     /// Streaming: wraps the transport body, so `#[body]` above extracts
//!     /// from `Metered<TransportBody>` with no buffering.
//!     #[transform(stream, create)]
//!     async fn meter<B>(
//!         &self,
//!         _parts: &RequestParts,
//!         body: B,
//!     ) -> BodyTransform<Metered<B>, StatusCode>
//!     where
//!         B: http_body::Body<Data = Bytes>,
//!     {
//!         BodyTransform::Replace(Metered { inner: body })
//!     }
//!
//!     /// Observes every generated response, including the `401` above.
//!     #[after]
//!     async fn stamp(&self, ctx: &mut AfterContext<'_>) {
//!         ctx.headers_mut()
//!             .insert("x-served-by", "routerama".parse().expect("valid header"));
//!     }
//! }
//! # async fn example() {
//! let ok = Api
//!     .route(
//!         Request::get("/me")
//!             .header("authorization", "******")
//!             .body(Body::empty())
//!             .unwrap(),
//!         &(),
//!     )
//!     .await;
//! assert_eq!(ok.status(), StatusCode::OK);
//! assert_eq!(ok.headers()["x-served-by"], "routerama");
//!
//! let anonymous = Api
//!     .route(Request::get("/me").body(Body::empty()).unwrap(), &())
//!     .await;
//! assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
//! // The short-circuit is a generated response, so `stamp` observed it too.
//! assert_eq!(anonymous.headers()["x-served-by"], "routerama");
//!
//! let missing = Api
//!     .route(
//!         Request::get("/absent")
//!             .header("authorization", "******")
//!             .body(Body::empty())
//!             .unwrap(),
//!         &(),
//!     )
//!     .await;
//! assert_eq!(missing.status(), StatusCode::NOT_FOUND);
//! assert_eq!(missing.headers()["x-served-by"], "routerama");
//! # }
//! ```
//!
//! ## Authentication and tracing
//!
//! Those two concerns are what interceptors are reached for first, and both
//! compose without a framework-imposed cost. A router-wide `#[before]`
//! authenticates once, before resolution, and inserts a typed principal that
//! handlers borrow with [`ExtensionRef`]; an unauthenticated caller is a
//! [`Before::Respond`] short-circuit that never reaches extraction.
//!
//! Tracing needs one more rule, because interceptors are ordinary `async`
//! methods: a `#[before]` *returns* before the handler runs, so a
//! [`tracing`](https://docs.rs/tracing) span entered there cannot stay entered
//! for the request, and an entered-span guard must never be held across an
//! `await`. Carry the span itself instead:
//!
//! - the router-wide `#[before]` opens the span (correlation id, method, path)
//!   and inserts it into the request extensions *before* it authenticates, so
//!   short-circuits and routing failures are correlated too;
//! - synchronous emission sites enter it with `Span::in_scope`, and a handler
//!   attaches it to its own future with `Instrument::instrument`, which enters
//!   on each poll and exits on each yield;
//! - a bare `#[after]` reaches the span through the immutable request head it
//!   borrows and records the final status of *every* generated response,
//!   including the authentication short-circuit, extractor rejections, and
//!   routing failures. A mounted service's response stays outside that scope.
//!
//! A transport-assigned correlation id fits naturally above this as a Tower
//! layer that inserts a request extension the `#[before]` then reads. See the
//! runnable `auth_tracing` example; `routerama` itself depends on no telemetry
//! crate and emits no events.
//!
//! ## Mounted services
//!
//! Mounted services receive router-wide `#[before]` interceptors, which run
//! before resolution and delegation, so a mount observes their enrichment and
//! their short-circuits. Per-handler `#[before]`, `#[transform]`, and every
//! `#[after]` apply to generated handlers only: after delegation the
//! mounted service owns the request head and its own response. See the runnable
//! `interceptors` example.
//!
//! # Tower transport adapter
//!
//! The separately enabled `tower` feature adds
//! [`tower`](https://docs.rs/routerama/latest/routerama/route/tower/), whose
//! [`RouteService`](https://docs.rs/routerama/latest/routerama/route/tower/struct.RouteService.html)
//! implements
//! [`tower_service::Service`](https://docs.rs/tower-service/latest/tower_service/trait.Service.html)
//! over any routing call — a generated static entry,
//! a configured dynamic or mixed router, `route_with_erased_mounts`, or a
//! standalone `mount::ErasedMountRouter`. It needs no macro attribute and
//! names no generated type, so no code generation changes and the ordinary
//! `route` signature is untouched.
//!
//! The adapter stores the concrete router, state, and callable and hands the
//! callable owned clones per request, because Tower's associated future type
//! cannot borrow the service. Routing has no backpressure, so readiness is
//! honestly always ready and the service error type is [`Infallible`]; body
//! errors remain body errors. Auto traits flow structurally instead of being
//! imposed, and the explicit response boundary — the default identity, a
//! `Send + 'static` [`SendBoxBody`], or the local
//! [`BoxBody`] — is where a
//! transport body's requirements are paid, at one allocation per application
//! of a boxing boundary. See the runnable `tower_service` example.
//!
//! [`BoxBody`]: crate::response::BoxBody
//! [`Infallible`]: core::convert::Infallible
//! [`SendBoxBody`]: crate::response::SendBoxBody
//!
//! # Zero-copy request metadata
//!
//! Reference parameters borrow directly from the request parts. Custom
//! extractors express the same relationship with the request lifetime on
//! [`FromRequestParts`]. Handler types use normal elision for references and
//! `'_` for named extractor types; generated lifetime names never appear in
//! application code:
//!
//! ```
//! use routerama::route::{
//!     FromRequestParts, HeaderMap, Request, RequestParts, StatusCode, router,
//! };
//!
//! struct UserAgent<'request>(&'request str);
//!
//! impl<'request, S: ?Sized> FromRequestParts<'request, S> for UserAgent<'request> {
//!     type Rejection = StatusCode;
//!
//!     fn from_request_parts(
//!         parts: &'request RequestParts,
//!         _state: &S,
//!     ) -> Result<Self, Self::Rejection> {
//!         parts
//!             .headers
//!             .get("user-agent")
//!             .and_then(|value| value.to_str().ok())
//!             .map(Self)
//!             .ok_or(StatusCode::BAD_REQUEST)
//!     }
//! }
//!
//! struct Metadata;
//!
//! #[router]
//! impl Metadata {
//!     #[route(GET, "/metadata")]
//!     async fn inspect(&self, headers: &HeaderMap, user_agent: UserAgent<'_>) -> StatusCode {
//!         assert_eq!(
//!             user_agent.0.as_ptr(),
//!             headers["user-agent"].as_bytes().as_ptr(),
//!         );
//!         core::future::ready(()).await;
//!         assert_eq!(user_agent.0, "routerama-docs");
//!         StatusCode::NO_CONTENT
//!     }
//! }
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() {
//!     let request = Request::get("/metadata")
//!         .header("user-agent", "routerama-docs")
//!         .body(())
//!         .expect("static request metadata is valid");
//!     let response = Metadata.route(request, &()).await;
//!     assert_eq!(response.status(), StatusCode::NO_CONTENT);
//! }
//! ```
//!
//! The generated route owns `RequestParts` until extraction and the awaited
//! handler complete, so these values cannot escape dispatch. Every anonymous
//! lifetime nested in a parts-extractor type is tied to that borrow. Explicit
//! named lifetimes are rejected because their relationship would be
//! ambiguous. Extractor rejections and their response bodies must be
//! request-independent (`'static`) so an early response cannot retain request
//! metadata.
//!
//! [`ExtensionRef<'_, T>`] performs one type-map lookup and returns `&T`
//! without cloning. [`ClonedExtension<T>`] is the explicitly owned form and
//! invokes `T::clone`. Both reject a missing value with
//! [`MissingExtension<T>`], an error that deliberately becomes
//! `500 Internal Server Error`. Requesting [`Extensions`] directly as
//! `&Extensions` remains the simplest zero-copy view of the complete map. See
//! the runnable `request_metadata` example, which asserts pointer identity
//! between the request head and every borrowed value a handler receives.
//!
//! # Response composition
//!
//! Response bodies, [`IntoResponse`](crate::response::IntoResponse), and
//! [`IntoResponseParts`](crate::response::IntoResponseParts) are owned by the
//! independently enabled [`response`](crate::response) module. Generated
//! routes use that canonical API directly; `route` does not define or re-export
//! a second response surface. See the runnable `response_composition` example
//! for status, header, extension, and fallible-metadata composition, and the
//! runnable `streaming_responses` example for a handler body that forwards
//! data frames, trailers, and a mid-stream error through the generated sum.
//!
//! # Raw body ownership
//!
//! [`RawBody`] is the explicit, unmodified streaming path. It works with any
//! request-body type and performs no buffering:
//!
//! ```
//! use http_body_util::BodyExt as _;
//! use routerama::route::{RawBody, Request, router};
//!
//! struct Echo;
//!
//! #[router]
//! impl Echo {
//!     #[route(POST, "/echo")]
//!     async fn echo(&self, #[body] body: RawBody<String>) -> String {
//!         body.into_inner()
//!     }
//! }
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() {
//!     let request = Request::post("/echo")
//!         .body(String::from("unchanged"))
//!         .expect("static request metadata is valid");
//!     let response = Echo.route(request, &()).await;
//!     let body = response
//!         .into_body()
//!         .collect()
//!         .await
//!         .expect("the generated body succeeds")
//!         .to_bytes();
//!     assert_eq!(body, b"unchanged"[..]);
//! }
//! ```
//!
//! # Explicitly bounded bytes and text
//!
//! [`BytesBody<LIMIT>`](BytesBody) and [`TextBody<LIMIT>`](TextBody) accept
//! request bodies implementing `http_body::Body<Data = bytes::Bytes>`. The
//! const-generic byte limit is part of the handler contract; Routerama exposes
//! no unbounded buffering operation and supplies no hidden default:
//!
//! ```
//! use http_body_util::BodyExt as _;
//! use routerama::response::Body;
//! use routerama::route::{BytesBody, Request, StatusCode, TextBody, router};
//!
//! struct Uploads;
//!
//! #[router]
//! impl Uploads {
//!     #[route(POST, "/bytes")]
//!     async fn bytes(&self, #[body] body: BytesBody<4>) -> bytes::Bytes {
//!         body.into_inner()
//!     }
//!
//!     #[route(POST, "/text")]
//!     async fn text(&self, #[body] body: TextBody<4>) -> String {
//!         body.into_inner()
//!     }
//! }
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() {
//!     let accepted = Request::post("/text")
//!         .body(Body::from("rust"))
//!         .expect("static request metadata is valid");
//!     let response = Uploads.route(accepted, &()).await;
//!     let body = response
//!         .into_body()
//!         .collect()
//!         .await
//!         .expect("the generated body succeeds")
//!         .to_bytes();
//!     assert_eq!(body, b"rust"[..]);
//!
//!     let oversized = Request::post("/bytes")
//!         .body(Body::from("12345"))
//!         .expect("static request metadata is valid");
//!     assert_eq!(
//!         Uploads.route(oversized, &()).await.status(),
//!         StatusCode::PAYLOAD_TOO_LARGE
//!     );
//! }
//! ```
//!
//! Size-limit failures become `413 Payload Too Large`, invalid UTF-8 becomes
//! `400 Bad Request`, and body transport failures become `400 Bad Request`.
//! Their typed diagnostics remain available through [`BodyRejection`],
//! [`BodySizeLimitError`], [`InvalidUtf8Error`], and
//! [`BodyTransportError`].
//!
//! # Optional bounded JSON
//!
//! The additive `json` Cargo feature implies `route` and exposes
//! `routerama::route::json::Json`. Neither `route`, `query`, nor `resolve`
//! enables `json`.
//! Applications must also derive or implement `serde::Deserialize` for their
//! value:
//!
//! ```
//! # #[cfg(feature = "json")]
//! use http_body_util::BodyExt as _;
//! # #[cfg(feature = "json")]
//! use routerama::response::Body;
//! # #[cfg(feature = "json")]
//! use routerama::route::json::Json;
//! # #[cfg(feature = "json")]
//! use routerama::route::{Request, router};
//! # #[cfg(feature = "json")]
//! use serde::Deserialize;
//!
//! # #[cfg(feature = "json")]
//! #[derive(Deserialize)]
//! struct Document {
//!     title: String,
//! }
//!
//! # #[cfg(feature = "json")]
//! struct Documents;
//!
//! # #[cfg(feature = "json")]
//! #[router]
//! impl Documents {
//!     #[route(POST, "/documents")]
//!     async fn create(&self, #[body] document: Json<Document, 1024>) -> String {
//!         document.title.clone()
//!     }
//! }
//!
//! # #[cfg(feature = "json")]
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() {
//!     let request = Request::post("/documents")
//!         .header("content-type", "application/json")
//!         .body(Body::from(r#"{"title":"Routerama"}"#))
//!         .expect("static request metadata is valid");
//!     let response = Documents.route(request, &()).await;
//!     let body = response
//!         .into_body()
//!         .collect()
//!         .await
//!         .expect("the generated body succeeds")
//!         .to_bytes();
//!     assert_eq!(body, b"Routerama"[..]);
//! }
//! # #[cfg(not(feature = "json"))]
//! # fn main() {}
//! ```
//!
//! JSON accepts `application/json` and `application/*+json`. A missing or
//! unsupported content type becomes `415 Unsupported Media Type`; malformed
//! JSON becomes `400 Bad Request`; the same explicit byte limit and transport
//! mappings apply before decoding. See the runnable `json_api` example for all
//! four outcomes behind one `#[catch]`.
//!
//! # Optional bounded forms
//!
//! The additive `form` Cargo feature implies both `route` and `query` and
//! exposes `routerama::route::form::Form`. It decodes a bounded
//! `application/x-www-form-urlencoded` body through the same
//! `routerama::query::FromQuery` implementation used for URI query strings.
//! Form output must be owned with respect to the temporary buffered text;
//! schemas containing fields that borrow from the encoded input are not
//! accepted. Exactly one valid content type is required, with
//! case-insensitive type/subtype matching and legal parameters. Media-type
//! failures become `415`, size failures become `413`, and transport, UTF-8,
//! and query-codec failures become `400`.
//!
//! There is no form-specific Serde dependency or second URL decoder. The
//! plus/percent rules, scalar/optional/repeated schema behavior, detailed
//! errors, and resource limits of the query codec apply unchanged. See the
//! crate's runnable `form` example for complete dispatch.

pub mod extract;
mod failure;
#[cfg(feature = "form")]
pub mod form;
mod interceptor;
#[cfg(feature = "json")]
pub mod json;
#[cfg(feature = "mount")]
pub mod mount;
mod predicate;
#[cfg(feature = "tower")]
pub mod tower;

pub use extract::{
    BodyRejection, BodySizeLimitError, BodyStateWitness, BodyTransportError, BytesBody, ClonedExtension, ExtensionRef, FromRef,
    FromRequestBody, FromRequestParts, InvalidUtf8Error, MissingExtension, RawBody, State, TextBody,
};
#[cfg(feature = "query")]
pub use extract::{Query, QueryRejection};
pub use failure::RouteFailure;
pub use http::request::Parts as RequestParts;
pub use http::{Extensions, HeaderMap, Method, Request, StatusCode, Uri, Version};
pub use interceptor::{AfterContext, Before, BeforeContext, BodyConsumed, BodyTransform, SelectedContext};
/// Generates static and dynamic routing for an annotated inherent impl.
///
/// # Example
///
/// One service can mix compile-time static routes with handlers whose method
/// and path are registered at startup. A static-only service is dispatched
/// directly through `service.route(request, state)`; adding a
/// `#[route(dynamic)]` handler additionally generates `router_builder()`, one
/// `add_<handler>` method per dynamic handler, and a persistent router whose
/// `route(&service, request, state)` performs the same exhaustive direct
/// routing:
///
/// ```
/// use http_body_util::BodyExt as _;
/// use routerama::response::Body;
/// use routerama::route::{Method, Request, State, StatusCode, TextBody, router};
///
/// #[derive(Clone)]
/// struct AppState {
///     label: &'static str,
/// }
///
/// struct Plugins;
///
/// #[router(state = AppState)]
/// impl Plugins {
///     #[route(POST, "/plugins/{id}", consumes = "text/plain")]
///     async fn rename(
///         &self,
///         id: u32,
///         #[body] name: TextBody<64>,
///         state: State<AppState>,
///     ) -> (StatusCode, String) {
///         (
///             StatusCode::ACCEPTED,
///             format!("{}:{id}:{}", state.label, name.as_str()),
///         )
///     }
///
///     #[route(dynamic)]
///     async fn invoke(&self, method: Method, #[capture] name: String) -> String {
///         format!("{method}:{name}")
///     }
/// }
///
/// #[tokio::main(flavor = "current_thread")]
/// async fn main() -> Result<(), Box<dyn core::error::Error>> {
///     // Dynamic registration is fallible and happens once, at startup.
///     let router = Plugins::router_builder()
///         .add_invoke("GET", "/run/{name}")
///         .build()?;
///     let state = AppState { label: "eu" };
///
///     let request = Request::post("/plugins/7")
///         .header("content-type", "text/plain; charset=utf-8")
///         .body(Body::from("audit"))?;
///     let response = router.route(&Plugins, request, &state).await;
///     assert_eq!(response.status(), StatusCode::ACCEPTED);
///     assert_eq!(collect(response.into_body()).await, b"eu:7:audit"[..]);
///
///     let request = Request::get("/run/tracing").body(Body::empty())?;
///     let response = router.route(&Plugins, request, &state).await;
///     assert_eq!(collect(response.into_body()).await, b"GET:tracing"[..]);
///
///     // A miss is a `404`; a capture that will not convert is a `400`.
///     let request = Request::get("/run").body(Body::empty())?;
///     assert_eq!(
///         router.route(&Plugins, request, &state).await.status(),
///         StatusCode::NOT_FOUND
///     );
///     Ok(())
/// }
///
/// async fn collect<B>(body: B) -> bytes::Bytes
/// where
///     B: http_body::Body<Data = bytes::Bytes>,
///     B::Error: core::fmt::Debug,
/// {
///     body.collect()
///         .await
///         .expect("the generated body succeeds")
///         .to_bytes()
/// }
/// ```
///
/// Runnable examples for every other capability live in the crate's
/// `examples/` directory: `request_metadata`, `request_predicates`,
/// `route_policy`, `required_state`, `streaming_responses`,
/// `response_composition`, `json_api`, `form`, `interceptors`,
/// `mounted_services`, `tower_service`, and `auth_tracing`.
///
/// # Reference
pub use routerama_macros::router;

/// Runtime support referenced by generated routers.
#[doc(hidden)]
pub mod __private {
    pub use bytes;
    pub use http;
    pub use http_body;
    pub use pin_project_lite::pin_project;
    pub use routerama_build::Route;

    #[cfg(feature = "form")]
    pub use super::form::FormRejection;
    pub use super::interceptor::{AfterContext, Before, BeforeContext, BodyConsumed, BodyTransform, SelectedContext};
    #[cfg(feature = "json")]
    pub use super::json::JsonRejection;
    #[cfg(feature = "mount")]
    pub use super::mount::ErasedMountRouter;
    pub use super::predicate::{accepts, content_type_matches, host_matches, set_produced_content_type};
    pub use super::{BodyRejection, BodyStateWitness, FromRequestBody, FromRequestParts, RouteFailure};
    pub use crate::captures::Captures;
    pub use crate::codegen_helpers::{
        InvalidPath, RouteMatch, ScannedPath, scan_path, scan_segments, seg_bytes, split_verb, substr, with_scanned_path,
    };
    pub use crate::configuration_error::ConfigurationError;
    pub use crate::dyn_builder::DynBuilder;
    pub use crate::dyn_route::DynRoute;
    pub use crate::extract_helpers::{coerce_cow, coerce_owned, coerce_parse, owned, parse};
    pub use crate::http_method::HttpMethod;
    pub use crate::raw_match::RawMatch;
    pub use crate::raw_resolver::RawResolver;
    pub use crate::resolve_error::ResolveError;
    pub use crate::resolver::Resolver;
    use crate::response::{IntoResponse, Response};

    /// Maps internal matching errors to the prototype HTTP rejection policy.
    #[must_use]
    pub fn resolve_error_response(error: ResolveError<'_>) -> Response {
        match error {
            ResolveError::NotFound(_) => http::StatusCode::NOT_FOUND.into_response(),
            ResolveError::InvalidPath(_)
            | ResolveError::MissingCapture(_)
            | ResolveError::InvalidCapture(_)
            | ResolveError::UndecodableCapture(_) => http::StatusCode::BAD_REQUEST.into_response(),
        }
    }

    /// Retains a resolver diagnostic for a generated typed fallback.
    #[must_use]
    pub const fn route_failure(error: ResolveError<'_>) -> RouteFailure<'_> {
        super::failure::from_resolve_error(error)
    }

    /// Buffers a request body for a generated
    /// `#[transform(limit = N, ...)]` interceptor.
    ///
    /// The explicit const limit mirrors the buffered body extractors, so a
    /// transform never silently buffers an unbounded request. A
    /// `#[transform(stream, ...)]` interceptor never reaches this helper: it
    /// receives the transport body by value and wraps it instead.
    ///
    /// # Errors
    ///
    /// Returns the same [`BodyRejection`] a bounded body extractor would.
    pub async fn buffer_request_body<B, const LIMIT: usize>(body: B) -> Result<bytes::Bytes, BodyRejection<B::Error>>
    where
        B: http_body::Body<Data = bytes::Bytes>,
    {
        super::extract::collect_body::<B, LIMIT>(body).await
    }
}
