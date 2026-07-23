// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Context and control-flow types for generated interceptors.
//!
//! Router-wide [`BeforeContext`] can rewrite the request head before
//! resolution. Per-handler [`SelectedContext`] preserves the selected URI while
//! allowing header and extension mutation. [`AfterContext`] exposes immutable
//! request metadata and mutable response metadata. [`BodyTransform`] and
//! [`BodyConsumed`] express terminal request-body ownership.

use http::request::Parts as RequestParts;
use http::response::Parts as ResponseParts;
use http::{Extensions, HeaderMap, Method, StatusCode, Uri, Version};

use crate::response::IntoResponse;

/// The mutable request head passed to a generated router-wide `#[before]`
/// interceptor.
///
/// It runs before route resolution and may mutate all request metadata. The
/// request body is not exposed.
#[derive(Debug)]
pub struct BeforeContext<'request> {
    parts: &'request mut RequestParts,
}

impl<'request> BeforeContext<'request> {
    /// Wraps the mutable request head.
    #[doc(hidden)]
    #[must_use]
    pub fn new(parts: &'request mut RequestParts) -> Self {
        Self { parts }
    }

    /// Returns the request method.
    #[must_use]
    pub const fn method(&self) -> &Method {
        &self.parts.method
    }

    /// Returns mutable access to the request method.
    #[must_use]
    pub const fn method_mut(&mut self) -> &mut Method {
        &mut self.parts.method
    }

    /// Returns the request URI.
    #[must_use]
    pub const fn uri(&self) -> &Uri {
        &self.parts.uri
    }

    /// Returns mutable access to the request URI.
    ///
    /// A router-wide `#[before]` interceptor runs before route resolution, so
    /// rewriting the URI path here changes which handler is selected.
    #[must_use]
    pub const fn uri_mut(&mut self) -> &mut Uri {
        &mut self.parts.uri
    }

    /// Returns the HTTP version.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.parts.version
    }

    /// Returns the request headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderMap {
        &self.parts.headers
    }

    /// Returns mutable access to the request headers.
    #[must_use]
    pub const fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.parts.headers
    }

    /// Returns the request extensions.
    #[must_use]
    pub const fn extensions(&self) -> &Extensions {
        &self.parts.extensions
    }

    /// Returns mutable access to the request extensions.
    #[must_use]
    pub const fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.parts.extensions
    }

    /// Returns the whole immutable request head.
    #[must_use]
    pub const fn parts(&self) -> &RequestParts {
        self.parts
    }

    /// Returns mutable access to the whole request head.
    #[must_use]
    pub const fn parts_mut(&mut self) -> &mut RequestParts {
        self.parts
    }

    /// Inserts a typed request-local value, returning any previous value.
    ///
    /// Downstream extractors observe the value through
    /// [`ExtensionRef`](crate::route::ExtensionRef) or
    /// [`ClonedExtension`](crate::route::ClonedExtension).
    pub fn insert_extension<T>(&mut self, value: T) -> Option<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        self.parts.extensions.insert(value)
    }

    /// Returns a typed request-local value inserted earlier.
    #[must_use]
    pub fn get_extension<T>(&self) -> Option<&T>
    where
        T: Send + Sync + 'static,
    {
        self.parts.extensions.get()
    }

    /// Removes and returns a typed request-local value.
    pub fn remove_extension<T>(&mut self) -> Option<T>
    where
        T: Send + Sync + 'static,
    {
        self.parts.extensions.remove()
    }
}

/// The split request head passed to a generated per-handler
/// `#[before(handler, ...)]` interceptor.
///
/// It provides read-only method, URI, and version access with mutable headers
/// and extensions. The split borrow preserves path captures.
#[derive(Debug)]
pub struct SelectedContext<'request> {
    method: &'request Method,
    uri: &'request Uri,
    version: Version,
    headers: &'request mut HeaderMap,
    extensions: &'request mut Extensions,
}

impl<'request> SelectedContext<'request> {
    /// Wraps the split request head of an already-selected route.
    ///
    /// The macro passes disjoint borrows of the same [`RequestParts`], which is
    /// what lets a per-handler guard coexist with borrowed path captures.
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        method: &'request Method,
        uri: &'request Uri,
        version: Version,
        headers: &'request mut HeaderMap,
        extensions: &'request mut Extensions,
    ) -> Self {
        Self {
            method,
            uri,
            version,
            headers,
            extensions,
        }
    }

    /// Returns the request method of the selected route.
    #[must_use]
    pub const fn method(&self) -> &Method {
        self.method
    }

    /// Returns the request URI of the selected route.
    ///
    /// The URI is read-only because the selected route's zero-copy captures
    /// borrow from it and because rewriting it after selection cannot change
    /// routing.
    #[must_use]
    pub const fn uri(&self) -> &Uri {
        self.uri
    }

    /// Returns the HTTP version.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// Returns the request headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderMap {
        self.headers
    }

    /// Returns mutable access to the request headers.
    #[must_use]
    pub const fn headers_mut(&mut self) -> &mut HeaderMap {
        self.headers
    }

    /// Returns the request extensions.
    #[must_use]
    pub const fn extensions(&self) -> &Extensions {
        self.extensions
    }

    /// Returns mutable access to the request extensions.
    #[must_use]
    pub const fn extensions_mut(&mut self) -> &mut Extensions {
        self.extensions
    }

    /// Inserts a typed request-local value, returning any previous value.
    ///
    /// Downstream extractors of the selected handler observe the value through
    /// [`ExtensionRef`](crate::route::ExtensionRef) or
    /// [`ClonedExtension`](crate::route::ClonedExtension).
    pub fn insert_extension<T>(&mut self, value: T) -> Option<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        self.extensions.insert(value)
    }

    /// Returns a typed request-local value inserted earlier.
    ///
    /// A router-wide `#[before]` interceptor runs first, so its enrichment is
    /// visible here.
    #[must_use]
    pub fn get_extension<T>(&self) -> Option<&T>
    where
        T: Send + Sync + 'static,
    {
        self.extensions.get()
    }

    /// Removes and returns a typed request-local value.
    pub fn remove_extension<T>(&mut self) -> Option<T>
    where
        T: Send + Sync + 'static,
    {
        self.extensions.remove()
    }
}

/// The immutable request head and mutable response head passed to a generated
/// `#[after]` interceptor.
///
/// Bare interceptors observe every generated response; handler-scoped
/// interceptors observe only their handlers. Mounted responses are excluded.
/// Neither request nor response bodies are exposed.
#[derive(Debug)]
pub struct AfterContext<'response> {
    request: &'response RequestParts,
    response: &'response mut ResponseParts,
}

impl<'response> AfterContext<'response> {
    /// Wraps the immutable request head and mutable response head.
    #[doc(hidden)]
    #[must_use]
    pub fn new(request: &'response RequestParts, response: &'response mut ResponseParts) -> Self {
        Self { request, response }
    }

    /// Returns the original request head.
    #[must_use]
    pub const fn request(&self) -> &RequestParts {
        self.request
    }

    /// Returns the response status.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.response.status
    }

    /// Sets the response status.
    pub const fn set_status(&mut self, status: StatusCode) {
        self.response.status = status;
    }

    /// Returns mutable access to the response status.
    #[must_use]
    pub const fn status_mut(&mut self) -> &mut StatusCode {
        &mut self.response.status
    }

    /// Returns the response version.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.response.version
    }

    /// Returns the response headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderMap {
        &self.response.headers
    }

    /// Returns mutable access to the response headers.
    #[must_use]
    pub const fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.response.headers
    }

    /// Returns the response extensions.
    #[must_use]
    pub const fn extensions(&self) -> &Extensions {
        &self.response.extensions
    }

    /// Returns mutable access to the response extensions.
    #[must_use]
    pub const fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.response.extensions
    }

    /// Inserts a typed response extension, returning any previous value.
    pub fn insert_extension<T>(&mut self, value: T) -> Option<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        self.response.extensions.insert(value)
    }

    /// Returns a typed response extension.
    #[must_use]
    pub fn get_extension<T>(&self) -> Option<&T>
    where
        T: Send + Sync + 'static,
    {
        self.response.extensions.get()
    }

    /// Removes and returns a typed response extension.
    pub fn remove_extension<T>(&mut self) -> Option<T>
    where
        T: Send + Sync + 'static,
    {
        self.response.extensions.remove()
    }
}

/// The control-flow outcome of a generated `#[before]` request interceptor.
///
/// Returning [`Next`](Self::Next) continues to the next interceptor and then
/// the handler. Returning [`Respond`](Self::Respond) short-circuits: no further
/// interceptor, extractor, or handler runs, and `R` becomes the response
/// through [`IntoResponse`]. The short-circuit response type enters the
/// generated concrete response body sum like any other handler response, so it
/// is neither boxed nor dynamically dispatched, and a bare `#[after]`
/// interceptor still observes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "a before interceptor outcome selects whether the handler runs"]
pub enum Before<R>
where
    R: IntoResponse,
{
    /// Continue to the next interceptor and the handler.
    Next,
    /// Short-circuit with this response; skip the handler.
    Respond(R),
}

/// The outcome of a generated `#[transform]` interceptor that replaces the
/// request body.
///
/// [`Replace`](Self::Replace) supplies a concrete body for later extraction;
/// [`Respond`](Self::Respond) short-circuits. Buffered and streaming transforms
/// use the same outcome type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "a transform outcome selects the replacement body or a short-circuit response"]
pub enum BodyTransform<B, R>
where
    R: IntoResponse,
{
    /// Continue with the replacement body for `#[body]` extraction.
    Replace(B),
    /// Short-circuit with this response; skip the handler.
    Respond(R),
}

/// The outcome of a generated `#[transform]` interceptor that consumes the
/// request body without producing a replacement.
///
/// Covered handlers cannot also declare a `#[body]` parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "a transform outcome selects whether the handler runs"]
pub enum BodyConsumed<R>
where
    R: IntoResponse,
{
    /// Continue to the handler with no request body.
    Consumed,
    /// Short-circuit with this response; skip the handler.
    Respond(R),
}
