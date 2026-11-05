// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::ready;

use futures::FutureExt as _;
use futures::future::Either;
use http::uri::Scheme;
use layered::Service;
use performables::arc::Arc;
use seatbelt::retry::Attempt;

use crate::handlers::TransportHandler;
use crate::options::{PoolIndex, PoolSelection, RequestFilter};
use crate::{HttpError, HttpRequest, HttpResponse, Result};

/// Sends HTTP requests through the configured transport.
///
/// `Dispatch` is the final handler. It:
///
/// - Validates request endpoints.
/// - Applies the HTTP and HTTPS scheme policy.
/// - Passes the request to the transport.
///
/// # Construction
///
/// `HttpClient` creates and configures `Dispatch`. Callers cannot create it directly.
///
/// # Testing
///
/// A client from `HttpClient::new_fake` sends requests to `FakeHandler` instead of the network.
#[derive(Debug)]
pub struct Dispatch {
    pub(crate) mode: DispatchMode,
    request_filter: RequestFilter,
}

impl Dispatch {
    pub(crate) fn new(mode: DispatchMode, request_filter: RequestFilter) -> Self {
        Self { mode, request_filter }
    }

    #[cfg(test)]
    pub(crate) fn new_fake(handler: impl Into<http_extensions::FakeHandler>) -> Self {
        Self::new_test(
            DispatchMode::single(TransportHandler::new(handler.into())),
            RequestFilter::HttpAndHttps,
        )
    }

    /// Test-only constructor for a dispatch mode assembled by the caller.
    #[cfg(test)]
    pub(crate) fn new_test(mode: DispatchMode, request_filter: RequestFilter) -> Self {
        Self::new(mode, request_filter)
    }
}

/// Boxed pool-selection strategy.
///
/// Built from [`PoolSelection::into_selector`]. It selects a transport and returns its
/// [`PoolIndex`]. The closure owns state such as a round-robin counter, so it is `Send + Sync`.
type PoolSelector = dyn for<'a> Fn(&'a [TransportHandler]) -> (&'a TransportHandler, PoolIndex) + Send + Sync;

pub(crate) enum DispatchMode {
    Single(TransportHandler),
    Pooled {
        transports: Arc<[TransportHandler]>,
        selector: Box<PoolSelector>,
    },
}

impl std::fmt::Debug for DispatchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Single(transport) => f.debug_tuple("Single").field(transport).finish(),
            Self::Pooled { transports, .. } => f.debug_struct("Pooled").field("transports", transports).finish_non_exhaustive(),
        }
    }
}

impl DispatchMode {
    pub(crate) fn single(transport: TransportHandler) -> Self {
        Self::Single(transport)
    }

    pub(crate) fn pooled(transports: Vec<TransportHandler>, selection: PoolSelection) -> Self {
        Self::Pooled {
            transports: Arc::from(transports),
            selector: Box::new(selection.into_selector::<TransportHandler>()),
        }
    }
}

impl Service<HttpRequest> for Dispatch {
    type Out = Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        // Copy attempt information from the request to the response.
        let attempt = input.extensions().get::<Attempt>().copied();

        if let Err(err) = validate(&self.request_filter, &input) {
            return Either::Right(ready(Err(err)));
        }

        // Select the transport before the async block. The future then stores one transport, the
        // inner future, and two Copy extension values instead of Dispatch and both match arms.
        let transport = match &self.mode {
            DispatchMode::Single(transport) => transport,
            DispatchMode::Pooled { transports, selector } => {
                // A PoolIndex from retry or pooling selects one transport. Without it, use the
                // configured strategy, such as round-robin.
                //
                // Expose the selected pool index through
                // [`ConnectionInfo::pool_index`](crate::telemetry::ConnectionInfo::pool_index).
                input
                    .extensions()
                    .get::<PoolIndex>()
                    .and_then(|idx| transports.get(idx.index()))
                    .unwrap_or_else(|| selector(transports).0)
            }
        };

        Either::Left(transport.execute(input).map(move |result| {
            let mut res = result?;

            // Copy the attempt number to the response. A retry or hedge can increase it.
            if let Some(attempt) = attempt {
                res.extensions_mut().insert(attempt);
            }

            Ok(res)
        }))
    }
}

#[cfg_attr(test, mutants::skip)] // causes test timeouts
fn validate(filter: &RequestFilter, input: &HttpRequest) -> crate::Result<()> {
    // Require a scheme and authority.
    let (Some(scheme), Some(authority)) = (input.uri().scheme(), input.uri().authority()) else {
        return Err(HttpError::other(
            "request must have scheme and authority set",
            seatbelt::RecoveryInfo::never(),
            crate::error_labels::LABEL_URI_ORIGIN_MISSING,
        ));
    };

    if !is_allowed(filter, scheme) {
        return Err(HttpError::other(
            format!("cannot send a request to '{scheme}://{authority}' because this HTTP client does not allow the '{scheme}' scheme"),
            seatbelt::RecoveryInfo::never(),
            crate::error_labels::LABEL_SCHEME_NOT_ALLOWED,
        ));
    }

    Ok(())
}

fn is_allowed(filter: &RequestFilter, scheme: &Scheme) -> bool {
    match filter {
        RequestFilter::Https => scheme == &Scheme::HTTPS,
        RequestFilter::HttpAndHttps => true,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http::{Request, StatusCode, Uri};
    use http_extensions::FakeHandler;
    use ohno::ErrorExt;
    use seatbelt::{Recovery, RecoveryKind};

    use super::*;
    use crate::HttpBodyBuilder;
    use crate::error_labels::collect_error_labels;

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn no_endpoint_error() {
        let handler = Dispatch::new_test(
            DispatchMode::single(TransportHandler::new(FakeHandler::never_completes())),
            RequestFilter::Https,
        );

        let uri = Uri::from_static("/relative-path");
        let request = Request::get(uri).body(HttpBodyBuilder::new_fake().empty()).unwrap();

        let error = handler.execute(request).await.unwrap_err();

        assert_eq!(error.recovery().kind(), RecoveryKind::Never);
        assert_eq!(collect_error_labels(&error), "uri_origin_missing");
        assert_eq!(error.message(), "request must have scheme and authority set");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn validate_scheme_ensure_http_rejected() {
        let handler = Dispatch::new_test(
            DispatchMode::single(TransportHandler::new(FakeHandler::from_status_codes([StatusCode::OK]))),
            RequestFilter::Https,
        );

        let request = Request::get(Uri::from_static("http://dummy.org/relative-path"))
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        let error = handler.execute(request).await.unwrap_err();

        assert_eq!(error.recovery().kind(), RecoveryKind::Never);
        assert_eq!(collect_error_labels(&error), "scheme_not_allowed");
        assert_eq!(
            error.message(),
            "cannot send a request to 'http://dummy.org' because this HTTP client does not allow the 'http' scheme"
        );
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn validate_scheme_ensure_https_accepted() {
        let handler = Dispatch::new_test(
            DispatchMode::single(TransportHandler::new(FakeHandler::default())),
            RequestFilter::Https,
        );

        let request = Request::get(Uri::from_static("https://dummy.org/relative-path"))
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        let _result = handler.execute(request).await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn validate_scheme_ensure_http_accepted() {
        let handler = Dispatch::new_test(
            DispatchMode::single(TransportHandler::new(FakeHandler::default())),
            RequestFilter::HttpAndHttps,
        );

        let request = Request::get(Uri::from_static("http://dummy.org/relative-path"))
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        let _result = handler.execute(request).await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn forward_attempt_number() {
        let handler = Dispatch::new_test(
            DispatchMode::single(TransportHandler::new(FakeHandler::from(StatusCode::OK))),
            RequestFilter::HttpAndHttps,
        );

        let request = Request::get(Uri::from_static("http://dummy.org/relative-path"))
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        let response = handler.execute(request).await.unwrap();
        assert!(response.extensions().get::<Attempt>().is_none());

        let mut request = Request::get(Uri::from_static("http://dummy.org/relative-path"))
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        request.extensions_mut().insert(Attempt::new(4, false));

        let response = handler.execute(request).await.unwrap();
        assert_eq!(response.extensions().get::<Attempt>().copied().unwrap(), Attempt::new(4, false));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn pool_index_selects_specific_pool() {
        let handler = Dispatch::new_test(
            DispatchMode::pooled(
                vec![
                    TransportHandler::new(FakeHandler::from(StatusCode::OK)),
                    TransportHandler::new(FakeHandler::from(StatusCode::ACCEPTED)),
                ],
                PoolSelection::saturating(PoolSelection::DEFAULT_REQUESTS_PER_CLIENT),
            ),
            RequestFilter::HttpAndHttps,
        );

        let mut request = Request::get(Uri::from_static("http://dummy.org/path"))
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        request.extensions_mut().insert(PoolIndex::new(1));

        let response = handler.execute(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn pool_index_out_of_bounds_falls_back_to_strategy() {
        let handler = Dispatch::new_test(
            DispatchMode::pooled(
                vec![
                    TransportHandler::new(FakeHandler::from(StatusCode::OK)),
                    TransportHandler::new(FakeHandler::from(StatusCode::ACCEPTED)),
                ],
                PoolSelection::saturating(PoolSelection::DEFAULT_REQUESTS_PER_CLIENT),
            ),
            RequestFilter::HttpAndHttps,
        );

        let mut request = Request::get(Uri::from_static("http://dummy.org/path"))
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        request.extensions_mut().insert(PoolIndex::new(99));

        // Out-of-bounds falls back to strategy, which selects index 0 first
        let response = handler.execute(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn pooled_dispatch_mode_has_debug_representation() {
        let mode = DispatchMode::pooled(
            vec![
                TransportHandler::new(FakeHandler::from(StatusCode::OK)),
                TransportHandler::new(FakeHandler::from(StatusCode::ACCEPTED)),
            ],
            PoolSelection::saturating(PoolSelection::DEFAULT_REQUESTS_PER_CLIENT),
        );

        insta::assert_debug_snapshot!(mode);
    }
}
