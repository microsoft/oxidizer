// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The connection events of the Hyper transport.

use hyper_util::client::legacy::connect::Connected;
use observed::{Text, event};
use templated_uri::BaseUri;

/// The bucket boundaries of `http.client.connection.setup.duration`, in seconds.
const SETUP_DURATION_BOUNDARIES: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0, 25.0, 50.0,
];

/// The bucket boundaries of `http.client.connection.duration`, in seconds. The OpenTelemetry HTTP metrics conventions
/// give these boundaries.
const CONNECTION_DURATION_BOUNDARIES: &[f64] = &[0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0];

/// The runtime, the transport and the name of the client that the transport serves.
#[derive(Clone, Debug)]
pub(crate) struct ClientAttributes {
    pub(crate) runtime: Text,
    pub(crate) transport: Text,
    pub(crate) client_name: Text,
}

/// Produces the event `http.client.connection.start` before the connector runs.
#[event("http.client.connection.start")]
#[debug("connecting to a remote endpoint")]
pub(crate) struct ConnectionStart<'a> {
    #[dimension(log = "server.address")]
    #[unredacted]
    pub(crate) server_address: &'a str,
    #[dimension(log = "server.port")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_port: Option<u16>,
    #[dimension(log = "url.scheme")]
    #[unredacted]
    pub(crate) scheme: &'a str,
    #[dimension(log = "url.full")]
    pub(crate) url_full: &'a BaseUri,
    #[dimension(log = "fetch.runtime")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) runtime: Option<Text>,
    #[dimension(log = "fetch.transport")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) transport: Option<Text>,
    #[dimension(log = "http.client.name")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) client_name: Option<Text>,
}

/// Produces the event `http.client.connection.success` when the connector returned a connection before the connect
/// timeout.
#[event("http.client.connection.success")]
#[info("connected to server")]
#[histogram(
    setup_duration_secs,
    name = "http.client.connection.setup.duration",
    desc = "The duration of setting up the connection.",
    unit = "s",
    boundaries = SETUP_DURATION_BOUNDARIES
)]
pub(crate) struct ConnectionSuccess<'a> {
    #[dimension(log = "server.address", metric = "server.address")]
    #[unredacted]
    pub(crate) server_address: &'a str,
    #[dimension(log = "server.port", metric = "server.port")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_port: Option<u16>,
    #[dimension(log = "url.scheme", metric = "url.scheme")]
    #[unredacted]
    pub(crate) scheme: &'a str,
    #[dimension(log = "url.full")]
    pub(crate) url_full: &'a BaseUri,
    #[dimension(log = "network.protocol.version", metric = "network.protocol.version")]
    #[unredacted]
    pub(crate) protocol_version: &'static str,
    #[dimension(log = "http.client.connection.setup.duration")]
    #[unredacted]
    pub(crate) setup_duration_secs: f64,
    #[dimension(log = "fetch.runtime", metric = "fetch.runtime")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) runtime: Option<Text>,
    #[dimension(log = "fetch.transport", metric = "fetch.transport")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) transport: Option<Text>,
    #[dimension(log = "http.client.name", metric = "http.client.name")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) client_name: Option<Text>,
}

/// Produces the event `http.client.connection.error` when the connector returned an error, or when the connect timeout
/// passed.
#[event("http.client.connection.error")]
#[warning("server connection failed")]
#[histogram(
    setup_duration_secs,
    name = "http.client.connection.setup.duration",
    desc = "The duration of setting up the connection.",
    unit = "s",
    boundaries = SETUP_DURATION_BOUNDARIES
)]
pub(crate) struct ConnectionError<'a> {
    #[dimension(log = "server.address", metric = "server.address")]
    #[unredacted]
    pub(crate) server_address: &'a str,
    #[dimension(log = "server.port", metric = "server.port")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_port: Option<u16>,
    #[dimension(log = "url.scheme", metric = "url.scheme")]
    #[unredacted]
    pub(crate) scheme: &'a str,
    #[dimension(log = "url.full")]
    pub(crate) url_full: &'a BaseUri,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    pub(crate) error_type: Text,
    #[dimension(log = "exception.message")]
    #[unredacted]
    pub(crate) exception_message: Text,
    #[dimension(log = "http.client.connection.setup.duration")]
    #[unredacted]
    pub(crate) setup_duration_secs: f64,
    #[dimension(log = "fetch.runtime", metric = "fetch.runtime")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) runtime: Option<Text>,
    #[dimension(log = "fetch.transport", metric = "fetch.transport")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) transport: Option<Text>,
    #[dimension(log = "http.client.name", metric = "http.client.name")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) client_name: Option<Text>,
}

/// Produces the event `http.client.connection.closed` when the stream of a connection is dropped.
#[event("http.client.connection.closed")]
#[debug("connection closed")]
#[histogram(
    duration_secs,
    name = "http.client.connection.duration",
    desc = "The total duration of the connection from establishment to close.",
    unit = "s",
    boundaries = CONNECTION_DURATION_BOUNDARIES
)]
pub(crate) struct ConnectionClosed<'a> {
    #[dimension(log = "server.address", metric = "server.address")]
    #[unredacted]
    pub(crate) server_address: &'a str,
    #[dimension(log = "server.port", metric = "server.port")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_port: Option<u16>,
    #[dimension(log = "url.scheme", metric = "url.scheme")]
    #[unredacted]
    pub(crate) scheme: &'a str,
    #[dimension(log = "network.protocol.version", metric = "network.protocol.version")]
    #[unredacted]
    pub(crate) protocol_version: &'static str,
    #[dimension(log = "http.client.connection.duration")]
    #[unredacted]
    pub(crate) duration_secs: f64,
    #[dimension(log = "fetch.runtime", metric = "fetch.runtime")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) runtime: Option<Text>,
    #[dimension(log = "fetch.transport", metric = "fetch.transport")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) transport: Option<Text>,
    #[dimension(log = "http.client.name", metric = "http.client.name")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) client_name: Option<Text>,
}

/// The value of `network.protocol.version` for a connection. A connection knows only whether ALPN chose HTTP/2.
pub(crate) fn protocol_version(connected: &Connected) -> &'static str {
    if connected.is_negotiated_h2() { "2" } else { "1" }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::ops::ControlFlow;

    use observed::VisitFields;
    use observed::metadata::{FieldDescriptor, InstrumentKind};
    use observed::processing::FieldValueFn;

    use super::*;

    /// The name, the kind, the description, the unit and the bucket boundaries of an instrument.
    type Instrument = (&'static str, InstrumentKind, &'static str, &'static str, &'static [f64]);

    const SETUP_DURATION: Instrument = (
        "http.client.connection.setup.duration",
        InstrumentKind::Histogram,
        "The duration of setting up the connection.",
        "s",
        &[
            0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0, 25.0, 50.0,
        ],
    );

    const CONNECTION_DURATION: Instrument = (
        "http.client.connection.duration",
        InstrumentKind::Histogram,
        "The total duration of the connection from establishment to close.",
        "s",
        &[0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0],
    );

    #[test]
    fn metric_dimensions_and_instruments_of_each_event() {
        let base_uri = BaseUri::from_static("https://example.test");
        let start = ConnectionStart {
            server_address: "example.test",
            server_port: Some(443),
            scheme: "https",
            url_full: &base_uri,
            runtime: Some(Text::from("tokio")),
            transport: Some(Text::from("hyper")),
            client_name: Some(Text::from("books")),
        };
        let success = ConnectionSuccess {
            server_address: "example.test",
            server_port: Some(443),
            scheme: "https",
            url_full: &base_uri,
            protocol_version: "2",
            setup_duration_secs: 0.0,
            runtime: Some(Text::from("tokio")),
            transport: Some(Text::from("hyper")),
            client_name: Some(Text::from("books")),
        };
        let error = ConnectionError {
            server_address: "example.test",
            server_port: Some(443),
            scheme: "https",
            url_full: &base_uri,
            error_type: Text::from("connect"),
            exception_message: Text::from("connection refused"),
            setup_duration_secs: 0.0,
            runtime: Some(Text::from("tokio")),
            transport: Some(Text::from("hyper")),
            client_name: Some(Text::from("books")),
        };
        let closed = ConnectionClosed {
            server_address: "example.test",
            server_port: Some(443),
            scheme: "https",
            protocol_version: "2",
            duration_secs: 0.0,
            runtime: Some(Text::from("tokio")),
            transport: Some(Text::from("hyper")),
            client_name: Some(Text::from("books")),
        };

        assert_eq!(metric_fields(&start), (vec![], vec![]));
        assert_eq!(
            metric_fields(&success),
            (
                vec![
                    "server.address",
                    "server.port",
                    "url.scheme",
                    "network.protocol.version",
                    "fetch.runtime",
                    "fetch.transport",
                    "http.client.name"
                ],
                vec![SETUP_DURATION]
            )
        );
        assert_eq!(
            metric_fields(&error),
            (
                vec![
                    "server.address",
                    "server.port",
                    "url.scheme",
                    "error.type",
                    "fetch.runtime",
                    "fetch.transport",
                    "http.client.name"
                ],
                vec![SETUP_DURATION]
            )
        );
        assert_eq!(
            metric_fields(&closed),
            (
                vec![
                    "server.address",
                    "server.port",
                    "url.scheme",
                    "network.protocol.version",
                    "fetch.runtime",
                    "fetch.transport",
                    "http.client.name"
                ],
                vec![CONNECTION_DURATION]
            )
        );
    }

    /// Returns the keys of the fields of `event` that are metric dimensions, and the instruments that its fields
    /// record.
    fn metric_fields(event: &impl VisitFields) -> (Vec<&'static str>, Vec<Instrument>) {
        let mut dimensions = Vec::new();
        let mut instruments = Vec::new();
        let _ = event.visit_fields(&mut |field: &FieldDescriptor, _: &FieldValueFn<'_>| {
            if let Some(metric) = field.metric() {
                match metric.instrument_description() {
                    Some(instrument) => instruments.push((
                        instrument.instrument_name(),
                        instrument.kind(),
                        instrument.description(),
                        instrument.unit(),
                        instrument.boundaries(),
                    )),
                    None => dimensions.push(metric.key()),
                }
            }
            ControlFlow::Continue(())
        });
        (dimensions, instruments)
    }
}
