// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`http_extensions::RequestHandler`] integration for HTTP readiness.

use std::task::Waker;

use bytesbuf::mem::GlobalPool;
use health::Monitor;
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest};
use layered::Execute;
use tick::runtime::InactiveClock;

/// Returns an [`http_extensions::RequestHandler`] serving readiness over `monitor`.
///
/// The application supplies only the monitor. The adapter owns its response body resources.
///
/// # Examples
///
/// This example uses the `test-util` feature of `http_extensions` to build a fake request body.
/// A server passes the incoming request directly to the handler.
///
/// ```
/// # use std::error::Error;
/// #
/// # fn main() -> Result<(), Box<dyn Error>> {
/// # #[cfg(feature = "http-readiness-request")] {
/// use health::Aggregator;
/// use health_probe::http::readiness_request::request_handler;
/// use http::{Request, StatusCode};
/// use http_extensions::HttpBodyBuilder;
/// use layered::Service;
/// use observed::Sink;
///
/// let aggregator = Aggregator::new(Sink::noop());
/// let monitor = aggregator.monitor();
///
/// // Create components and retain their publishers in application state.
///
/// let handler = request_handler(monitor);
/// let response = futures::executor::block_on(
///     handler.execute(Request::new(HttpBodyBuilder::new_fake().empty())),
/// )?;
/// assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
/// # }
/// # Ok(())
/// # }
/// ```
#[cfg_attr(docsrs, doc(cfg(feature = "http-readiness-request")))]
pub fn request_handler(monitor: Monitor) -> impl http_extensions::RequestHandler + Clone + 'static {
    // Text bodies are fully buffered and never consult the clock, so an isolated clock that is
    // never driven is sufficient.
    let (clock, _driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let bodies = HttpBodyBuilder::new(GlobalPool::new(), &clock);
    Execute::new(move |_request: HttpRequest| {
        let response = super::readiness(monitor.state(), |text| bodies.text(text));
        async move { Ok::<_, HttpError>(response) }
    })
}
