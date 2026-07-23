// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generated HTTP handler routing and request extraction.
//!
//! [`router`] turns annotated inherent methods into direct static dispatch.
//! Static handlers use `#[route(METHOD, "path")]`; `#[route(dynamic)]`
//! handlers receive method/path registrations through a generated startup
//! builder. Generated code preserves concrete handler futures and response
//! bodies.
//!
//! Handler parameters are classified as follows:
//!
//! - static path captures are matched by parameter name;
//! - dynamic captures use `#[capture]` and must be owned;
//! - one `#[body]` parameter implements [`FromRequestBody`];
//! - all other parameters implement [`FromRequestParts`].
//!
//! Metadata references and borrowed custom extractors borrow from the request
//! head through the handler's `.await`. [`RawBody`] transfers the transport
//! body unchanged. [`BytesBody`], [`TextBody`], and [`Utf8Body`] buffer up to
//! an explicit const-generic limit. The `json` and `form` features add bounded
//! typed extractors.
//!
//! Bare `#[router]` keeps shared state generic. `#[router(state = T)]` fixes
//! the state contract and validates state-dependent extraction where the
//! service is defined. [`State`] uses [`FromRef`] for projections.
//!
//! Routes may declare `host`, `consumes`, and `produces` predicates. A
//! `headers(insert("name", "value"), append("name", "value"))` plan emits
//! compile-time-validated static response-header operations directly after
//! handler conversion. Negotiated `Content-Type` is applied next, then
//! `#[after]` interceptors observe and may modify the complete response head.
//! Exact method/template overlaps require compatible captures and distinct
//! explicit priorities. One `#[fallback]` handles [`RouteFailure`]; `#[catch]`
//! methods customize extractor rejections.
//!
//! Generated interceptors are direct async method calls:
//!
//! - router-wide and per-handler `#[before]` methods inspect or mutate request
//!   metadata and may short-circuit;
//! - `#[transform(limit = N, ...)]` buffers a bounded body, while
//!   `#[transform(stream, ...)]` wraps the transport body;
//! - per-handler and router-wide `#[after]` methods mutate response metadata.
//!
//! The `mount` feature provides explicitly erased runtime services. Generated
//! routes retain precedence and concrete bodies; only a complete generated
//! miss reaches the mount router. The `tower` feature provides a
//! `tower_service::Service` adapter.
//!
//! Response conversion uses [`crate::response`]. Generated sums forward data,
//! trailers, errors, size hints, and auto traits without boxing. Applications
//! may opt into [`crate::response::BoxBody`] or
//! [`crate::response::SendBoxBody`] at an explicit erasure boundary.
//!
//! Paths are matched on raw bytes, with no percent-decoding, dot-segment
//! normalization, or trailing-slash equivalence, and captures are decoded only
//! after a route is chosen. Those rules and their security consequences are
//! documented under the matching semantics on [`crate::resolve`], and they
//! apply here unchanged.

#[cfg(feature = "bytesbuf")]
pub mod bytesbuf;
pub mod extract;
mod failure;
#[cfg(feature = "form")]
pub mod form;
pub mod header;
mod interceptor;
#[cfg(feature = "json")]
pub mod json;
#[cfg(feature = "mount")]
pub mod mount;
mod predicate;
#[cfg(feature = "tower")]
pub mod tower;

pub use extract::{
    BodyFrameLimitError, BodyRejection, BodySizeLimitError, BodyStateWitness, BodyTransportError, BytesBody, ClonedExtension, ExtensionRef,
    FromRef, FromRequestBody, FromRequestParts, InvalidUtf8Error, MissingExtension, RawBody, State, TextBody, Utf8Body,
};
#[cfg(feature = "query")]
pub use extract::{Query, QueryRejection};
pub use failure::RouteFailure;
pub use http::request::Parts as RequestParts;
pub use http::{Extensions, HeaderMap, Method, Request, StatusCode, Uri, Version};
pub use interceptor::{AfterContext, Before, BeforeContext, BodyConsumed, BodyTransform, SelectedContext};
/// Generates static and runtime-configured routing for an inherent impl.
///
/// Static services dispatch through `service.route(request, state)`. A service
/// containing `#[route(dynamic)]` also receives `router_builder()`, generated
/// registration methods, and a router whose
/// `route(&service, request, state)` method handles both route sets.
///
/// See the attribute's reference documentation and the crate examples for
/// predicates, extraction, state, interceptors, mounts, and response handling.
pub use routerama_macros::router;

/// Runtime support referenced by generated routers.
#[doc(hidden)]
pub mod __private {
    pub use bytes;
    pub use http;
    pub use http_body;
    pub use pin_project_lite::pin_project;
    pub use routerama_build::Route;
    #[cfg(feature = "tower")]
    pub use tower_service;

    #[cfg(feature = "form")]
    pub use super::form::FormRejection;
    pub use super::interceptor::{AfterContext, Before, BeforeContext, BodyConsumed, BodyTransform, SelectedContext};
    #[cfg(feature = "json")]
    pub use super::json::JsonRejection;
    #[cfg(feature = "mount")]
    pub use super::mount::{ErasedMountRouter, MountDelegate, SendErasedMountRouter};
    pub use super::predicate::{
        MediaType, OverlapPredicateState, accepts, accepts_parsed, content_type_matches, content_type_matches_parsed, host_matches,
    };
    #[cfg(feature = "tower")]
    pub use super::tower::RouteService;
    pub use super::{BodyRejection, BodyStateWitness, FromRequestBody, FromRequestParts, RouteFailure};
    pub use crate::captures::Captures;
    pub use crate::codegen_helpers::{
        InvalidPath, RouteMatch, ScannedPath, ScannedPathPrefix, scan_path, scan_segments, seg_bytes, split_verb, substr, with_scanned_path,
    };
    pub use crate::configuration_error::ConfigurationError;
    pub use crate::dyn_builder::DynBuilder;
    pub use crate::dyn_route::DynRoute;
    pub use crate::extract_helpers::{
        PrimitiveCapture, PrimitivePath, coerce_cow, coerce_owned, coerce_parse, coerce_primitive, owned, parse, primitive,
    };
    pub use crate::http_method::HttpMethod;
    pub use crate::raw_match::RawMatch;
    pub use crate::raw_resolver::RawResolver;
    pub use crate::resolve_error::ResolveError;
    pub use crate::resolver::Resolver;
    use crate::response::{IntoResponse, Response};

    /// Internal generated routing contract for exact Tower services.
    #[cfg(feature = "tower")]
    pub trait GeneratedExactRoute<B, State: ?Sized, ServiceHandle, StateHandle> {
        /// Routes one request while preserving the generated concrete body.
        fn route_exact(
            service: ServiceHandle,
            request: http::Request<B>,
            state: StateHandle,
        ) -> impl Future<
            Output = Response<
                impl http_body::Body<Data = bytes::Bytes, Error = impl core::error::Error + Send + Sync + 'static> + Send + 'static,
            >,
        > + Send
        + 'static;
    }

    /// Internal exact Tower contract for routers with heterogeneous frame data.
    #[cfg(feature = "tower")]
    pub trait GeneratedExactRouteData<B, State: ?Sized, ServiceHandle, StateHandle> {
        /// Routes one request while preserving the generated concrete body and data sum.
        fn route_exact(
            service: ServiceHandle,
            request: http::Request<B>,
            state: StateHandle,
        ) -> impl Future<
            Output = Response<impl http_body::Body<Error = impl core::error::Error + Send + Sync + 'static> + Send + 'static>,
        > + Send
        + 'static;
    }

    /// Internal generated routing contract for configured exact Tower services.
    #[cfg(feature = "tower")]
    pub trait GeneratedExactConfiguredRoute<B, Service: ?Sized, State: ?Sized, RouterHandle, ServiceHandle, StateHandle> {
        /// Routes one configured request while preserving the generated body.
        fn route_exact(
            router: RouterHandle,
            service: ServiceHandle,
            request: http::Request<B>,
            state: StateHandle,
        ) -> impl Future<
            Output = Response<
                impl http_body::Body<Data = bytes::Bytes, Error = impl core::error::Error + Send + Sync + 'static> + Send + 'static,
            >,
        > + Send
        + 'static;
    }

    /// Internal configured Tower contract for routers with heterogeneous frame data.
    #[cfg(feature = "tower")]
    pub trait GeneratedExactConfiguredRouteData<B, Service: ?Sized, State: ?Sized, RouterHandle, ServiceHandle, StateHandle> {
        /// Routes one configured request while preserving the generated body and data sum.
        fn route_exact(
            router: RouterHandle,
            service: ServiceHandle,
            request: http::Request<B>,
            state: StateHandle,
        ) -> impl Future<
            Output = Response<impl http_body::Body<Error = impl core::error::Error + Send + Sync + 'static> + Send + 'static>,
        > + Send
        + 'static;
    }

    /// Maps internal matching errors to HTTP status responses.
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

    /// Collects a request body up to `LIMIT` bytes for a buffered transform.
    ///
    /// # Errors
    ///
    /// Returns the same [`BodyRejection`] as a bounded body extractor.
    pub async fn buffer_request_body<B, const LIMIT: usize>(body: B) -> Result<bytes::Bytes, BodyRejection<B::Error>>
    where
        B: http_body::Body<Data = bytes::Bytes>,
    {
        super::extract::collect_body::<B, LIMIT>(body).await
    }
}
