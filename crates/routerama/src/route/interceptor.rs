// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generated before/after interceptor contexts and control-flow outcomes.
//!
//! Interceptors are ordinary `async` methods on a `#[router]` service that the
//! macro calls directly. They are not boxed, do not require [`Send`], and add
//! no per-request allocation.
//!
//! Request interceptors receive one of two contexts, chosen by their scope:
//!
//! - a *router-wide* `#[before]` runs before route resolution and receives a
//!   [`BeforeContext`], which borrows the whole mutable request head and may
//!   therefore rewrite the method and URI and change routing;
//! - a *per-handler* `#[before(handler, ...)]` runs after route selection and
//!   receives a [`SelectedContext`], which reads the selected method, URI, and
//!   version and mutates only the headers and extensions. Its split borrow
//!   leaves the URI intact, so a handler may still take zero-copy borrowed path
//!   captures.
//!
//! Response interceptors receive an [`AfterContext`] that borrows the immutable
//! request head and the mutable response head. No context exposes the request
//! or response body, so a parts-only interceptor can never accidentally consume
//! a body.
//!
//! Terminal body transforms use [`BodyTransform`] and [`BodyConsumed`], which
//! make request-body ownership explicit: a transform either produces a
//! replacement body for later `#[body]` extraction or consumes the body with no
//! replacement, in which case a handler `#[body]` parameter is a compile-time
//! conflict. A buffered `#[transform(limit = N, ...)]` receives the bounded
//! [`bytes::Bytes`](bytes) it collected; a streaming
//! `#[transform(stream, ...)]` is generic over the transport body and receives
//! it by value, so it can wrap it lazily without buffering.

use http::request::Parts as RequestParts;
use http::response::Parts as ResponseParts;
use http::{Extensions, HeaderMap, Method, StatusCode, Uri, Version};

use crate::response::IntoResponse;

/// The mutable request head passed to a generated router-wide `#[before]`
/// interceptor.
///
/// A router-wide before interceptor runs at every generated entry, *before*
/// route resolution, extraction, and the handler. It owns the whole mutable
/// request head: it may read and mutate any request metadata, rewrite the
/// method or URI to change which handler is selected, and most commonly enrich
/// the typed [`Extensions`] map with an authenticated identity, a trace span,
/// or another request-local value that later extractors observe. It never
/// exposes the request body, so it cannot interfere with the single-consumer
/// body plan.
///
/// A per-handler `#[before(handler, ...)]` interceptor receives a
/// [`SelectedContext`] instead, because the URI already backs the selected
/// route's zero-copy captures at that point.
///
/// The typed extension helpers forward to [`http::Extensions`] and therefore
/// require the same value bounds as that map. They do not add a type-map lookup
/// to routes that never request an extension.
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
/// A per-handler before interceptor runs inside the selected dispatch arm,
/// after route selection and predicates and before extraction and the handler.
/// The selected route already borrows the request URI to produce zero-copy path
/// captures, so this context deliberately borrows the request head *by field*:
/// the method, URI, and version are readable, while the headers and extensions
/// stay mutable. A guard can therefore authenticate, enrich the typed
/// [`Extensions`] map, and normalize headers while the handler still receives
/// borrowed `&str` captures and [`ExtensionRef`](crate::route::ExtensionRef)
/// parameters.
///
/// Changing the method or URI after selection cannot change routing, so those
/// fields are read-only here; use a router-wide `#[before]`, which takes a
/// [`BeforeContext`] and runs before resolution, to rewrite them.
///
/// Like every parts-only interceptor context, it never exposes the request
/// body.
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
/// An after interceptor can mutate the response status, headers, and
/// extensions, most commonly to add tracing or security headers. It borrows the
/// original request head immutably so a response interceptor can correlate the
/// response with request metadata. It never exposes either body, so response
/// mutation cannot replace or drop the concrete generated body, and streaming
/// responses keep their frames and trailers.
///
/// Scope depends on the annotation:
///
/// - a bare `#[after]` observes **every response this router generates**:
///   handler responses, `#[before]`/`#[transform]` short-circuits, extractor
///   rejections and catcher responses, predicate rejections, and routing
///   failures or `#[fallback]` responses. It does not observe a response
///   produced by a mounted service, because that request head was moved into
///   the mount;
/// - an `#[after(handler, ...)]` observes only the responses its named handlers
///   returned, and runs before any bare `#[after]`.
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
/// Returning [`Replace`](Self::Replace) hands the replacement body to the
/// handler's `#[body]` extraction, which is how a transform composes with later
/// extraction. Returning [`Respond`](Self::Respond) short-circuits with a
/// response.
///
/// Both transform modes use this type, and both stay codegen-solvable:
///
/// - a buffered `#[transform(limit = N, ...)]` receives bounded
///   [`bytes::Bytes`](bytes) and returns a *concrete* replacement body `B`;
/// - a streaming `#[transform(stream, ...)]` is generic over the transport body
///   type and returns a replacement expressed in terms of that generic
///   parameter, for example `BodyTransform<Decompress<B>, R>`. The macro
///   substitutes the router's transport body type for `B`, so the handler's
///   `#[body]` parameter is checked against the exact wrapper type with no
///   boxing and no framework-imposed allocation.
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
/// A handler covered by a consuming transform must not declare a `#[body]`
/// parameter, because the body is already gone; that combination is a
/// compile-time conflict. Use this for size enforcement, body-aware logging, or
/// signature verification whose handler does not re-read the body. A buffered
/// transform consumes the collected [`bytes::Bytes`](bytes); a streaming
/// transform takes the transport body by value and may drain it without ever
/// buffering it.
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

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use http::{Request, Response};

    use super::*;
    use crate::response::Body;

    #[test]
    fn before_context_enriches_and_reads_request_metadata() {
        #[derive(Clone, Debug, PartialEq, Eq)]
        struct UserId(u32);

        let (mut parts, ()) = Request::builder()
            .method("PATCH")
            .uri("/widgets/7")
            .header("x-api-key", "secret")
            .body(())
            .expect("static request metadata is valid")
            .into_parts();

        let mut context = BeforeContext::new(&mut parts);
        assert_eq!(context.method(), Method::PATCH);
        assert_eq!(context.uri().path(), "/widgets/7");
        assert_eq!(context.headers()["x-api-key"], "secret");
        assert_eq!(context.insert_extension(UserId(42)), None);
        assert_eq!(context.get_extension::<UserId>(), Some(&UserId(42)));
        context.headers_mut().insert("x-checked", "1".parse().expect("valid header"));
        assert_eq!(context.remove_extension::<UserId>(), Some(UserId(42)));
        assert_eq!(context.get_extension::<UserId>(), None);

        assert_eq!(parts.headers["x-checked"], "1");
    }

    #[test]
    fn selected_context_coexists_with_a_borrowed_uri_capture() {
        #[derive(Clone, Debug, PartialEq, Eq)]
        struct UserId(u32);

        let (mut parts, ()) = Request::builder()
            .uri("/books/rust-in-action")
            .header("x-api-key", "secret")
            .body(())
            .expect("static request metadata is valid")
            .into_parts();

        // A zero-copy capture borrowed from the URI, exactly like the one a
        // selected route hands to a handler taking `&str`.
        let capture: &str = parts.uri.path().rsplit('/').next().expect("the path has a final segment");

        let mut context = SelectedContext::new(&parts.method, &parts.uri, parts.version, &mut parts.headers, &mut parts.extensions);
        assert_eq!(context.method(), Method::GET);
        assert_eq!(context.uri().path(), "/books/rust-in-action");
        assert_eq!(context.version(), Version::HTTP_11);
        assert_eq!(context.headers()["x-api-key"], "secret");
        assert_eq!(context.insert_extension(UserId(3)), None);
        assert_eq!(context.get_extension::<UserId>(), Some(&UserId(3)));
        context.headers_mut().insert("x-guarded", "1".parse().expect("valid header"));
        assert_eq!(context.remove_extension::<UserId>(), Some(UserId(3)));

        // The capture is still live after the guard mutated headers and
        // extensions, which is the borrow the split context preserves.
        assert_eq!(capture, "rust-in-action");
        assert_eq!(parts.headers["x-guarded"], "1");
    }

    #[test]
    fn after_context_mutates_response_and_reads_request() {
        let (request_parts, ()) = Request::builder()
            .uri("/status")
            .body(())
            .expect("static request metadata is valid")
            .into_parts();
        let (mut response_parts, _body) = Response::new(Body::empty()).into_parts();

        let mut context = AfterContext::new(&request_parts, &mut response_parts);
        assert_eq!(context.request().uri.path(), "/status");
        context.set_status(StatusCode::CREATED);
        context.headers_mut().insert("x-trace", "abc".parse().expect("valid header"));

        assert_eq!(response_parts.status, StatusCode::CREATED);
        assert_eq!(response_parts.headers["x-trace"], "abc");
    }

    #[test]
    fn control_flow_outcomes_are_constructible() {
        let next: Before<StatusCode> = Before::Next;
        assert_eq!(next, Before::Next);

        let replace: BodyTransform<Body, StatusCode> = BodyTransform::Replace(Body::from_bytes(Bytes::from_static(b"hi")));
        assert!(matches!(replace, BodyTransform::Replace(_)));

        let consumed: BodyConsumed<StatusCode> = BodyConsumed::Consumed;
        assert_eq!(consumed, BodyConsumed::Consumed);
    }
}
