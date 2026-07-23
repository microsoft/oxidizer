// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Bytes;

use super::into_response::IntoResponse;
use super::{Body, Response};

/// A zero-copy byte response backed by a static byte slice.
///
/// The payload is wrapped with [`Bytes::from_static`] without allocating or
/// copying it. Like [`Bytes`] and [`Vec<u8>`] responses, `StaticBytes` does not
/// set a `Content-Type` header.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "response")] {
/// use http::header::CONTENT_TYPE;
/// use routerama::response::{IntoResponse, StaticBytes};
///
/// let response = StaticBytes(b"\x00\x01").into_response();
///
/// assert!(response.headers().get(CONTENT_TYPE).is_none());
/// assert_eq!(response.body().as_bytes(), b"\x00\x01");
/// # }
/// # }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticBytes(pub &'static [u8]);

impl IntoResponse for StaticBytes {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        Body::from(Bytes::from_static(self.0)).into_response()
    }
}
