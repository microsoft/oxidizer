// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An enum that wraps the `TLS` connector, dispatching to the configured backend.

use std::marker::PhantomData;
#[cfg(feature = "rustls")]
use std::sync::Arc;

use http::Version;
use templated_uri::BaseUri;
#[cfg(any(feature = "rustls", feature = "native-tls"))]
use tower::Service as _;

#[cfg(any(feature = "rustls", feature = "native-tls"))]
use crate::connection::hyper_connector_adapter::HyperConnectorAdapter;
use crate::options::RequestFilter;
use crate::tls::TlsBackend;
use crate::{Connect, HyperIo};

/// An enum that wraps the `TLS` connector, dispatching to the correct backend at runtime.
pub(crate) enum TlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    #[cfg(feature = "rustls")]
    Rustls(hyper_rustls::HttpsConnector<HyperConnectorAdapter<C, S>>, PhantomData<fn() -> S>),

    #[cfg(feature = "native-tls")]
    NativeTls(hyper_tls::HttpsConnector<HyperConnectorAdapter<C, S>>, PhantomData<fn() -> S>),

    #[cfg(not(any(feature = "rustls", feature = "native-tls")))]
    None(PhantomData<fn(C, S)>),
}

impl<C, S> Clone for TlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    fn clone(&self) -> Self {
        match self {
            #[cfg(feature = "rustls")]
            Self::Rustls(c, _) => Self::Rustls(c.clone(), PhantomData),
            #[cfg(feature = "native-tls")]
            Self::NativeTls(c, _) => Self::NativeTls(c.clone(), PhantomData),
            #[cfg(not(any(feature = "rustls", feature = "native-tls")))]
            Self::None(_) => Self::None(PhantomData),
        }
    }
}

impl<C, S> TlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    #[allow(unreachable_patterns, unused_variables)]
    pub(crate) fn new(backend: TlsBackend, connector: C, request_filter: &RequestFilter, supported_versions: &[Version]) -> Self {
        match backend {
            #[cfg(feature = "rustls")]
            TlsBackend::Rustls(config) => Self::Rustls(
                build_rustls_connector(config, connector, request_filter, supported_versions),
                PhantomData,
            ),
            #[cfg(feature = "native-tls")]
            TlsBackend::NativeTls(native) => Self::NativeTls(build_native_tls_connector(native, connector, request_filter), PhantomData),
            #[cfg(not(any(feature = "rustls", feature = "native-tls")))]
            _ => {
                let _ = (connector, request_filter);
                // `TlsBackend` is uninhabited when no TLS feature is enabled, so this match is exhaustive.
                match backend {}
            }
        }
    }
}

#[cfg(feature = "rustls")]
fn build_rustls_connector<C, S>(
    config: Arc<rustls::ClientConfig>,
    connector: C,
    request_filter: &RequestFilter,
    supported_versions: &[Version],
) -> hyper_rustls::HttpsConnector<HyperConnectorAdapter<C, S>>
where
    C: Connect<S>,
    S: HyperIo,
{
    let config = Arc::unwrap_or_clone(config);
    let builder = hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(config);

    let builder = match request_filter {
        RequestFilter::Https => builder.https_only(),
        RequestFilter::HttpAndHttps => builder.https_or_http(),
    };

    let inner = HyperConnectorAdapter::new(connector);

    let http1 = supported_versions.contains(&Version::HTTP_11) || supported_versions.contains(&Version::HTTP_10);
    let http2 = supported_versions.contains(&Version::HTTP_2);

    if http1 && http2 {
        builder.enable_http1().enable_http2().wrap_connector(inner)
    } else if http2 {
        builder.enable_http2().wrap_connector(inner)
    } else {
        builder.enable_http1().wrap_connector(inner)
    }
}

#[cfg(feature = "native-tls")]
fn build_native_tls_connector<C, S>(
    native: native_tls::TlsConnector,
    connector: C,
    request_filter: &RequestFilter,
) -> hyper_tls::HttpsConnector<HyperConnectorAdapter<C, S>>
where
    C: Connect<S>,
    S: HyperIo,
{
    let tokio_connector = tokio_native_tls::TlsConnector::from(native);
    let inner = HyperConnectorAdapter::new(connector);
    let mut https = hyper_tls::HttpsConnector::from((inner, tokio_connector));

    https.https_only(matches!(request_filter, RequestFilter::Https));

    https
}

impl<C, S> layered::Service<BaseUri> for TlsConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    type Out = http_extensions::Result<Box<dyn HyperIo>>;

    async fn execute(&self, input: BaseUri) -> Self::Out {
        match self {
            #[cfg(feature = "rustls")]
            Self::Rustls(c, _) => {
                let mut c = c.clone();
                std::future::poll_fn(|cx| c.poll_ready(cx)).await.map_err(handle_tls_error)?;
                c.call(input.into())
                    .await
                    .map(|s| Box::new(s) as Box<dyn HyperIo>)
                    .map_err(handle_tls_error)
            }
            #[cfg(feature = "native-tls")]
            Self::NativeTls(c, _) => {
                let mut c = c.clone();
                std::future::poll_fn(|cx| c.poll_ready(cx)).await.map_err(handle_tls_error)?;
                c.call(input.into())
                    .await
                    .map(|s| Box::new(s) as Box<dyn HyperIo>)
                    .map_err(handle_tls_error)
            }
            #[cfg(not(any(feature = "rustls", feature = "native-tls")))]
            Self::None(_) => {
                let _ = input;
                unreachable!(
                    "`TlsConnector::None` cannot be constructed because `TlsBackend` is uninhabited when no TLS feature is enabled"
                )
            }
        }
    }
}

#[cfg(any(feature = "rustls", feature = "native-tls"))]
fn handle_tls_error(e: Box<dyn std::error::Error + Send + Sync>) -> http_extensions::HttpError {
    let recovery = crate::recoverability::detect_recoverability(e.as_ref());
    http_extensions::HttpError::other(e, recovery, crate::error_labels::LABEL_CONNECT)
}
