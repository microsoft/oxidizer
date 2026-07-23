// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime-selected services mounted by method and path template.
//!
//! [`ErasedMountRouter`] stores explicitly erased services built at startup.
//! A matched call crosses one service-vtable boundary, boxes the service
//! future, and converts the response body to [`BoxBody<D>`](BoxBody). The
//! request body and response frame data are moved unchanged, and no [`Send`]
//! or [`Sync`] bound is imposed.
//!
//! Construction reports invalid or conflicting registrations through
//! [`ConfigurationError`]. Generated routes take precedence; only a complete
//! generated miss reaches mounts. [`MountedRequest`] provides raw, decoded,
//! and typed capture access.
//!
//! [`SendErasedMountRouter`] mirrors that surface for multi-threaded
//! transports: its services, futures, and [`SendBoxBody<D>`](SendBoxBody)
//! responses are all [`Send`], so a mounted route stays reachable after the
//! routing future is moved across threads. The two routers are interchangeable
//! at the generated entry because both implement [`MountDelegate`], which the
//! generated mounted method is generic over; auto traits then follow whichever
//! delegate a caller passes, at no runtime cost.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::sync::Arc;
use core::fmt;
use core::pin::Pin;
use core::str::FromStr;
use core::task::{Context, Poll};

use bytes::Bytes;
use smallvec::SmallVec;

use crate::captures::materialize_range;
use crate::codegen_helpers::ScannedPath;
pub use crate::configuration_error::ConfigurationError;
use crate::decode::decode;
use crate::dyn_builder::DynBuilder;
use crate::dyn_route::DynRoute;
use crate::raw_match::INLINE_CAPTURES;
use crate::raw_resolver::RawResolver;
use crate::response::{Body, BoxBody, IntoResponse, Response, SendBoxBody};

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
pub struct ErasedMountService<B, S: ?Sized, D: bytes::Buf = Bytes> {
    inner: Rc<dyn ErasedCall<B, S, D>>,
}

impl<B, S: ?Sized, D> ErasedMountService<B, S, D>
where
    D: bytes::Buf + 'static,
{
    /// Erases a named [`MountedService`].
    ///
    /// The concrete response body and error must be `'static` because the
    /// mounted response stores them behind [`BoxBody`]. The service itself is
    /// owned by the returned handle and must therefore also be `'static`.
    #[must_use]
    pub fn new<T>(service: T) -> Self
    where
        T: MountedService<B, S> + 'static,
        <T::Response as IntoResponse>::Body: http_body::Body<Data = D> + 'static,
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
        R::Body: http_body::Body<Data = D> + 'static,
        <R::Body as http_body::Body>::Error: core::error::Error + 'static,
    {
        Self {
            inner: Rc::new(AsyncFnAdapter(service)),
        }
    }

    fn call<'a>(&'a self, request: MountedRequest<'a, B>, state: &'a S) -> ErasedFuture<'a, D>
    where
        B: 'a,
    {
        self.inner.call(request, state)
    }
}

impl<B, S: ?Sized, D: bytes::Buf> Clone for ErasedMountService<B, S, D> {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<B, S: ?Sized, D: bytes::Buf> fmt::Debug for ErasedMountService<B, S, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErasedMountService").finish_non_exhaustive()
    }
}

struct ErasedFuture<'a, D: bytes::Buf = Bytes> {
    inner: Pin<Box<dyn core::future::Future<Output = Response<BoxBody<D>>> + 'a>>,
}

impl<D: bytes::Buf> Future for ErasedFuture<'_, D> {
    type Output = Response<BoxBody<D>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

trait ErasedCall<B, S: ?Sized, D: bytes::Buf> {
    fn call<'a>(&'a self, request: MountedRequest<'a, B>, state: &'a S) -> ErasedFuture<'a, D>
    where
        B: 'a;
}

struct ServiceAdapter<T>(T);

impl<B, S: ?Sized, D, T> ErasedCall<B, S, D> for ServiceAdapter<T>
where
    T: MountedService<B, S>,
    <T::Response as IntoResponse>::Body: http_body::Body<Data = D> + 'static,
    <<T::Response as IntoResponse>::Body as http_body::Body>::Error: core::error::Error + 'static,
    D: bytes::Buf + 'static,
{
    fn call<'a>(&'a self, request: MountedRequest<'a, B>, state: &'a S) -> ErasedFuture<'a, D>
    where
        B: 'a,
    {
        ErasedFuture {
            inner: Box::pin(async move { self.0.call(request, state).await.into_response().map(BoxBody::new) }),
        }
    }
}

struct AsyncFnAdapter<F>(F);

impl<B, S: ?Sized, D, F, R> ErasedCall<B, S, D> for AsyncFnAdapter<F>
where
    B: 'static,
    F: for<'a> AsyncFn(MountedRequest<'a, B>, &'a S) -> R,
    R: IntoResponse,
    R::Body: http_body::Body<Data = D> + 'static,
    <R::Body as http_body::Body>::Error: core::error::Error + 'static,
    D: bytes::Buf + 'static,
{
    fn call<'a>(&'a self, request: MountedRequest<'a, B>, state: &'a S) -> ErasedFuture<'a, D>
    where
        B: 'a,
    {
        ErasedFuture {
            inner: Box::pin(async move { (self.0)(request, state).await.into_response().map(BoxBody::new) }),
        }
    }
}

/// Builds an immutable [`ErasedMountRouter`].
///
/// Registrations are validated together by [`build`](Self::build), so method,
/// template, and deterministic conflict errors are startup failures rather
/// than request-time policy.
pub struct ErasedMountRouterBuilder<B, S: ?Sized, D: bytes::Buf = Bytes> {
    inner: DynBuilder<ErasedMountService<B, S, D>>,
    fallback: fn(http::StatusCode) -> Response<BoxBody<D>>,
}

impl<B, S: ?Sized> ErasedMountRouterBuilder<B, S, Bytes> {
    /// Creates an empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self::new_with_fallback(boxed_status)
    }
}

impl<B, S: ?Sized, D> ErasedMountRouterBuilder<B, S, D>
where
    D: bytes::Buf + 'static,
{
    /// Creates an empty builder with the response used for routing failures.
    ///
    /// Use this constructor when mounted services emit data other than
    /// [`Bytes`]. The fallback supplies matching data for `404` and `400`
    /// responses without requiring a conversion.
    #[must_use]
    pub fn new_with_fallback(fallback: fn(http::StatusCode) -> Response<BoxBody<D>>) -> Self {
        Self {
            inner: DynBuilder::new(),
            fallback,
        }
    }

    /// Registers one erased service for an HTTP method and path template.
    ///
    /// Call this more than once with clones of one
    /// [`ErasedMountService`] to create aliases. Errors are accumulated and
    /// returned by [`build`](Self::build).
    #[must_use]
    pub fn mount(mut self, method: impl AsRef<str>, path: impl AsRef<str>, service: ErasedMountService<B, S, D>) -> Self {
        self.inner.add_untyped(method, path.as_ref(), service);
        self
    }

    /// Validates all registrations and builds an immutable mount router.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError`] containing all invalid methods,
    /// templates, and conflicting method/template shapes.
    pub fn build(self) -> Result<ErasedMountRouter<B, S, D>, ConfigurationError> {
        self.inner.finish_mounts().map(|resolver| ErasedMountRouter {
            resolver,
            fallback: self.fallback,
        })
    }
}

impl<B, S: ?Sized> Default for ErasedMountRouterBuilder<B, S, Bytes> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B, S: ?Sized, D: bytes::Buf> fmt::Debug for ErasedMountRouterBuilder<B, S, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErasedMountRouterBuilder")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

/// An immutable method/path router for explicitly erased mounted services.
pub struct ErasedMountRouter<B, S: ?Sized, D: bytes::Buf = Bytes> {
    resolver: RawResolver<DynRoute<ErasedMountService<B, S, D>>>,
    fallback: fn(http::StatusCode) -> Response<BoxBody<D>>,
}

impl<B, S: ?Sized> ErasedMountRouter<B, S, Bytes> {
    /// Creates a startup builder.
    #[must_use]
    pub fn builder() -> ErasedMountRouterBuilder<B, S> {
        ErasedMountRouterBuilder::new()
    }
}

impl<B, S: ?Sized, D> ErasedMountRouter<B, S, D>
where
    D: bytes::Buf + 'static,
{
    /// Creates a startup builder with the response used for routing failures.
    #[must_use]
    pub fn builder_with_fallback(fallback: fn(http::StatusCode) -> Response<BoxBody<D>>) -> ErasedMountRouterBuilder<B, S, D> {
        ErasedMountRouterBuilder::new_with_fallback(fallback)
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
    pub async fn route(&self, request: http::Request<B>, state: &S) -> Response<BoxBody<D>> {
        let matched = self
            .resolver
            .resolve_scanned_checked(request.method().as_str(), request.uri().path(), |leaf, route, scanned| {
                (route.extractor(), MountedCaptureRanges::new(leaf, scanned))
            });
        match matched {
            Ok(Some((service, captures))) => service.call(MountedRequest { request, captures }, state).await,
            Ok(None) => (self.fallback)(http::StatusCode::NOT_FOUND),
            Err(_) => (self.fallback)(http::StatusCode::BAD_REQUEST),
        }
    }
}

impl<B, S: ?Sized, D: bytes::Buf> fmt::Debug for ErasedMountRouter<B, S, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErasedMountRouter")
            .field("resolver", &self.resolver)
            .finish_non_exhaustive()
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
    /// Decoding runs after the route has been chosen, so the decoded value can
    /// contain characters that are structural in a path, including `/`, `..`,
    /// and NUL: the request `/files/..%2F..%2Fetc%2Fpasswd` yields the capture
    /// `../../etc/passwd`. Validate the result before joining it into a
    /// filesystem path, a URL, or a command; see the matching semantics
    /// documented on [`crate::resolve`].
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

/// A concrete [`Send`] service contract that can be explicitly erased for
/// mounting behind a multi-threaded transport.
///
/// This is the [`Send`] counterpart of [`MountedService`]. It differs in
/// exactly one way: the returned future must be [`Send`], which is what lets
/// [`SendErasedMountService::new`] store the service behind an [`Arc`] and
/// erase its response body through [`SendBoxBody`] rather than the local
/// [`BoxBody`]. Implement [`MountedService`] instead whenever the mounted
/// response stays on one thread.
pub trait SendMountedService<B, S: ?Sized> {
    /// The concrete response produced by this service.
    type Response: IntoResponse;

    /// Handles one matched mounted request.
    fn call<'a>(&'a self, request: MountedRequest<'a, B>, state: &'a S) -> impl Future<Output = Self::Response> + Send + 'a
    where
        B: 'a;
}

/// An explicitly type-erased [`Send`] mounted service.
///
/// Construction allocates the stored service once into an [`Arc`], so cloning
/// this handle to register aliases only increments a refcount. Per-request
/// future and body allocations happen only when a mounted route matches, and
/// the response body is erased exactly once through [`SendBoxBody`].
///
/// The local [`ErasedMountService`] cannot be used here: its [`BoxBody`] is
/// deliberately not [`Send`], so a locally erased mounted response cannot be
/// re-erased for a `Send` transport. This type is that path.
pub struct SendErasedMountService<B, S: ?Sized, D: bytes::Buf = Bytes> {
    inner: Arc<dyn SendErasedCall<B, S, D> + Send + Sync>,
}

impl<B, S, D> SendErasedMountService<B, S, D>
where
    S: Sync + ?Sized,
    B: Send,
    D: bytes::Buf + 'static,
{
    /// Erases a named [`SendMountedService`].
    ///
    /// The concrete response body must be `Send + 'static` and its error
    /// `Send + Sync + 'static` because both are stored behind owned trait
    /// objects that cross a transport boundary.
    ///
    /// There is deliberately no `from_async_fn` counterpart here. Stable Rust
    /// cannot name the future of an [`AsyncFn`] closure, so a `Send` bound
    /// cannot be stated for it; [`SendMountedService`] expresses exactly that
    /// bound directly as `impl Future + Send`, so implement the trait on a
    /// named unit struct instead.
    #[must_use]
    pub fn new<T>(service: T) -> Self
    where
        T: SendMountedService<B, S> + Send + Sync + 'static,
        <T::Response as IntoResponse>::Body: http_body::Body<Data = D> + Send + 'static,
        <<T::Response as IntoResponse>::Body as http_body::Body>::Error: core::error::Error + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(SendServiceAdapter(service)),
        }
    }

    fn call<'a>(&'a self, request: MountedRequest<'a, B>, state: &'a S) -> SendErasedFuture<'a, D>
    where
        B: 'a,
    {
        self.inner.call(request, state)
    }
}

impl<B, S: ?Sized, D: bytes::Buf> Clone for SendErasedMountService<B, S, D> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<B, S: ?Sized, D: bytes::Buf> fmt::Debug for SendErasedMountService<B, S, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendErasedMountService").finish_non_exhaustive()
    }
}

struct SendErasedFuture<'a, D: bytes::Buf = Bytes> {
    inner: Pin<Box<dyn core::future::Future<Output = Response<SendBoxBody<D>>> + Send + 'a>>,
}

impl<D: bytes::Buf> Future for SendErasedFuture<'_, D> {
    type Output = Response<SendBoxBody<D>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

trait SendErasedCall<B, S: ?Sized, D: bytes::Buf> {
    fn call<'a>(&'a self, request: MountedRequest<'a, B>, state: &'a S) -> SendErasedFuture<'a, D>
    where
        B: 'a;
}

struct SendServiceAdapter<T>(T);

impl<B, S, D, T> SendErasedCall<B, S, D> for SendServiceAdapter<T>
where
    T: SendMountedService<B, S> + Sync,
    <T::Response as IntoResponse>::Body: http_body::Body<Data = D> + Send + 'static,
    <<T::Response as IntoResponse>::Body as http_body::Body>::Error: core::error::Error + Send + Sync + 'static,
    D: bytes::Buf + 'static,
    B: Send,
    S: Sync + ?Sized,
{
    fn call<'a>(&'a self, request: MountedRequest<'a, B>, state: &'a S) -> SendErasedFuture<'a, D>
    where
        B: 'a,
    {
        SendErasedFuture {
            inner: Box::pin(async move { self.0.call(request, state).await.into_response().map(SendBoxBody::new) }),
        }
    }
}

/// Builds an immutable [`SendErasedMountRouter`].
///
/// Registrations are validated together by [`build`](Self::build), so method,
/// template, and deterministic conflict errors are startup failures rather
/// than request-time policy.
pub struct SendErasedMountRouterBuilder<B, S: ?Sized, D: bytes::Buf = Bytes> {
    inner: DynBuilder<SendErasedMountService<B, S, D>>,
    fallback: fn(http::StatusCode) -> Response<SendBoxBody<D>>,
}

impl<B, S: ?Sized> SendErasedMountRouterBuilder<B, S, Bytes> {
    /// Creates an empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self::new_with_fallback(send_boxed_status)
    }
}

impl<B, S: ?Sized, D> SendErasedMountRouterBuilder<B, S, D>
where
    D: bytes::Buf + 'static,
{
    /// Creates an empty builder with the response used for routing failures.
    ///
    /// Use this constructor when mounted services emit data other than
    /// [`Bytes`].
    #[must_use]
    pub fn new_with_fallback(fallback: fn(http::StatusCode) -> Response<SendBoxBody<D>>) -> Self {
        Self {
            inner: DynBuilder::new(),
            fallback,
        }
    }

    /// Registers one erased service for an HTTP method and path template.
    ///
    /// Call this more than once with clones of one
    /// [`SendErasedMountService`] to create aliases. Errors are accumulated and
    /// returned by [`build`](Self::build).
    #[must_use]
    pub fn mount(mut self, method: impl AsRef<str>, path: impl AsRef<str>, service: SendErasedMountService<B, S, D>) -> Self {
        self.inner.add_untyped(method, path.as_ref(), service);
        self
    }

    /// Validates all registrations and builds an immutable mount router.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError`] containing all invalid methods,
    /// templates, and conflicting method/template shapes.
    pub fn build(self) -> Result<SendErasedMountRouter<B, S, D>, ConfigurationError> {
        self.inner.finish_mounts().map(|resolver| SendErasedMountRouter {
            resolver,
            fallback: self.fallback,
        })
    }
}

impl<B, S: ?Sized> Default for SendErasedMountRouterBuilder<B, S, Bytes> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B, S: ?Sized, D: bytes::Buf> fmt::Debug for SendErasedMountRouterBuilder<B, S, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendErasedMountRouterBuilder")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

/// An immutable method/path router for explicitly erased [`Send`] mounted
/// services.
///
/// Resolution is the same table and the same single vtable hop as
/// [`ErasedMountRouter`]; only the erasure boundary differs, so a mounted
/// response reaches a multi-threaded transport with one boxed body rather
/// than none at all.
pub struct SendErasedMountRouter<B, S: ?Sized, D: bytes::Buf = Bytes> {
    resolver: RawResolver<DynRoute<SendErasedMountService<B, S, D>>>,
    fallback: fn(http::StatusCode) -> Response<SendBoxBody<D>>,
}

impl<B, S: ?Sized> SendErasedMountRouter<B, S, Bytes> {
    /// Creates a startup builder.
    #[must_use]
    pub fn builder() -> SendErasedMountRouterBuilder<B, S> {
        SendErasedMountRouterBuilder::new()
    }
}

impl<B, S, D> SendErasedMountRouter<B, S, D>
where
    S: Sync + ?Sized,
    B: Send,
    D: bytes::Buf + 'static,
{
    /// Creates a startup builder with the response used for routing failures.
    #[must_use]
    pub fn builder_with_fallback(fallback: fn(http::StatusCode) -> Response<SendBoxBody<D>>) -> SendErasedMountRouterBuilder<B, S, D> {
        SendErasedMountRouterBuilder::new_with_fallback(fallback)
    }

    /// Routes one request entirely through the mounted-service table.
    ///
    /// A complete miss becomes `404 Not Found`; an invalid path becomes
    /// `400 Bad Request`. Request parts and body move directly into
    /// [`MountedRequest`] without boxing, cloning, or reconstruction.
    pub async fn route(&self, request: http::Request<B>, state: &S) -> Response<SendBoxBody<D>> {
        let matched = self
            .resolver
            .resolve_scanned_checked(request.method().as_str(), request.uri().path(), |leaf, route, scanned| {
                (route.extractor(), MountedCaptureRanges::new(leaf, scanned))
            });
        match matched {
            Ok(Some((service, captures))) => service.call(MountedRequest { request, captures }, state).await,
            Ok(None) => (self.fallback)(http::StatusCode::NOT_FOUND),
            Err(_) => (self.fallback)(http::StatusCode::BAD_REQUEST),
        }
    }
}

impl<B, S: ?Sized, D: bytes::Buf> fmt::Debug for SendErasedMountRouter<B, S, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendErasedMountRouter")
            .field("resolver", &self.resolver)
            .finish_non_exhaustive()
    }
}

/// A mounted-service table a generated router can delegate a complete miss to.
///
/// Generated routing takes precedence and keeps its concrete response body;
/// only a total generated miss reaches this trait. Making the generated entry
/// generic over this contract is what lets one generated method serve both the
/// local [`ErasedMountRouter`] and the [`SendErasedMountRouter`]: the response
/// body is an associated type rather than a hard-coded [`BoxBody`], so the
/// auto traits of the generated entry's opaque return type follow the delegate
/// that was actually passed. Dispatch is a monomorphized static call, so the
/// generalization costs nothing at run time.
///
/// Implement this for a custom mount table to plug it into a generated router
/// without an intermediate erasure.
pub trait MountDelegate<B, S: ?Sized> {
    /// The response body produced by a mounted match or routing fallback.
    type Body: http_body::Body;

    /// Routes one request that generated routing did not match.
    fn route(&self, request: http::Request<B>, state: &S) -> impl Future<Output = Response<Self::Body>>;
}

impl<B, S: ?Sized, D> MountDelegate<B, S> for ErasedMountRouter<B, S, D>
where
    D: bytes::Buf + 'static,
{
    type Body = BoxBody<D>;

    #[expect(
        clippy::future_not_send,
        reason = "the local mount boundary intentionally supports local futures, bodies, and state"
    )]
    fn route(&self, request: http::Request<B>, state: &S) -> impl Future<Output = Response<Self::Body>> {
        Self::route(self, request, state)
    }
}

impl<B, S, D> MountDelegate<B, S> for SendErasedMountRouter<B, S, D>
where
    S: Sync + ?Sized,
    B: Send,
    D: bytes::Buf + 'static,
{
    type Body = SendBoxBody<D>;

    fn route(&self, request: http::Request<B>, state: &S) -> impl Future<Output = Response<Self::Body>> {
        Self::route(self, request, state)
    }
}

/// Delegates through a shared handle.
///
/// The generated mounted entry takes `&__RouteramaMounts` by generic
/// reference, which does not deref-coerce. These forwarding impls let a
/// caller pass a mount table held behind a handle without widening the trait.
macro_rules! forward_mount_delegate {
    ($($holder:ty),* $(,)?) => {
        $(
            impl<B, S: ?Sized, T> MountDelegate<B, S> for $holder
            where
                T: MountDelegate<B, S> + ?Sized,
            {
                type Body = T::Body;

                fn route(&self, request: http::Request<B>, state: &S) -> impl Future<Output = Response<Self::Body>> {
                    (**self).route(request, state)
                }
            }
        )*
    };
}

forward_mount_delegate!(&T, Box<T>, Rc<T>, Arc<T>);

fn send_boxed_status(status: http::StatusCode) -> Response<SendBoxBody> {
    status.into_response().map(SendBoxBody::new)
}
