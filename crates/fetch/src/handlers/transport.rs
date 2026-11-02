// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http_extensions::{HttpRequest, HttpResponse, RequestHandler, Result};
use layered::{DynamicService, DynamicServiceExt, Service};

/// Type-erased transport that performs request I/O.
///
/// Logging, metrics, retries, and buffering delegate to a transport. A transport converts an
/// [`HttpRequest`] into bytes and response bytes into an [`HttpResponse`]. Transports can use
/// HTTP/1.1 or HTTP/2 over TCP (`http://`) or TLS (`https://`). Tests can use an in-process fake.
#[derive(Debug)]
pub(crate) struct TransportHandler(pub DynamicService<HttpRequest, Result<HttpResponse>>);

impl TransportHandler {
    pub(crate) fn new<H: RequestHandler + 'static>(handler: H) -> Self {
        Self(handler.into_dynamic())
    }
}

impl Service<HttpRequest> for TransportHandler {
    type Out = Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        self.0.execute(input)
    }
}
