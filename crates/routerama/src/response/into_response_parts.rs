// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::convert::Infallible;

use http::header::{HeaderName, HeaderValue};
use http::{HeaderMap, StatusCode};

use super::into_response::IntoResponse;
use super::response_parts::ResponseParts;

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
