// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Internal generic [`HyperHandler`] driving hyper-util's `legacy::Client`.
//!
//! Implements [`Service<HttpRequest>`]. Type-erased into
//! [`HyperTransport`](crate::HyperTransport) by
//! [`HyperTransportBuilder::build`](crate::HyperTransportBuilder::build).

use std::error::Error;
use std::fmt::{self, Display};

use bytesbuf::BytesView;
use futures::TryFutureExt;
use http::{Extensions, Version};
use http_body_util::BodyExt;
use http_extensions::timeout::BodyTimeout;
use http_extensions::{HttpBody, HttpBodyOptions, HttpError, HttpRequest, HttpResponse, Result};
use hyper_util::client::legacy::connect::{CaptureConnection, capture_connection};
use hyper_util::client::legacy::{self, Client};
use layered::Service;
use opentelemetry::metrics::Meter;

use crate::builder::{HyperTransportBuilder, SpawnerExecutor};
use crate::connection::client_connector::ClientConnector;
use crate::connection::connect::Connect;
use crate::connection::hyper_connector_adapter::HyperConnectorAdapter;
use crate::connection::io::HyperIo;
use crate::connection::tracked_stream::TrackedStream;
use crate::error_labels::LABEL_REQUEST_HYPER;
use crate::recoverability::detect_recoverability;
use crate::telemetry::ConnectionInfo;
use crate::timer::ClockTimer;
use crate::tls::TlsConnector;

/// The fully-wrapped connector chain handed to `hyper`'s [`Client`].
type WrappedConnector<C, S> = HyperConnectorAdapter<ClientConnector<TlsConnector<C, S>, Box<dyn HyperIo>>, TrackedStream<Box<dyn HyperIo>>>;

/// A Hyper-backed request handler, parameterized by the user-supplied
/// connector and stream types. Public consumers see only the
/// type-erased [`HyperTransport`](crate::HyperTransport).
pub(crate) struct HyperHandler<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    client: Client<WrappedConnector<C, S>, HttpBody>,
    body_builder: http_extensions::HttpBodyBuilder,
}

impl<C, S> fmt::Debug for HyperHandler<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>()).finish_non_exhaustive()
    }
}

impl<C, S> Service<HttpRequest> for HyperHandler<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    type Out = Result<HttpResponse>;

    fn execute(&self, mut input: HttpRequest) -> impl Future<Output = Result<HttpResponse>> + Send {
        let captured = capture_connection::<HttpBody>(&mut input);

        let body_options = input
            .extensions()
            .get::<BodyTimeout>()
            .map(|v| HttpBodyOptions::default().timeout(v.duration()))
            .unwrap_or_default();

        self.client
            .request(input)
            .map_err(create_http_error_from_hyper_util)
            .map_ok(move |res| {
                let (parts, body) = res.into_parts();

                let body = body
                    .map_frame(|f| f.map_data(BytesView::from))
                    .map_err(create_http_error_from_hyper);

                handle_poisoning(&captured, &parts.extensions);

                HttpResponse::from_parts(parts, self.body_builder.body(body, &body_options))
            })
    }
}

/// Assembles a [`HyperHandler`] from a configured [`HyperTransportBuilder`].
pub(crate) fn build_hyper_handler<C, S>(builder: HyperTransportBuilder<C, S>, meter: &Meter) -> HyperHandler<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    let HyperTransportBuilder {
        connector,
        spawner,
        clock,
        tls,
        body_builder,
        request_filter,
        supported_http_versions,
        connection_lifetime,
        connect_timeout,
        pool_index,
        configure_hyper,
        ..
    } = builder;

    let timer = ClockTimer::new(clock.clone());
    let mut hyper_builder = legacy::Client::builder(SpawnerExecutor(spawner));

    hyper_builder.timer(timer.clone()).pool_timer(timer);

    if supported_http_versions.len() == 1 && supported_http_versions[0] == Version::HTTP_2 {
        hyper_builder.http2_only(true);
    }

    if let Some(configure) = configure_hyper {
        configure(&mut hyper_builder);
    }

    let tls_connector = TlsConnector::new(tls, connector, &request_filter, &supported_http_versions);

    let inner = ClientConnector::new(
        tls_connector,
        clock,
        connect_timeout,
        supported_http_versions,
        meter,
        pool_index,
        connection_lifetime,
    );

    HyperHandler {
        client: hyper_builder.build(HyperConnectorAdapter::new(inner)),
        body_builder,
    }
}

fn create_http_error_from_hyper_util(error: legacy::Error) -> HttpError {
    let recovery = detect_recoverability(&error);
    HttpError::other(HyperError::Legacy(error), recovery, LABEL_REQUEST_HYPER)
}

fn create_http_error_from_hyper(error: hyper::Error) -> HttpError {
    let recovery = detect_recoverability(&error);
    HttpError::other(HyperError::Hyper(error), recovery, LABEL_REQUEST_HYPER)
}

#[derive(Debug)]
enum HyperError {
    Legacy(legacy::Error),
    Hyper(hyper::Error),
}

impl Error for HyperError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Legacy(e) => Some(e),
            Self::Hyper(e) => Some(e),
        }
    }
}

impl Display for HyperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Legacy(error) => write!(f, "{error}")?,
            Self::Hyper(error) => write!(f, "{error}")?,
        }

        let mut current: Option<&(dyn Error + 'static)> = self.source();
        while let Some(source) = current {
            write!(f, "\ncaused by: {source}")?;
            current = source.source();
        }

        Ok(())
    }
}

fn handle_poisoning(capture: &CaptureConnection, extensions: &Extensions) {
    if let Some(info) = extensions.get::<ConnectionInfo>()
        && info.is_expired()
        && let Some(connected) = capture.connection_metadata().as_ref()
    {
        connected.poison();
        info.mark_poisoned();
    }
}
