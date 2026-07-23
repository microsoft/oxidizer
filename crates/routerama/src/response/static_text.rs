// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Bytes;

use super::into_response::{IntoResponse, text_response};
use super::{Body, Response};

/// A zero-copy text response backed by a static string.
///
/// The payload is wrapped with [`Bytes::from_static`] without allocating or
/// copying it. Like [`String`] and `&str` responses, `StaticText` sets
/// `Content-Type` to `text/plain; charset=utf-8`.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "response")] {
/// use http::header::CONTENT_TYPE;
/// use routerama::response::{IntoResponse, StaticText};
///
/// let response = StaticText("ready").into_response();
///
/// assert_eq!(
///     response.headers()[CONTENT_TYPE],
///     "text/plain; charset=utf-8"
/// );
/// assert_eq!(response.body().as_bytes(), b"ready");
/// # }
/// # }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticText(pub &'static str);

impl IntoResponse for StaticText {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        text_response(Body::from(Bytes::from_static(self.0.as_bytes())))
    }
}
