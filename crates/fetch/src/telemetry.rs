// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Types for the request telemetry and the connection diagnostics of `fetch`.
//!
//! `fetch` emits its request telemetry as [`http_telemetry`] client events through the sink of its
//! deps. [`Telemetry`][crate::telemetry::Telemetry], [`TelemetryLayer`][crate::telemetry::TelemetryLayer]
//! and [`Client`][crate::telemetry::Client] build the layer of those events. A custom
//! pipeline gets the layer of its client from
//! [`PipelineContext::client_telemetry`](crate::pipeline::PipelineContext::client_telemetry).
//! [`ConnectionInfo`][crate::telemetry::ConnectionInfo] describes the connection that served a
//! response.
//!
//! For the full list of emitted metrics and events, see the
//! [telemetry reference](crate::documentation::telemetry).

use std::borrow::Cow;
use std::fmt;

/// Diagnostic information about the connection that served an HTTP response.
///
/// Re-exported from [`fetch_options`]. The bundled Hyper transport adds it as a response extension.
/// A custom transport must add it if it supports this diagnostic.
/// [`FakeHandler`](crate::fake::FakeHandler) does not add it. Read it through
/// `response.extensions().get::<ConnectionInfo>()`.
pub use fetch_options::ConnectionInfo;
pub use http_telemetry::{Client, Telemetry, TelemetryLayer};
use opentelemetry::metrics::{Meter, MeterProvider};
use opentelemetry::{InstrumentationScope, KeyValue};
use performables::arc::Arc;

pub(crate) const METER_NAME: &str = "fetch";

/// Key of the runtime of a client, in the scope of the `fetch` meter and in each request event.
pub(crate) const FETCH_RUNTIME_ATTRIBUTE: &str = "fetch.runtime";

/// Key of the transport of a client, in the scope of the `fetch` meter and in each request event.
pub(crate) const FETCH_TRANSPORT_ATTRIBUTE: &str = "fetch.transport";

/// Key of the name of a client, in the scope of the `fetch` meter and in each request event.
pub(crate) const HTTP_CLIENT_NAME_ATTRIBUTE: &str = "http.client.name";

/// The runtime, the transport and the name that tell which client sent a request.
///
/// The scope of the `fetch` meter and the request events of the client carry these values.
#[derive(Debug, Clone)]
pub(crate) struct ClientAttributes {
    runtime: Cow<'static, str>,
    transport: Cow<'static, str>,
    client_name: Cow<'static, str>,
}

impl ClientAttributes {
    pub(crate) fn new(runtime: Cow<'static, str>, transport: Cow<'static, str>, client_name: Cow<'static, str>) -> Self {
        Self {
            runtime,
            transport,
            client_name,
        }
    }

    pub(crate) fn with_client_name(mut self, client_name: Cow<'static, str>) -> Self {
        self.client_name = client_name;
        self
    }

    /// Returns the scope of the `fetch` meter, with the three values as attributes.
    pub(crate) fn scope(&self) -> InstrumentationScope {
        InstrumentationScope::builder(METER_NAME)
            .with_attributes([
                KeyValue::new(FETCH_RUNTIME_ATTRIBUTE, self.runtime.clone()),
                KeyValue::new(FETCH_TRANSPORT_ATTRIBUTE, self.transport.clone()),
                KeyValue::new(HTTP_CLIENT_NAME_ATTRIBUTE, self.client_name.clone()),
            ])
            .build()
    }

    /// Adds the three values to each event of `layer`, as log fields and metric dimensions.
    pub(crate) fn apply(&self, layer: TelemetryLayer<Client>) -> TelemetryLayer<Client> {
        layer
            .dimension(FETCH_RUNTIME_ATTRIBUTE, self.runtime.clone())
            .dimension(FETCH_TRANSPORT_ATTRIBUTE, self.transport.clone())
            .dimension(HTTP_CLIENT_NAME_ATTRIBUTE, self.client_name.clone())
    }
}

/// How a client's `fetch` metrics are metered.
///
/// Stores the client attributes and an optional [provider][MeterProvider]. It uses the global
/// meter provider if no custom provider is set. It creates the [`InstrumentationScope`] with the
/// [`Meter`], so the client name can change after provider selection.
#[derive(Clone)]
pub(crate) struct Metering {
    provider: Option<Arc<dyn MeterProvider + Send + Sync>>,
    client_attributes: ClientAttributes,
}

impl fmt::Debug for Metering {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Metering")
            .field("client_attributes", &self.client_attributes)
            .field("custom_provider", &self.provider.is_some())
            .finish()
    }
}

#[cfg(test)]
mod gamma_survivor_tests {
    use opentelemetry::metrics::NoopMeterProvider;

    use super::*;

    #[test]
    fn gamma_survivor_metering_debug_reports_all_fields_and_provider_presence() {
        let metering = Metering::new(ClientAttributes::new(
            "runtime-marker".into(),
            "transport-marker".into(),
            "client-marker".into(),
        ));
        assert_eq!(
            format!("{metering:?}"),
            "Metering { client_attributes: ClientAttributes { runtime: \"runtime-marker\", transport: \"transport-marker\", client_name: \"client-marker\" }, custom_provider: false }"
        );
        let metering = metering.with_provider(NoopMeterProvider::new());
        assert_eq!(
            format!("{metering:?}"),
            "Metering { client_attributes: ClientAttributes { runtime: \"runtime-marker\", transport: \"transport-marker\", client_name: \"client-marker\" }, custom_provider: true }"
        );
    }
}
impl Metering {
    /// Creates metering that records against the global meter provider.
    pub(crate) fn new(client_attributes: ClientAttributes) -> Self {
        Self {
            provider: None,
            client_attributes,
        }
    }

    /// Records against the given custom meter provider instead of the global one.
    ///
    /// The meter scope uses a client name set before or after this call.
    pub(crate) fn with_provider<P>(mut self, provider: P) -> Self
    where
        P: MeterProvider + Send + Sync + 'static,
    {
        let provider: Box<dyn MeterProvider + Send + Sync> = Box::new(provider);
        self.provider = Some(provider.into());
        self
    }

    /// Updates the client name recorded on the eventual meter's scope and on the request events.
    pub(crate) fn with_client_name(mut self, client_name: Cow<'static, str>) -> Self {
        self.client_attributes = self.client_attributes.with_client_name(client_name);
        self
    }

    pub(crate) fn client_attributes(&self) -> &ClientAttributes {
        &self.client_attributes
    }

    /// Returns `true` if the client uses a custom meter provider.
    #[cfg(test)]
    pub(crate) fn has_custom_provider(&self) -> bool {
        self.provider.is_some()
    }
}

impl From<Metering> for Meter {
    fn from(metering: Metering) -> Self {
        let scope = metering.client_attributes.scope();
        match metering.provider {
            Some(provider) => provider.meter_with_scope(scope),
            None => opentelemetry::global::meter_with_scope(scope),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use opentelemetry::Value;

    use super::*;

    fn test_metering(client_name: &'static str) -> Metering {
        Metering::new(ClientAttributes::new(
            Cow::Borrowed("tokio"),
            Cow::Borrowed("hyper"),
            Cow::Borrowed(client_name),
        ))
    }

    fn test_provider() -> opentelemetry_sdk::metrics::SdkMeterProvider {
        opentelemetry_sdk::metrics::SdkMeterProvider::builder().build()
    }

    #[test]
    fn new_metering_uses_global_provider() {
        let metering = test_metering("http_client");
        assert!(metering.provider.is_none());
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn with_provider_sets_custom_provider() {
        let metering = test_metering("http_client").with_provider(test_provider());
        assert!(metering.provider.is_some());
    }

    #[test]
    fn from_global_metering_materializes_meter() {
        let _meter: Meter = test_metering("http_client").into();
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn with_provider_preserves_client_name() {
        let metering = test_metering("preserved_client").with_provider(test_provider());
        assert_eq!(metering.client_attributes().client_name, "preserved_client");
        assert!(metering.provider.is_some());
    }

    #[test]
    fn scope_has_runtime_transport_and_client_name_attributes() {
        let scope = ClientAttributes::new(Cow::Borrowed("tokio"), Cow::Borrowed("hyper"), Cow::Borrowed("my_client")).scope();

        let attributes = scope
            .attributes()
            .map(|kv| (kv.key.as_str().to_owned(), kv.value.clone()))
            .collect::<Vec<_>>();
        assert_eq!(scope.name(), METER_NAME);
        assert_eq!(
            attributes,
            [
                (FETCH_RUNTIME_ATTRIBUTE.to_owned(), Value::from("tokio")),
                (FETCH_TRANSPORT_ATTRIBUTE.to_owned(), Value::from("hyper")),
                (HTTP_CLIENT_NAME_ATTRIBUTE.to_owned(), Value::from("my_client")),
            ]
        );
    }

    #[test]
    fn with_client_name_updates_global_client_name() {
        let metering = test_metering("http_client").with_client_name(Cow::Borrowed("renamed_client"));
        assert_eq!(metering.client_attributes().client_name, "renamed_client");
        assert!(metering.provider.is_none());
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn with_client_name_updates_custom_client_name() {
        let metering = test_metering("http_client")
            .with_provider(test_provider())
            .with_client_name(Cow::Borrowed("renamed_client"));
        assert_eq!(metering.client_attributes().client_name, "renamed_client");
        assert!(metering.provider.is_some());
    }
}
