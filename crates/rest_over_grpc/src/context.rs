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

/// Removes connection-management and message-framing headers that a handler
/// must not control from `headers`.
///
/// Handler-supplied response headers flow onto the wire response, but framing
/// (`Content-Length`, `Transfer-Encoding`) and hop-by-hop headers (`Connection`
/// and the connection tokens it governs) belong to the serving stack. Leaving a
/// stale `Content-Length` or `Transfer-Encoding` in place would corrupt the
/// response, so they are stripped before the handler's headers are applied.
pub(crate) fn strip_uncontrolled_response_headers(headers: &mut HeaderMap) {
    use http::header;

    const HOP_BY_HOP: &[HeaderName] = &[
        header::CONTENT_LENGTH,
        header::TRANSFER_ENCODING,
        header::CONNECTION,
        header::TE,
        header::TRAILER,
        header::UPGRADE,
        header::PROXY_AUTHENTICATE,
        header::PROXY_AUTHORIZATION,
    ];

    // Headers named by `Connection` tokens are hop-by-hop for this message and
    // must be stripped alongside the fixed set (RFC 9110 §7.6.1). Collect them
    // before `Connection` itself is removed below. Tokens that are directives
    // (`close`, `keep-alive`) rather than header names simply match nothing.
    let connection_named: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|token| HeaderName::from_bytes(token.trim().as_bytes()).ok())
        .collect();
    for name in &connection_named {
        while headers.remove(name).is_some() {}
    }

    for name in HOP_BY_HOP {
        let _ = headers.remove(name);
    }
    // `Keep-Alive` and `Proxy-Connection` have no `http::header` constant.
    for name in ["keep-alive", "proxy-connection"] {
        let _ = headers.remove(name);
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
        assert_eq!(taken.get(header::HOST).unwrap(), "example.test");
        assert!(cx.request_headers().is_empty());
    }

    #[test]
    fn strip_uncontrolled_response_headers_removes_framing_and_hop_by_hop() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("10"));
        headers.insert(header::TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
        headers.insert(header::CONNECTION, HeaderValue::from_static("close"));
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert(header::ETAG, HeaderValue::from_static("\"v1\""));

        strip_uncontrolled_response_headers(&mut headers);

        assert!(headers.get(header::CONTENT_LENGTH).is_none());
        assert!(headers.get(header::TRANSFER_ENCODING).is_none());
        assert!(headers.get(header::CONNECTION).is_none());
        assert!(headers.get("keep-alive").is_none());
        assert_eq!(headers.get(header::ETAG).unwrap(), "\"v1\"");
    }

    #[test]
    fn strip_uncontrolled_response_headers_removes_connection_named_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONNECTION, HeaderValue::from_static("close, X-Foo, x-bar"));
        headers.insert("x-foo", HeaderValue::from_static("1"));
        headers.append("x-bar", HeaderValue::from_static("a"));
        headers.append("x-bar", HeaderValue::from_static("b"));
        headers.insert(header::ETAG, HeaderValue::from_static("\"v1\""));

        strip_uncontrolled_response_headers(&mut headers);

        assert!(headers.get(header::CONNECTION).is_none());
        assert!(headers.get("x-foo").is_none(), "header named by a Connection token is stripped");
        assert!(
            headers.get("x-bar").is_none(),
            "every value of a Connection-named header is stripped"
        );
        assert_eq!(headers.get(header::ETAG).unwrap(), "\"v1\"");
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
