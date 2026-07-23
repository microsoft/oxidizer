// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Explicitly erased services mounted by method and path template.
//!
//! [`ErasedMountRouter`] is the opt-in boundary for handlers whose concrete
//! service, future, or response-body types are not known to a generated
//! router. Every matched call performs one service-vtable call, allocates and
//! dynamically polls one service future, and normalizes the returned concrete
//! body through one [`BoxBody`] allocation. Body errors allocate only when
//! they occur. Up to four capture ranges and paths up to the 16-segment inline
//! matcher boundary add no matching allocation; larger sets may spill their
//! existing `SmallVec` scratch storage. A complete miss allocates only its
//! boxed fixed response body. No [`Send`] or [`Sync`] bound is imposed.
//!
//! The immutable router is built at startup. Invalid method tokens, invalid
//! templates, and conflicting mounted shapes are accumulated in
//! [`ConfigurationError`]. Registering the same [`ErasedMountService`] at
//! multiple method/template pairs creates aliases without another service
//! allocation.
//!
//! `#[router(state = S, erased_mounts)]` explicitly generates the additional
//! `route_with_erased_mounts` entry. Enabling the feature alone changes no
//! generated service. The named entry tries every generated static and
//! configured-dynamic handler first and consults mounts only after a complete
//! generated miss. The mount table is the final backstop: its own miss returns
//! a plain `404 Not Found` rather than invoking a generated custom
//! `#[fallback]`. Its response body is structurally
//! `EitherBody<Generated, BoxBody>`: the generated branch is never boxed and
//! does not invoke a mounted-service trait object.
//!
//! # Generated dynamic handlers versus erased mounts
//!
//! `#[route(dynamic)]` and this module solve different problems:
//!
//! - a generated dynamic handler has a concrete method, future, extractors,
//!   state contract, and response body known to the macro. Only its
//!   method/template aliases are configured at startup. Calls remain direct,
//!   futures remain unboxed, and the body enters the generated concrete sum;
//! - an erased mount accepts a runtime-chosen closure or named
//!   [`MountedService`]. Registration stores a trait object, and every matched
//!   request crosses the boxed-future and [`BoxBody`] boundary described
//!   above.
//!
//! Prefer generated dynamic handlers whenever the handler implementation is
//! statically known. Use mounts for plugin registries, application-selected
//! service sets, and other genuinely open handler/body sets.
//!
//! # Precedence and failures
//!
//! An [`ErasedMountRouter`] rejects conflicts within its own registrations.
//! The generated integration intentionally does not reject overlap with a
//! generated static or configured-dynamic route: generated routes always win.
//! A generated capture conversion failure, predicate rejection, extractor
//! rejection, or handler response is therefore final and never falls through.
//! Only a generated `ResolveError::NotFound` enters the mount table. If that
//! table also misses, it returns `404`; its invalid-path policy is `400`.
//!
//! Mounted templates do not declare Rust capture types. [`MountedRequest`]
//! exposes raw and decoded zero-copy views plus explicit one-shot
//! [`capture`](MountedRequest::capture) conversion. A conversion error becomes
//! `400` when returned from a service.
//!
//! # Ownership and lifetimes
//!
//! The mount router is parameterized by one request body `B` and state `S`.
//! Matching stores capture offsets, then moves the original `Request<B>` into
//! the service; it does not box or copy the request body. Services are owned
//! for the router lifetime and therefore `'static`. The generated adapter must
//! first split requests so static captures can borrow their URI; on a miss it
//! reassembles the same parts and body by move, without cloning, copying, or
//! request-body boxing. Service call futures may borrow the service and state
//! and may be local (`!Send`). Response bodies and errors must be `'static`
//! because [`BoxBody`] owns them behind trait objects. The `tower` feature
//! leaves this local core contract unchanged: its adapter inherits service
//! auto traits, reports generated routing as always ready, and lets a mounted
//! router compose through its identity or local boxing boundary.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::rc::Rc;
use core::fmt;
use core::pin::Pin;
use core::str::FromStr;

use smallvec::SmallVec;

use crate::captures::materialize_range;
use crate::codegen_helpers::ScannedPath;
pub use crate::configuration_error::ConfigurationError;
use crate::decode::decode;
use crate::dyn_builder::DynBuilder;
use crate::dyn_route::DynRoute;
use crate::raw_match::INLINE_CAPTURES;
use crate::raw_resolver::RawResolver;
use crate::response::{Body, BoxBody, IntoResponse, Response};

/// A concrete service contract that can be explicitly erased for mounting.
///
/// Implement this trait for named services. The returned future and response
/// remain concrete here; [`ErasedMountService::new`] is the point that boxes
/// the future and response body. The future may borrow the service, mounted
/// request, and state, and need not be `Send`.
pub trait MountedService<B, S: ?Sized> {
    /// The concrete response produced by this service.
    type Response: IntoResponse;

    /// Handles one matched mounted request.
    fn call<'a>(&'a self, request: MountedRequest<'a, B>, state: &'a S) -> impl Future<Output = Self::Response> + 'a
    where
        B: 'a;
}

/// An explicitly type-erased mounted service.
///
/// Construction allocates the stored service once. Cloning this handle, for
/// example to register aliases, only increments an `Rc` count and does not
/// allocate. Per-request future and body allocations happen only when a
/// mounted route actually matches.
pub struct ErasedMountService<B, S: ?Sized> {
    inner: Rc<dyn ErasedCall<B, S>>,
}

impl<B, S: ?Sized> ErasedMountService<B, S> {
    /// Erases a named [`MountedService`].
    ///
    /// The concrete response body and error must be `'static` because the
    /// mounted response stores them behind [`BoxBody`]. The service itself is
    /// owned by the returned handle and must therefore also be `'static`.
    #[must_use]
    pub fn new<T>(service: T) -> Self
    where
        T: MountedService<B, S> + 'static,
        <T::Response as IntoResponse>::Body: 'static,
        <<T::Response as IntoResponse>::Body as http_body::Body>::Error: core::error::Error + 'static,
    {
        Self {
            inner: Rc::new(ServiceAdapter(service)),
        }
    }

    /// Erases an async closure.
    ///
    /// Async closures receive the owned [`MountedRequest`] and shared state.
    /// This convenience requires a `'static` request-body type so the closure
    /// can implement the higher-ranked call contract. It still imposes no
    /// `Send` or `Sync` bound on the closure, future, state, or response body.
    #[must_use]
    pub fn from_async_fn<F, R>(service: F) -> Self
    where
        B: 'static,
        F: for<'a> AsyncFn(MountedRequest<'a, B>, &'a S) -> R + 'static,
        R: IntoResponse,
        R::Body: 'static,
        <R::Body as http_body::Body>::Error: core::error::Error + 'static,
    {
        Self {
            inner: Rc::new(AsyncFnAdapter(service)),
        }
    }

    fn call<'a>(&'a self, request: MountedRequest<'a, B>, state: &'a S) -> ErasedFuture<'a>
    where
        B: 'a,
    {
        self.inner.call(request, state)
    }
}

impl<B, S: ?Sized> Clone for ErasedMountService<B, S> {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<B, S: ?Sized> fmt::Debug for ErasedMountService<B, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErasedMountService").finish_non_exhaustive()
    }
}

type ErasedFuture<'a> = Pin<Box<dyn core::future::Future<Output = Response<BoxBody>> + 'a>>;

trait ErasedCall<B, S: ?Sized> {
    fn call<'a>(&'a self, request: MountedRequest<'a, B>, state: &'a S) -> ErasedFuture<'a>
    where
        B: 'a;
}

struct ServiceAdapter<T>(T);

impl<B, S: ?Sized, T> ErasedCall<B, S> for ServiceAdapter<T>
where
    T: MountedService<B, S>,
    <T::Response as IntoResponse>::Body: 'static,
    <<T::Response as IntoResponse>::Body as http_body::Body>::Error: core::error::Error + 'static,
{
    fn call<'a>(&'a self, request: MountedRequest<'a, B>, state: &'a S) -> ErasedFuture<'a>
    where
        B: 'a,
    {
        Box::pin(async move { self.0.call(request, state).await.into_response().map(BoxBody::new) })
    }
}

struct AsyncFnAdapter<F>(F);

impl<B, S: ?Sized, F, R> ErasedCall<B, S> for AsyncFnAdapter<F>
where
    B: 'static,
    F: for<'a> AsyncFn(MountedRequest<'a, B>, &'a S) -> R,
    R: IntoResponse,
    R::Body: 'static,
    <R::Body as http_body::Body>::Error: core::error::Error + 'static,
{
    fn call<'a>(&'a self, request: MountedRequest<'a, B>, state: &'a S) -> ErasedFuture<'a>
    where
        B: 'a,
    {
        Box::pin(async move { (self.0)(request, state).await.into_response().map(BoxBody::new) })
    }
}

/// Builds an immutable [`ErasedMountRouter`].
///
/// Registrations are validated together by [`build`](Self::build), so method,
/// template, and deterministic conflict errors are startup failures rather
/// than request-time policy.
pub struct ErasedMountRouterBuilder<B, S: ?Sized> {
    inner: DynBuilder<ErasedMountService<B, S>>,
}

impl<B, S: ?Sized> ErasedMountRouterBuilder<B, S> {
    /// Creates an empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self { inner: DynBuilder::new() }
    }

    /// Registers one erased service for an HTTP method and path template.
    ///
    /// Call this more than once with clones of one
    /// [`ErasedMountService`] to create aliases. Errors are accumulated and
    /// returned by [`build`](Self::build).
    #[must_use]
    pub fn mount(mut self, method: impl AsRef<str>, path: impl AsRef<str>, service: ErasedMountService<B, S>) -> Self {
        self.inner.add_untyped(method, path.as_ref(), service);
        self
    }

    /// Validates all registrations and builds an immutable mount router.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError`] containing all invalid methods,
    /// templates, and conflicting method/template shapes.
    pub fn build(self) -> Result<ErasedMountRouter<B, S>, ConfigurationError> {
        self.inner.finish_mounts().map(|resolver| ErasedMountRouter { resolver })
    }
}

impl<B, S: ?Sized> Default for ErasedMountRouterBuilder<B, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B, S: ?Sized> fmt::Debug for ErasedMountRouterBuilder<B, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErasedMountRouterBuilder").field("inner", &self.inner).finish()
    }
}

/// An immutable method/path router for explicitly erased mounted services.
pub struct ErasedMountRouter<B, S: ?Sized> {
    resolver: RawResolver<DynRoute<ErasedMountService<B, S>>>,
}

impl<B, S: ?Sized> ErasedMountRouter<B, S> {
    /// Creates a startup builder.
    #[must_use]
    pub fn builder() -> ErasedMountRouterBuilder<B, S> {
        ErasedMountRouterBuilder::new()
    }

    /// Routes one request entirely through the mounted-service table.
    ///
    /// A complete miss becomes `404 Not Found`; an invalid path becomes
    /// `400 Bad Request`. A match invokes the erased service. Request parts and
    /// body move directly into [`MountedRequest`] without boxing, cloning, or
    /// reconstruction.
    #[expect(
        clippy::future_not_send,
        reason = "the core mounted-service boundary intentionally supports local futures, bodies, and state"
    )]
    pub async fn route(&self, request: http::Request<B>, state: &S) -> Response<BoxBody> {
        let matched = self
            .resolver
            .resolve_scanned_checked(request.method().as_str(), request.uri().path(), |leaf, route, scanned| {
                (route.extractor(), MountedCaptureRanges::new(leaf, scanned))
            });
        match matched {
            Ok(Some((service, captures))) => service.call(MountedRequest { request, captures }, state).await,
            Ok(None) => boxed_status(http::StatusCode::NOT_FOUND),
            Err(_) => boxed_status(http::StatusCode::BAD_REQUEST),
        }
    }
}

impl<B, S: ?Sized> fmt::Debug for ErasedMountRouter<B, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErasedMountRouter").field("resolver", &self.resolver).finish()
    }
}

/// An owned request passed to a matched mounted service.
///
/// Capture ranges are computed during route matching and retained as offsets
/// into the request URI. Raw capture access and iteration therefore neither
/// parse the path again nor allocate. Call [`into_request`](Self::into_request)
/// after extracting any needed captures to transfer the original request
/// parts and body directly.
pub struct MountedRequest<'mount, B> {
    request: http::Request<B>,
    captures: MountedCaptureRanges<'mount>,
}

impl<B> MountedRequest<'_, B> {
    /// Borrows the original request.
    #[must_use]
    pub const fn request(&self) -> &http::Request<B> {
        &self.request
    }

    /// Consumes this adapter and returns the original request.
    #[must_use]
    pub fn into_request(self) -> http::Request<B> {
        self.request
    }

    /// Returns one raw, still-percent-encoded capture without allocation.
    #[must_use]
    pub fn raw_capture(&self, name: &str) -> Option<&str> {
        let index = self.captures.leaf.vars.iter().position(|plan| plan.key() == name)?;
        self.request.uri().path().get(self.captures.ranges.get(index)?.clone())
    }

    /// Percent-decodes one capture, borrowing when no decoding is needed.
    ///
    /// # Errors
    ///
    /// Returns [`MountedCaptureError::Missing`] if `name` is not a capture and
    /// [`MountedCaptureError::Undecodable`] for malformed encoding or UTF-8.
    pub fn decoded_capture(&self, name: &str) -> Result<Cow<'_, str>, MountedCaptureError> {
        let raw = self.raw_capture(name).ok_or(MountedCaptureError::Missing)?;
        decode(raw).ok_or(MountedCaptureError::Undecodable)
    }

    /// Percent-decodes and parses one capture once.
    ///
    /// Numeric and other non-allocating `FromStr` implementations remain
    /// allocation-free when the capture contains no percent escapes.
    ///
    /// # Errors
    ///
    /// Returns [`MountedCaptureError`] when the capture is absent, cannot be
    /// decoded, or cannot be parsed as `T`.
    pub fn capture<T: FromStr>(&self, name: &str) -> Result<T, MountedCaptureError> {
        self.decoded_capture(name)?.parse().map_err(|_error| MountedCaptureError::Invalid)
    }

    /// Iterates raw `(name, value)` captures in template declaration order.
    ///
    /// Iteration parses neither the template nor request path again and
    /// allocates nothing.
    pub fn captures(&self) -> impl Iterator<Item = (&str, &str)> {
        self.captures
            .leaf
            .vars
            .iter()
            .zip(&self.captures.ranges)
            .filter_map(|(plan, range)| self.request.uri().path().get(range.clone()).map(|value| (plan.key(), value)))
    }
}

impl<B: fmt::Debug> fmt::Debug for MountedRequest<'_, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MountedRequest")
            .field("request", &self.request)
            .field("capture_count", &self.captures.ranges.len())
            .finish()
    }
}

struct MountedCaptureRanges<'mount> {
    leaf: &'mount routerama_build::trie::Leaf,
    ranges: SmallVec<[core::ops::Range<usize>; INLINE_CAPTURES]>,
}

impl<'mount> MountedCaptureRanges<'mount> {
    fn new(leaf: &'mount routerama_build::trie::Leaf, scanned: &ScannedPath<'_, '_>) -> Self {
        let mut ranges = if leaf.vars.len() <= INLINE_CAPTURES {
            SmallVec::new()
        } else {
            SmallVec::with_capacity(leaf.vars.len())
        };
        ranges.extend(leaf.vars.iter().map(|plan| materialize_range(plan, scanned)));
        Self { leaf, ranges }
    }
}

/// A mounted capture lookup, decoding, or conversion failure.
///
/// This compact value intentionally does not allocate a copy of a
/// runtime-configured capture name. Converting it through [`IntoResponse`]
/// always produces `400 Bad Request`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MountedCaptureError {
    /// The requested name is not captured by the matched template.
    Missing,
    /// Percent encoding was malformed or decoded to invalid UTF-8.
    Undecodable,
    /// The decoded value did not parse as the requested type.
    Invalid,
}

impl fmt::Display for MountedCaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Missing => "mounted route capture is missing",
            Self::Undecodable => "mounted route capture could not be percent-decoded",
            Self::Invalid => "mounted route capture could not be parsed",
        })
    }
}

impl core::error::Error for MountedCaptureError {}

impl IntoResponse for MountedCaptureError {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        let _ = self;
        http::StatusCode::BAD_REQUEST.into_response()
    }
}

fn boxed_status(status: http::StatusCode) -> Response<BoxBody> {
    status.into_response().map(BoxBody::new)
}
