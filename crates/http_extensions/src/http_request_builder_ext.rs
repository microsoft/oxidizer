// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{HttpBodyBuilder, HttpRequestBuilder, RequestHandler};

/// Extension trait for types that implement `RequestHandler` and `AsRef<HttpBodyBuilder>`.
pub trait HttpRequestBuilderExt: RequestHandler
where
    Self: Sized,
{
    /// Creates a new HTTP request builder associated with this handler.
    fn request_builder(&self) -> HttpRequestBuilder<'_, Self>;
}

impl<T> HttpRequestBuilderExt for T
where
    T: RequestHandler + AsRef<HttpBodyBuilder>,
{
    fn request_builder(&self) -> HttpRequestBuilder<'_, Self> {
        HttpRequestBuilder::with_request_handler(self, self.as_ref())
    }
}
