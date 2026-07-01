// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`HttpResponse`] neutral HTTP response value type.

use http::StatusCode;

/// A transcoded HTTP response: a status, a content type, and a body.
///
/// This is intentionally web-stack-agnostic; an adapter converts it into the
/// response type of whatever server is in use.
///
/// # Examples
///
/// ```
/// use http::StatusCode;
/// use rest_over_grpc::HttpResponse;
///
/// let response = HttpResponse::json(StatusCode::CREATED, br#"{"name":"shelves/7"}"#.to_vec());
/// assert_eq!(response.status(), StatusCode::CREATED);
/// assert_eq!(response.content_type(), "application/json");
/// assert_eq!(response.body(), br#"{"name":"shelves/7"}"#);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HttpResponse {
    status: StatusCode,
    content_type: &'static str,
    body: Vec<u8>,
}

impl HttpResponse {
    /// Creates a response with an explicit status, content type, and body.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use rest_over_grpc::HttpResponse;
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
            body,
        }
    }

    /// Creates a `200 OK` `application/json` response with `body`.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use rest_over_grpc::HttpResponse;
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
    /// use rest_over_grpc::HttpResponse;
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

    /// The HTTP status code.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use rest_over_grpc::HttpResponse;
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
    /// use rest_over_grpc::HttpResponse;
    ///
    /// let response = HttpResponse::ok_json(b"{}".to_vec());
    /// assert_eq!(response.content_type(), "application/json");
    /// ```
    #[must_use]
    pub const fn content_type(&self) -> &'static str {
        self.content_type
    }

    /// The response body bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::HttpResponse;
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
    /// use rest_over_grpc::HttpResponse;
    ///
    /// let body = HttpResponse::ok_json(b"[]".to_vec()).into_body();
    /// assert_eq!(body, b"[]");
    /// ```
    #[must_use]
    pub fn into_body(self) -> Vec<u8> {
        self.body
    }

    /// Converts this response into an [`http::Response`] with the status and
    /// `Content-Type` header set.
    ///
    /// This is the bridge used by web-stack adapters; the `http` crate is the
    /// neutral standard shared by hyper, axum, tower, and others.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::HttpResponse;
    ///
    /// let http = HttpResponse::ok_json(b"{}".to_vec()).into_http();
    /// assert_eq!(
    ///     http.headers()[http::header::CONTENT_TYPE],
    ///     "application/json"
    /// );
    /// ```
    #[must_use]
    pub fn into_http(self) -> http::Response<Vec<u8>> {
        http::Response::builder()
            .status(self.status)
            .header(http::header::CONTENT_TYPE, self.content_type)
            .body(self.body)
            .unwrap_or_else(|_| {
                // A static content type and a valid status never fail to build;
                // fall back to a bare response to avoid a panic regardless.
                let mut fallback = http::Response::new(Vec::new());
                *fallback.status_mut() = self.status;
                fallback
            })
    }
}

impl From<HttpResponse> for Vec<u8> {
    /// Returns the owned response body bytes (see [`HttpResponse::into_body`]).
    fn from(response: HttpResponse) -> Self {
        response.into_body()
    }
}

impl From<HttpResponse> for http::Response<Vec<u8>> {
    /// Builds an [`http::Response`] with the status and `Content-Type` header set
    /// (see [`HttpResponse::into_http`]).
    fn from(response: HttpResponse) -> Self {
        response.into_http()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_accessors() {
        let r = HttpResponse::ok_json(b"{}".to_vec());
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.content_type(), "application/json");
        assert_eq!(r.body(), b"{}");
        assert_eq!(r.into_body(), b"{}");
    }

    #[test]
    fn into_http_falls_back_on_invalid_content_type() {
        // A content type with control bytes can't be set as a header value, so
        // `into_http` takes its bare-response fallback while preserving the status.
        let r = HttpResponse::new(
            StatusCode::IM_A_TEAPOT,
            "bad
value",
            b"body".to_vec(),
        );
        let http = r.into_http();
        assert_eq!(http.status(), StatusCode::IM_A_TEAPOT);
        assert!(http.headers().get(http::header::CONTENT_TYPE).is_none());
        assert!(http.body().is_empty());
    }

    #[test]
    fn from_conversions_match_inherent_methods() {
        let bytes = Vec::<u8>::from(HttpResponse::ok_json(b"{}".to_vec()));
        assert_eq!(bytes, b"{}");

        let http = http::Response::<Vec<u8>>::from(HttpResponse::ok_json(b"[]".to_vec()));
        assert_eq!(http.status(), StatusCode::OK);
        assert_eq!(http.headers()[http::header::CONTENT_TYPE], "application/json");
    }
}
