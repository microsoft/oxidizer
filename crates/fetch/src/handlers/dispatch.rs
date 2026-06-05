// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::ready;
use std::sync::Arc;

use futures::TryFutureExt;
use futures::future::Either;
use http::uri::Scheme;
use http_extensions::RequestInfo;
use layered::Service;

use crate::handlers::TransportHandler;
use crate::options::{PoolIndex, PoolSelection, RequestFilter};
use crate::{HttpError, HttpRequest, HttpResponse, Result};

/// The final handler responsible for sending HTTP requests to the network.
///
/// `Dispatch` sits at the end of the handler chain and performs:
/// - Final validation of request endpoints
/// - Security filtering based on URL schemes (HTTP vs HTTPS)
/// - Actual network dispatch via the underlying transport
///
/// Think of it as the gateway between your application and the network - all requests
/// must pass through here before hitting the wire.
///
/// # Construction
///
/// `Dispatch` is an internal implementation detail and cannot be created manually.
/// It's instantiated and managed by the `HttpClient` which configures it with the
/// appropriate transport and security settings. Users should interact with the `HttpClient`
/// rather than trying to use this handler directly.
///
/// # Testing
///
/// When the `HttpClient` is created by calling the `HttpClient::new_fake` method, the `Dispatch` doesn't actually send
/// requests to the network. Instead, it delegates the response handling to the `FakeHandler`,
/// which allows for deterministic testing without real network calls. This makes it easy
/// to write tests that verify your code's behavior without relying on external services.
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
        Self::new(
            DispatchMode::single(TransportHandler::new(handler.into())),
            RequestFilter::HttpAndHttps,
        )
    }
}

/// Boxed pool-selection strategy.
///
/// Built from [`PoolSelection::into_selector`]; given the pool of transports it
/// returns the transport to use and its [`PoolIndex`]. The closure owns its
/// selection state (e.g. a round-robin counter), so it is `Send + Sync`.
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
    pub fn single(transport: TransportHandler) -> Self {
        Self::Single(transport)
    }

    pub fn pooled(transports: Vec<TransportHandler>, selection: PoolSelection) -> Self {
        Self::Pooled {
            transports: Arc::from(transports),
            selector: Box::new(selection.into_selector::<TransportHandler>()),
        }
    }
}

impl Service<HttpRequest> for Dispatch {
    type Out = Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        // Preserve the requets info.
        let request_info = input.extensions().get::<RequestInfo>().cloned();

        if let Err(err) = validate(&self.request_filter, &input) {
            return Either::Right(ready(Err(err)));
        }

        // Select the transport synchronously *before* entering the async block.
        // Performing the mode/pool dispatch out here keeps the resulting future
        // small: it only has to carry a single `&TransportHandler`, the inner
        // future, and a couple of `Copy` extension values - instead of the
        // whole `&Dispatch` plus the state of both match arms.
        let transport = match &self.mode {
            DispatchMode::Single(transport) => transport,
            DispatchMode::Pooled { transports, selector } => {
                // If the request carries a `PoolIndex` extension (set by the
                // retry/pooling layer), use it to pin the request to a
                // specific transport in the pool. Otherwise, fall back to the
                // configured selection strategy (e.g. round-robin).
                //
                // The pool index of the actually-selected pool is exposed to
                // callers on the response via
                // [`ConnectionInfo::pool_index`](crate::telemetry::ConnectionInfo::pool_index).
                input
                    .extensions()
                    .get::<PoolIndex>()
                    .and_then(|idx| transports.get(idx.index()))
                    .unwrap_or_else(|| selector(transports).0)
            }
        };

        Either::Left(transport.execute(input).map_ok(move |mut res| {
            // Forward the attempt information to the response if present. This
            // allows inspecting the attempt used to get this response. In
            // healthy scenarios this is always the first attempt, but in
            // degraded scenarios it may be higher.
            if let Some(info) = request_info {
                res.extensions_mut().insert(info);
            }

            res
        }))
    }
}

#[cfg_attr(test, mutants::skip)] // causes test timeouts
fn validate(filter: &RequestFilter, input: &HttpRequest) -> crate::Result<()> {
    // Ensure the request has a scheme and authority set
    let (Some(scheme), Some(authority)) = (input.uri().scheme(), input.uri().authority()) else {
        return Err(HttpError::other(
            "request must have scheme and authority set",
            seatbelt::RecoveryInfo::never(),
            crate::error_labels::LABEL_URI_ORIGIN_MISSING,
        ));
    };

    if !is_allowed(filter, scheme) {
        return Err(HttpError::other(
            format!(
                "unable to communicate with '{scheme}://{authority}', because the '{scheme}' scheme is not allowed by this HTTP client"
            ),
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
        let handler = Dispatch::new(
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
        let handler = Dispatch::new(
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
            "unable to communicate with 'http://dummy.org', because the 'http' scheme is not allowed by this HTTP client"
        );
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn validate_scheme_ensure_https_accepted() {
        let handler = Dispatch::new(
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
        let handler = Dispatch::new(
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
        let handler = Dispatch::new(
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
        let handler = Dispatch::new(
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
        let handler = Dispatch::new(
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
