// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Context`] request/response metadata a handler reads and writes.

use core::mem;

use http::{HeaderMap, HeaderName, HeaderValue};

/// Request and response metadata for one handler invocation.
///
/// Request headers are readable by the handler; response headers are merged
/// into the HTTP response after the handler returns.
///
/// # Examples
///
/// ```
/// use http::{HeaderMap, HeaderValue, header};
/// use rest_over_grpc::handling::Context;
///
/// let mut request = HeaderMap::new();
/// request.insert(
///     header::AUTHORIZATION,
///     HeaderValue::from_static("Bearer t0ken"),
/// );
///
/// let mut cx = Context::new(request);
///
/// // Read an incoming header.
/// assert_eq!(
///     cx.request_headers().get(header::AUTHORIZATION).unwrap(),
///     "Bearer t0ken"
/// );
///
/// // Set an outgoing header.
/// cx.insert_response_header(header::LOCATION, HeaderValue::from_static("/v1/shelves/7"));
/// assert_eq!(
///     cx.response_headers().get(header::LOCATION).unwrap(),
///     "/v1/shelves/7"
/// );
/// ```
#[derive(Debug, Clone, Default)]
pub struct Context {
    request_headers: HeaderMap,
    response_headers: HeaderMap,
}

impl Context {
    /// Creates a context for a request carrying `request_headers`, with an empty
    /// set of response headers.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::HeaderMap;
    /// use rest_over_grpc::handling::Context;
    ///
    /// let cx = Context::new(HeaderMap::new());
    /// assert!(cx.request_headers().is_empty());
    /// assert!(cx.response_headers().is_empty());
    /// ```
    #[must_use]
    pub fn new(request_headers: HeaderMap) -> Self {
        Self {
            request_headers,
            response_headers: HeaderMap::new(),
        }
    }

    /// The incoming request headers (the request-side gRPC metadata).
    ///
    /// # Examples
    ///
    /// ```
    /// use http::{HeaderMap, HeaderValue, header};
    /// use rest_over_grpc::handling::Context;
    ///
    /// let mut request = HeaderMap::new();
    /// request.insert(header::USER_AGENT, HeaderValue::from_static("curl/8"));
    /// let cx = Context::new(request);
    /// assert_eq!(
    ///     cx.request_headers().get(header::USER_AGENT).unwrap(),
    ///     "curl/8"
    /// );
    /// ```
    #[must_use]
    pub const fn request_headers(&self) -> &HeaderMap {
        &self.request_headers
    }

    /// Takes the request headers out of the context, leaving an empty map in
    /// their place.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::{HeaderMap, HeaderValue, header};
    /// use rest_over_grpc::handling::Context;
    ///
    /// let mut request = HeaderMap::new();
    /// request.insert(header::USER_AGENT, HeaderValue::from_static("curl/8"));
    /// let mut cx = Context::new(request);
    ///
    /// let taken = cx.take_request_headers();
    /// assert_eq!(taken.get(header::USER_AGENT).unwrap(), "curl/8");
    /// assert!(cx.request_headers().is_empty());
    /// ```
    #[must_use]
    pub fn take_request_headers(&mut self) -> HeaderMap {
        mem::take(&mut self.request_headers)
    }

    /// The outgoing response headers accumulated so far (the response-side gRPC
    /// metadata).
    ///
    /// # Examples
    ///
    /// ```
    /// use http::HeaderMap;
    /// use rest_over_grpc::handling::Context;
    ///
    /// let cx = Context::new(HeaderMap::new());
    /// assert!(cx.response_headers().is_empty());
    /// ```
    #[must_use]
    pub const fn response_headers(&self) -> &HeaderMap {
        &self.response_headers
    }

    /// Returns the outgoing response headers for mutation.
    /// # Examples
    ///
    /// ```
    /// use http::{HeaderValue, header};
    /// use rest_over_grpc::handling::Context;
    ///
    /// let mut cx = Context::new(http::HeaderMap::new());
    /// cx.response_headers_mut()
    ///     .append(header::SET_COOKIE, HeaderValue::from_static("a=1"));
    /// cx.response_headers_mut()
    ///     .append(header::SET_COOKIE, HeaderValue::from_static("b=2"));
    /// assert_eq!(
    ///     cx.response_headers()
    ///         .get_all(header::SET_COOKIE)
    ///         .iter()
    ///         .count(),
    ///     2
    /// );
    /// ```
    pub fn response_headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.response_headers
    }

    /// Sets a single response header, replacing any existing value for `name`.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::{HeaderValue, header};
    /// use rest_over_grpc::handling::Context;
    ///
    /// let mut cx = Context::new(http::HeaderMap::new());
    /// cx.insert_response_header(header::ETAG, HeaderValue::from_static("\"v1\""));
    /// assert_eq!(cx.response_headers().get(header::ETAG).unwrap(), "\"v1\"");
    /// ```
    pub fn insert_response_header(&mut self, name: HeaderName, value: HeaderValue) -> Option<HeaderValue> {
        self.response_headers.insert(name, value)
    }

    /// Consumes the context, returning the accumulated response headers.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::{HeaderValue, header};
    /// use rest_over_grpc::handling::Context;
    ///
    /// let mut cx = Context::new(http::HeaderMap::new());
    /// cx.insert_response_header(header::LOCATION, HeaderValue::from_static("/x"));
    /// let headers = cx.into_response_headers();
    /// assert_eq!(headers.get(header::LOCATION).unwrap(), "/x");
    /// ```
    #[must_use]
    pub fn into_response_headers(self) -> HeaderMap {
        self.response_headers
    }

    /// Merges `headers` into the outgoing response headers, preserving repeated
    /// values (used by the `tonic` bridge to forward response metadata).
    pub fn merge_response_headers(&mut self, headers: HeaderMap) {
        append_headers(&mut self.response_headers, headers);
    }
}

/// Appends headers without discarding repeated values.
pub(crate) fn append_headers(dst: &mut HeaderMap, src: HeaderMap) {
    let mut current: Option<HeaderName> = None;
    for (name, value) in src {
        if name.is_some() {
            current = name;
        }
        if let Some(name) = current.clone() {
            dst.append(name, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use http::header;

    use super::*;

    #[test]
    fn new_starts_with_empty_response_headers() {
        let mut request = HeaderMap::new();
        request.insert(header::HOST, HeaderValue::from_static("example.test"));
        let cx = Context::new(request);
        assert_eq!(cx.request_headers().get(header::HOST).unwrap(), "example.test");
        assert!(cx.response_headers().is_empty());
    }

    #[test]
    fn take_request_headers_moves_out_and_empties_the_context() {
        let mut request = HeaderMap::new();
        request.insert(header::HOST, HeaderValue::from_static("example.test"));
        let mut cx = Context::new(request);

        let taken = cx.take_request_headers();
        // The taken map carries the original headers …
        assert_eq!(taken.get(header::HOST).unwrap(), "example.test");
        // … and the context's request headers are now empty (a real move, not a
        // fresh default left beside the untouched original).
        assert!(cx.request_headers().is_empty());
    }

    #[test]
    fn insert_response_header_replaces() {
        let mut cx = Context::new(HeaderMap::new());
        assert!(cx.insert_response_header(header::ETAG, HeaderValue::from_static("a")).is_none());
        let previous = cx.insert_response_header(header::ETAG, HeaderValue::from_static("b"));
        assert_eq!(previous.unwrap(), "a");
        assert_eq!(cx.response_headers().get(header::ETAG).unwrap(), "b");
    }

    #[test]
    fn merge_response_headers_preserves_repeated_values() {
        let mut cx = Context::new(HeaderMap::new());
        cx.response_headers_mut()
            .append(header::SET_COOKIE, HeaderValue::from_static("a=1"));

        let mut extra = HeaderMap::new();
        extra.append(header::SET_COOKIE, HeaderValue::from_static("b=2"));
        cx.merge_response_headers(extra);

        assert_eq!(cx.response_headers().get_all(header::SET_COOKIE).iter().count(), 2);
    }

    #[test]
    fn into_response_headers_returns_accumulated() {
        let mut cx = Context::new(HeaderMap::new());
        cx.insert_response_header(header::LOCATION, HeaderValue::from_static("/x"));
        let headers = cx.into_response_headers();
        assert_eq!(headers.get(header::LOCATION).unwrap(), "/x");
    }
}
