// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Standalone HTTP response bodies and typed response composition.
//!
//! [`IntoResponse`] retains one concrete `http_body::Body` whose data type is
//! [`Bytes`]. In particular, `http::Response<B>` is returned unchanged when
//! `B` is such a body. Use the status/header tuple implementations around a
//! string or byte value, or construct `http::Response<Body>`, when starting
//! from a value that is not itself an HTTP body.
//!
//! This module is enabled by the `response` Cargo feature. It does not depend
//! on path matching, request extraction, generated routing, query codecs, or
//! JSON support. The `route` feature enables it transitively and generated
//! routers consume these same canonical traits and body types.
//!
//! # Body choices
//!
//! [`Body`] is the built-in zero-or-one-frame byte body. [`EitherBody`] retains
//! either of two concrete body types without allocation, and [`NeverBody`]
//! represents an impossible response branch. [`BoxBody`] is the explicit
//! allocating escape hatch for an open set of body types; it does not require
//! `Send` or `Sync`. [`SendBoxBody`] is the same boundary for transports that
//! require a `Send + 'static` response body, such as a Tower or Hyper service.
//! Both erasures forward data frames, trailers, size hints, and end-of-stream
//! state unchanged and box a body error only if one occurs.
//!
//! # Fallible metadata
//!
//! [`IntoResponseParts`] receives body-free [`ResponseParts`] and may return a
//! typed rejection that converts independently through [`IntoResponse`]:
//!
//! ```
//! use http::StatusCode;
//! use http::header::{HeaderName, HeaderValue};
//! use routerama::response::{Body, IntoResponse, IntoResponseParts, Response, ResponseParts};
//!
//! struct CheckedHeader(&'static str);
//! struct InvalidHeader;
//!
//! impl IntoResponse for InvalidHeader {
//!     type Body = Body;
//!
//!     fn into_response(self) -> Response {
//!         let mut response = Response::new(Body::from("invalid response header"));
//!         *response.status_mut() = StatusCode::BAD_REQUEST;
//!         response
//!     }
//! }
//!
//! impl IntoResponseParts for CheckedHeader {
//!     type Error = InvalidHeader;
//!
//!     fn into_response_parts(
//!         self,
//!         mut response: ResponseParts,
//!     ) -> Result<ResponseParts, Self::Error> {
//!         let value: HeaderValue = self.0.parse().map_err(|_invalid| InvalidHeader)?;
//!         response
//!             .headers_mut()
//!             .insert(HeaderName::from_static("x-checked"), value);
//!         Ok(response)
//!     }
//! }
//!
//! let response = (CheckedHeader("contains\nnewline"), "discarded").into_response();
//! assert_eq!(response.status(), StatusCode::BAD_REQUEST);
//! ```
//!
//! # Tuple order and failure precedence
//!
//! The final tuple item is converted first and supplies the initial status,
//! headers, extensions, and body. Metadata is then applied from right to left:
//!
//! - `(part, value)` applies `part`;
//! - `(first, second, value)` applies `second`, then `first`.
//!
//! The built-in status, header-map, and header-array parts replace existing
//! values for the fields or header names they carry. The leftmost built-in part
//! therefore wins over parts to its right and over the final response. Header
//! arrays process their entries from left to right, making the last duplicate
//! within one array win.
//!
//! Failure short-circuits in that same order. In a three-item tuple, a
//! `second` failure prevents `first` from running; a `first` failure occurs
//! only after `second` succeeded. The failing part's response is returned
//! independently, without applying an outer status/header to it. The original
//! success body and every partially modified [`ResponseParts`] value are
//! dropped.
//!
//! Bodies remain unboxed. `(part, value)` uses
//! `EitherBody<value_body, part_error_body>`. A three-item tuple uses
//! `EitherBody<value_body, EitherBody<second_error_body,
//! first_error_body>>`, matching the application and failure order exactly.
//! Body frames, trailers, errors, and auto traits remain those of the concrete
//! branches.

use alloc::string::String;
use alloc::vec::Vec;
use core::convert::Infallible;

use bytes::Bytes;
use http::header::{CONTENT_TYPE, HeaderName, HeaderValue};
use http::{Extensions, HeaderMap, StatusCode, Version};
use http_body::Body as HttpBody;

mod body;

pub use body::{Body, BoxBody, BoxBodyError, EitherBody, EitherBodyError, NeverBody, SendBoxBody, SendBoxBodyError};

/// An HTTP response, using Routerama's fixed [`Body`] by default.
pub type Response<B = Body> = http::Response<B>;

/// Converts a handler result or rejection into an HTTP response.
///
/// Response bodies with another data type are rejected at the associated type:
///
/// ```compile_fail
/// use bytes::BytesMut;
/// use http_body_util::Empty;
/// use routerama::response::{IntoResponse, Response};
///
/// struct Reply;
///
/// impl IntoResponse for Reply {
///     type Body = Empty<BytesMut>;
///
///     fn into_response(self) -> Response<Self::Body> {
///         Response::new(Empty::new())
///     }
/// }
/// ```
pub trait IntoResponse {
    /// The concrete body retained by this response conversion.
    ///
    /// Generated routers combine the finite set of these body types into one
    /// service-specific sum. Response data is standardized on [`Bytes`] so a
    /// generated body can forward frames and trailers without copying.
    type Body: HttpBody<Data = Bytes>;

    /// Converts this value into a response.
    #[must_use]
    fn into_response(self) -> Response<Self::Body>;
}

/// The metadata being composed around a response body.
///
/// This type deliberately exposes status, version, headers, and extensions,
/// but not the response body. An [`IntoResponseParts`] implementation returns
/// the value on success. On failure, tuple conversion discards this value and
/// the original success body before converting the typed rejection.
#[derive(Debug)]
pub struct ResponseParts {
    inner: http::response::Parts,
}

impl ResponseParts {
    fn new(inner: http::response::Parts) -> Self {
        Self { inner }
    }

    fn into_inner(self) -> http::response::Parts {
        self.inner
    }

    /// Returns the response status.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.inner.status
    }

    /// Returns mutable access to the response status.
    #[must_use]
    pub fn status_mut(&mut self) -> &mut StatusCode {
        &mut self.inner.status
    }

    /// Returns the HTTP version.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.inner.version
    }

    /// Returns mutable access to the HTTP version.
    #[must_use]
    pub fn version_mut(&mut self) -> &mut Version {
        &mut self.inner.version
    }

    /// Returns the response headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderMap {
        &self.inner.headers
    }

    /// Returns mutable access to the response headers.
    #[must_use]
    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.inner.headers
    }

    /// Returns the response extensions.
    #[must_use]
    pub const fn extensions(&self) -> &Extensions {
        &self.inner.extensions
    }

    /// Returns mutable access to the response extensions.
    #[must_use]
    pub fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.inner.extensions
    }
}

/// Applies metadata to response parts, with a typed rejection on failure.
///
/// Implementations should modify and return `response` on success. Tuple
/// conversion owns that value, so a failure cannot expose metadata applied by
/// an earlier part. The rejection is converted independently through
/// [`IntoResponse`] and retains its concrete body.
pub trait IntoResponseParts {
    /// The typed rejection produced when this part cannot be applied.
    type Error: IntoResponse;

    /// Applies this value to `response`.
    ///
    /// # Errors
    ///
    /// Returns the typed rejection without a partially composed success
    /// response when the metadata cannot be applied.
    fn into_response_parts(self, response: ResponseParts) -> Result<ResponseParts, Self::Error>;
}

impl<B> IntoResponse for http::Response<B>
where
    B: HttpBody<Data = Bytes>,
{
    type Body = B;

    fn into_response(self) -> Response<Self::Body> {
        self
    }
}

impl IntoResponse for Body {
    type Body = Self;

    fn into_response(self) -> Response<Self::Body> {
        Response::new(self)
    }
}

impl IntoResponse for BoxBody {
    type Body = Self;

    fn into_response(self) -> Response<Self::Body> {
        Response::new(self)
    }
}

impl IntoResponse for Bytes {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        Body::from(self).into_response()
    }
}

impl IntoResponse for Vec<u8> {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        Body::from(self).into_response()
    }
}

impl IntoResponse for String {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        text_response(Body::from(self))
    }
}

impl IntoResponse for &str {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        text_response(Body::from(self))
    }
}

impl IntoResponse for () {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        Body::empty().into_response()
    }
}

impl IntoResponse for StatusCode {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        let mut response = Body::empty().into_response();
        *response.status_mut() = self;
        response
    }
}

impl IntoResponse for Infallible {
    type Body = NeverBody;

    fn into_response(self) -> Response<Self::Body> {
        match self {}
    }
}

impl<T, E> IntoResponse for Result<T, E>
where
    T: IntoResponse,
    E: IntoResponse,
{
    type Body = EitherBody<T::Body, E::Body>;

    fn into_response(self) -> Response<Self::Body> {
        match self {
            Ok(value) => value.into_response().map(|body| EitherBody::Left { body }),
            Err(error) => error.into_response().map(|body| EitherBody::Right { body }),
        }
    }
}

impl<P, R> IntoResponse for (P, R)
where
    P: IntoResponseParts,
    R: IntoResponse,
{
    type Body = EitherBody<R::Body, <P::Error as IntoResponse>::Body>;

    fn into_response(self) -> Response<Self::Body> {
        let (parts, value) = self;
        let (response_parts, body) = value.into_response().into_parts();
        match parts.into_response_parts(ResponseParts::new(response_parts)) {
            Ok(response_parts) => Response::from_parts(response_parts.into_inner(), EitherBody::Left { body }),
            Err(error) => error.into_response().map(|body| EitherBody::Right { body }),
        }
    }
}

impl<P1, P2, R> IntoResponse for (P1, P2, R)
where
    P1: IntoResponseParts,
    P2: IntoResponseParts,
    R: IntoResponse,
{
    type Body = EitherBody<R::Body, EitherBody<<P2::Error as IntoResponse>::Body, <P1::Error as IntoResponse>::Body>>;

    fn into_response(self) -> Response<Self::Body> {
        let (first, second, value) = self;
        let (response_parts, body) = value.into_response().into_parts();
        let response_parts = match second.into_response_parts(ResponseParts::new(response_parts)) {
            Ok(response_parts) => response_parts,
            Err(error) => {
                return error.into_response().map(|body| EitherBody::Right {
                    body: EitherBody::Left { body },
                });
            }
        };
        match first.into_response_parts(response_parts) {
            Ok(response_parts) => Response::from_parts(response_parts.into_inner(), EitherBody::Left { body }),
            Err(error) => error.into_response().map(|body| EitherBody::Right {
                body: EitherBody::Right { body },
            }),
        }
    }
}

impl IntoResponseParts for StatusCode {
    type Error = Infallible;

    fn into_response_parts(self, mut response: ResponseParts) -> Result<ResponseParts, Self::Error> {
        *response.status_mut() = self;
        Ok(response)
    }
}

impl IntoResponseParts for HeaderMap {
    type Error = Infallible;

    fn into_response_parts(self, mut response: ResponseParts) -> Result<ResponseParts, Self::Error> {
        response.headers_mut().extend(self);
        Ok(response)
    }
}

impl<const N: usize> IntoResponseParts for [(HeaderName, HeaderValue); N] {
    type Error = Infallible;

    fn into_response_parts(self, mut response: ResponseParts) -> Result<ResponseParts, Self::Error> {
        for (name, value) in self {
            response.headers_mut().insert(name, value);
        }
        Ok(response)
    }
}

fn text_response(body: Body) -> Response {
    let mut response = Response::new(body);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    response
}

#[cfg(test)]
mod tests {
    use http::header::LOCATION;
    use http_body_util::BodyExt as _;

    use super::*;

    #[tokio::test]
    async fn text_and_parts_are_composed_without_boxing() {
        let response = (
            StatusCode::CREATED,
            [(LOCATION, HeaderValue::from_static("/books/42"))],
            String::from("created"),
        )
            .into_response();

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()[LOCATION], "/books/42");
        assert_eq!(response.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
        let body = response
            .into_body()
            .collect()
            .await
            .expect("all built-in response and part bodies are infallible")
            .to_bytes();
        assert_eq!(body, b"created"[..]);
    }

    #[test]
    fn response_parts_replace_inner_headers() {
        let response = ([(CONTENT_TYPE, HeaderValue::from_static("application/json"))], String::from("{}")).into_response();

        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        assert_eq!(response.headers().get_all(CONTENT_TYPE).iter().count(), 1);
    }

    #[tokio::test]
    async fn result_uses_each_branch_response() {
        let ok: Result<&str, StatusCode> = Ok("yes");
        let error: Result<&str, StatusCode> = Err(StatusCode::CONFLICT);

        let body = ok
            .into_response()
            .into_body()
            .collect()
            .await
            .expect("both built-in response bodies are infallible")
            .to_bytes();
        assert_eq!(body, b"yes"[..]);
        assert_eq!(error.into_response().status(), StatusCode::CONFLICT);
    }
}
