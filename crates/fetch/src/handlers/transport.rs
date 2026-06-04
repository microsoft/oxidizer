// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http_extensions::{HttpRequest, HttpResponse, RequestHandler, Result};
use layered::{DynamicService, DynamicServiceExt, Service};

/// Type-erased transport that performs the actual I/O for a request.
///
/// All upper layers in the fetch pipeline (logging, metrics, retries,
/// buffering, ...) eventually delegate to a transport, which turns an
/// [`HttpRequest`] into bytes on the wire and the response bytes back into an
/// [`HttpResponse`]. Concrete transports include HTTP/1.1 or HTTP/2 over plain
/// TCP (`http://`) or TLS (`https://`), and in-process fakes for tests.
#[derive(Debug)]
pub(crate) struct TransportHandler(pub DynamicService<HttpRequest, Result<HttpResponse>>);

impl TransportHandler {
    pub fn new<H: RequestHandler + 'static>(handler: H) -> Self {
        Self(handler.into_dynamic())
    }
}

impl Service<HttpRequest> for TransportHandler {
    type Out = Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        self.0.execute(input)
    }
}
