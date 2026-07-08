// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`HttpResponse`] neutral HTTP response value type.

use http::StatusCode;
use serde::Serialize;

use crate::handling::Status;

/// The JSON body shape for a [`Status`] response, mirroring `google.rpc.Status`:
/// `{"code": <i32>, "message": <string>, "details": [ … ]}` (the `details` array
/// is omitted when empty).
#[derive(Serialize)]
struct StatusBody<'a> {
    code: i32,
    message: &'a str,
    #[serde(skip_serializing_if = "<[serde_json::Value]>::is_empty")]
    details: &'a [serde_json::Value],
}

/// A transcoded HTTP response: a status, a content type, and a body.
///
/// This is intentionally web-stack-agnostic; an adapter converts it into the
/// response type of whatever server is in use.
///
/// # Examples
///
/// ```
/// use http::StatusCode;
/// use rest_over_grpc::transcoding::HttpResponse;
///
/// let response = HttpResponse::json(StatusCode::CREATED, br#"{"name":"shelves/7"}"#.to_vec());
/// assert_eq!(response.status(), StatusCode::CREATED);
/// assert_eq!(response.content_type(), "application/json");
/// assert_eq!(response.body(), br#"{"name":"shelves/7"}"#);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    status: StatusCode,
    content_type: &'static str,
    headers: http::HeaderMap,
    body: Vec<u8>,
}

impl HttpResponse {
    /// Creates a response with an explicit status, content type, and body, and
    /// no extra headers.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use rest_over_grpc::transcoding::HttpResponse;
    ///
    /// let response = HttpResponse::new(StatusCode::ACCEPTED, "text/plain", b"queued".to_vec());
    /// assert_eq!(response.status(), StatusCode::ACCEPTED);
    /// assert_eq!(response.content_type(), "text/plain");
    /// assert_eq!(response.body(), b"queued");
    /// ```
    #[must_use]
    pub fn new(status: StatusCode, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type,
            headers: http::HeaderMap::new(),
            body,
        }
    }

    /// Creates a `200 OK` `application/json` response with `body`.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use rest_over_grpc::transcoding::HttpResponse;
    ///
    /// let response = HttpResponse::ok_json(br#"{"ok":true}"#.to_vec());
    /// assert_eq!(response.status(), StatusCode::OK);
    /// assert_eq!(response.content_type(), "application/json");
    /// ```
    #[must_use]
    pub fn ok_json(body: Vec<u8>) -> Self {
        Self::new(StatusCode::OK, "application/json", body)
    }

    /// Creates an `application/json` response with an explicit `status`.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use rest_over_grpc::transcoding::HttpResponse;
    ///
    /// let response = HttpResponse::json(StatusCode::CREATED, br#"{"id":"7"}"#.to_vec());
    /// assert_eq!(response.status(), StatusCode::CREATED);
    /// assert_eq!(response.content_type(), "application/json");
    /// assert_eq!(response.into_body(), br#"{"id":"7"}"#);
    /// ```
    #[must_use]
    pub fn json(status: StatusCode, body: Vec<u8>) -> Self {
        Self::new(status, "application/json", body)
    }

    /// Renders a [`Status`] as a JSON response, mapping its [`Code`](crate::handling::Code)
    /// to the corresponding HTTP status.
    ///
    /// The body mirrors `google.rpc.Status`:
    /// `{"code": <i32>, "message": <string>, "details": [ … ]}`, with the
    /// `details` array omitted when the status carries none.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::handling::{Code, Status};
    /// use rest_over_grpc::transcoding::HttpResponse;
    ///
    /// let response = HttpResponse::from_status(&Status::not_found("shelf 7"));
    /// assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    ///
    /// let body: serde_json::Value = serde_json::from_slice(response.body())?;
    /// assert_eq!(body["code"], Code::NotFound.as_i32());
    /// assert_eq!(body["message"], "shelf 7");
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    #[must_use]
    pub fn from_status(status: &Status) -> Self {
        let http = status.code().to_http_status();
        let body = StatusBody {
            code: status.code().as_i32(),
            message: status.message(),
            details: status.details(),
        };
        let bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
        Self::json(http, bytes)
    }

    /// Renders a `404 Not Found` JSON response for an unmatched route.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::handling::Code;
    /// use rest_over_grpc::transcoding::HttpResponse;
    ///
    /// let response = HttpResponse::not_found();
    /// assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    ///
    /// let body: serde_json::Value = serde_json::from_slice(response.body())?;
    /// assert_eq!(body["code"], Code::NotFound.as_i32());
    /// assert_eq!(body["message"], "no route matches the request");
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    #[must_use]
    pub fn not_found() -> Self {
        // Build the body directly rather than via `from_status(&Status::…)`: this
        // path runs for every unmatched route, and the message is a static string
        // with no details, so there is no need to allocate an intermediate
        // `Status` (an owned `String` plus a details `Vec`) just to serialize it.
        let body = StatusBody {
            code: crate::handling::Code::NotFound.as_i32(),
            message: "no route matches the request",
            details: &[],
        };
        let bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
        Self::json(crate::handling::Code::NotFound.to_http_status(), bytes)
    }

    /// The HTTP status code.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use rest_over_grpc::transcoding::HttpResponse;
    ///
    /// let response = HttpResponse::ok_json(b"{}".to_vec());
    /// assert_eq!(response.status(), StatusCode::OK);
    /// ```
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.status
    }

    /// The `Content-Type` header value.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::transcoding::HttpResponse;
    ///
    /// let response = HttpResponse::ok_json(b"{}".to_vec());
    /// assert_eq!(response.content_type(), "application/json");
    /// ```
    #[must_use]
    pub const fn content_type(&self) -> &'static str {
        self.content_type
    }

    /// The custom response headers set on this response (excluding the
    /// `Content-Type`, which is tracked separately by
    /// [`content_type`](Self::content_type)).
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::transcoding::HttpResponse;
    ///
    /// let response = HttpResponse::ok_json(b"{}".to_vec());
    /// assert!(response.headers().is_empty());
    /// ```
    #[must_use]
    pub const fn headers(&self) -> &http::HeaderMap {
        &self.headers
    }

    /// A mutable handle to the custom response headers.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::{HeaderValue, header};
    /// use rest_over_grpc::transcoding::HttpResponse;
    ///
    /// let mut response = HttpResponse::ok_json(b"{}".to_vec());
    /// response
    ///     .headers_mut()
    ///     .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    /// assert_eq!(
    ///     response.headers().get(header::CACHE_CONTROL).unwrap(),
    ///     "no-store"
    /// );
    /// ```
    pub fn headers_mut(&mut self) -> &mut http::HeaderMap {
        &mut self.headers
    }

    /// Sets a custom response header, returning the updated response.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::{HeaderValue, header};
    /// use rest_over_grpc::transcoding::HttpResponse;
    ///
    /// let response = HttpResponse::ok_json(b"{}".to_vec())
    ///     .with_header(header::LOCATION, HeaderValue::from_static("/v1/shelves/7"));
    /// assert_eq!(
    ///     response.headers().get(header::LOCATION).unwrap(),
    ///     "/v1/shelves/7"
    /// );
    /// ```
    #[must_use]
    pub fn with_header(mut self, name: http::HeaderName, value: http::HeaderValue) -> Self {
        _ = self.headers.insert(name, value);
        self
    }

    /// Merges `headers` into this response's custom headers, preserving repeated
    /// values (e.g. multiple `Set-Cookie` lines).
    ///
    /// Used by the generated transcoder to apply the response headers a handler
    /// accumulated on its [`Context`](crate::handling::Context).
    pub fn merge_headers(&mut self, headers: http::HeaderMap) {
        crate::context::append_headers(&mut self.headers, headers);
    }

    /// The response body bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::transcoding::HttpResponse;
    ///
    /// let response = HttpResponse::ok_json(br#"{"ok":true}"#.to_vec());
    /// assert_eq!(response.body(), br#"{"ok":true}"#);
    /// ```
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Consumes the response, returning the owned body bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::transcoding::HttpResponse;
    ///
    /// let body = HttpResponse::ok_json(b"[]".to_vec()).into_body();
    /// assert_eq!(body, b"[]");
    /// ```
    #[must_use]
    pub fn into_body(self) -> Vec<u8> {
        self.body
    }

    /// Converts this response into an [`http::Response`] with the status,
    /// `Content-Type`, and any custom headers set.
    ///
    /// This is the bridge used by web-stack adapters; the `http` crate is the
    /// neutral standard shared by hyper, axum, tower, and others. The
    /// `Content-Type` is authoritative: it is applied after the custom headers,
    /// so a stray `content-type` among them never overrides the negotiated
    /// value.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::{HeaderValue, header};
    /// use rest_over_grpc::transcoding::HttpResponse;
    ///
    /// let http = HttpResponse::ok_json(b"{}".to_vec())
    ///     .with_header(header::ETAG, HeaderValue::from_static("\"v1\""))
    ///     .into_http();
    /// assert_eq!(http.headers()[header::CONTENT_TYPE], "application/json");
    /// assert_eq!(http.headers()[header::ETAG], "\"v1\"");
    /// ```
    #[must_use]
    pub fn into_http(self) -> http::Response<Vec<u8>> {
        let Self {
            status,
            content_type,
            mut headers,
            body,
        } = self;
        // Build the content-type value from the static string without copying it:
        // `Builder::header(name, &str)` / `HeaderValue::try_from(&str)` would
        // allocate a fresh buffer per response, whereas `from_maybe_shared` over a
        // static `Bytes` adopts the static bytes directly.
        let Ok(content_type) = http::HeaderValue::from_maybe_shared(bytes::Bytes::from_static(content_type.as_bytes())) else {
            // An invalid caller-supplied content type: fall back to a bare
            // response that still carries the status rather than panicking.
            let mut fallback = http::Response::new(Vec::new());
            *fallback.status_mut() = status;
            return fallback;
        };
        let mut response = http::Response::new(body);
        *response.status_mut() = status;
        let dst = response.headers_mut();
        // The negotiated content type is authoritative: drop any custom one, set
        // the negotiated value, then move the remaining custom headers in (this
        // response owns them, so they need not be cloned).
        headers.remove(http::header::CONTENT_TYPE);
        let _ = dst.insert(http::header::CONTENT_TYPE, content_type);
        crate::context::append_headers(dst, headers);
        response
    }
}

impl From<HttpResponse> for Vec<u8> {
    fn from(response: HttpResponse) -> Self {
        response.into_body()
    }
}

impl From<HttpResponse> for http::Response<Vec<u8>> {
    fn from(response: HttpResponse) -> Self {
        response.into_http()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_getter_exposes_the_custom_header_map() {
        let mut response = HttpResponse::ok_json(b"{}".to_vec());
        assert!(response.headers().is_empty());
        _ = response
            .headers_mut()
            .insert(http::header::ETAG, http::HeaderValue::from_static("\"v1\""));
        assert_eq!(response.headers()[http::header::ETAG], "\"v1\"");
    }
}
