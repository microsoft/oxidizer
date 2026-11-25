// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use observed::processing::EventProcessor;
use observed_destination_core::{DestinationResult, IntoLogDestination, IntoMetricDestination, LogDestination, MetricDestination};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{LoggerProviderBuilder, SdkLoggerProvider};
use opentelemetry_sdk::metrics::{MeterProviderBuilder, SdkMeterProvider};
use performables::arc::Arc;

use crate::redaction_policy::engine_from_layers;
use crate::sampling::LogSampler;
use crate::{DestinationOverlay, LogEventProcessor, MetricEventProcessor, RedactionPolicy};

/// Creates a [`ProcessorBuilder`] for the pipeline-only path.
///
/// Use this when the resulting processor list is wired into something other
/// than a freshly-built [`Sink`](observed::Sink) — e.g., a deferred-init harness. For the
/// standard "give me an sink" path, use [`builder`](crate::builder()) instead.
pub fn processor_builder() -> ProcessorBuilder {
    ProcessorBuilder {
        processors: Vec::new(),
        log_destinations: Vec::new(),
        metric_destinations: Vec::new(),
        redaction: None,
        resource: None,
    }
}

/// Builds a `Vec<Arc<dyn EventProcessor>>` from log/metric destinations and
/// optional raw processors. Does **not** carry sink-level knobs (`id`,
/// `isolated_enrichment`) — those live on [`crate::Builder`].
///
/// Configure via [`log`](Self::log) / [`metric`](Self::metric) plus their
/// companion methods ([`redaction`](Self::redaction),
/// [`resource`](Self::resource)) and/or register raw processors via
/// [`processor`](Self::processor) / [`processors`](Self::processors).
///
/// Terminate with [`build`](Self::build), which returns a
/// [`DestinationResult<Vec<Arc<dyn EventProcessor>>>`] because destination
/// setup, such as opening a file, can fail.
#[must_use]
pub struct ProcessorBuilder {
    processors: Vec<Arc<dyn EventProcessor>>,
    log_destinations: Vec<ErasedLogDestination>,
    metric_destinations: Vec<ErasedMetricDestination>,
    redaction: Option<RedactionPolicy>,
    resource: Option<Resource>,
}

impl std::fmt::Debug for ProcessorBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessorBuilder")
            .field("processors", &self.processors.len())
            .field("log_destinations", &self.log_destinations.len())
            .field("metric_destinations", &self.metric_destinations.len())
            .finish_non_exhaustive()
    }
}

impl ProcessorBuilder {
    /// Adds a raw processor. Included in the output of [`build`](Self::build)
    /// alongside any destination-derived processors.
    pub fn processor(mut self, processor: impl EventProcessor + 'static) -> Self {
        self.processors.push(Arc::new(processor).coerce::<dyn EventProcessor>(|arc| arc));
        self
    }

    /// Adds multiple raw processors. Included in the output of
    /// [`build`](Self::build) alongside any destination-derived processors.
    pub fn processors(mut self, processors: impl IntoIterator<Item = impl Into<Arc<dyn EventProcessor>>>) -> Self {
        self.processors.extend(processors.into_iter().map(Into::into));
        self
    }

    /// Adds a log destination.
    pub fn log(mut self, destination: impl Into<ErasedLogDestination>) -> Self {
        self.log_destinations.push(destination.into());
        self
    }

    /// Adds a metric destination.
    pub fn metric(mut self, destination: impl Into<ErasedMetricDestination>) -> Self {
        self.metric_destinations.push(destination.into());
        self
    }

    /// Sets the sink-level baseline [`RedactionPolicy`].
    ///
    /// Each destination's [`RedactionEngine`](data_privacy::RedactionEngine)
    /// is built from this baseline plus the destination's own overlay (set
    /// via methods of [`crate::MetricDestinationExt`] / [`crate::LogDestinationExt`]
    /// extension traits). Overlay entries win on `DataClass` conflict; destinations
    /// without an overlay use an engine built from the baseline alone.
    ///
    /// Defaults to [`RedactionPolicy::new()`] (empty — every class falls
    /// through to the upstream default fallback redactor) when not set.
    pub fn redaction(mut self, redaction: RedactionPolicy) -> Self {
        self.redaction = Some(redaction);
        self
    }

    /// Sets the `OTel` [`Resource`] attached to the logger and meter providers
    /// assembled from the registered destinations.
    pub fn resource(mut self, resource: Resource) -> Self {
        self.resource = Some(resource);
        self
    }

    /// Assembles the `OTel` providers from any registered destinations, wraps
    /// them in the corresponding processors, and returns the full processor
    /// list (raw processors first, then destination-derived ones).
    ///
    /// Each assembled processor owns its provider. [`EventProcessor::flush`]
    /// sends buffered telemetry without shutting the provider down.
    /// See [`LogEventProcessor`] and [`MetricEventProcessor`] for shutdown behavior.
    ///
    /// # Errors
    ///
    /// Returns the destination's error if a log or metric destination fails to register.
    ///
    /// # Instrumentation scope
    ///
    /// - **Loggers**: always constructed with the default empty `OTel`
    ///   instrumentation scope.
    /// - **Meters**: per-destination — if a metric destination was wrapped via
    ///   [`crate::MetricDestinationExt::with_scope`], its `InstrumentationScope` is
    ///   passed to `provider.meter_with_scope(...)`; otherwise the default
    ///   empty scope is used.
    pub fn build(self) -> DestinationResult<Vec<Arc<dyn EventProcessor>>> {
        let Self {
            mut processors,
            log_destinations,
            metric_destinations,
            redaction,
            resource,
        } = self;
        let dest_processors = assemble_processors(log_destinations, metric_destinations, redaction, resource.as_ref())?;
        processors.extend(dest_processors);
        Ok(processors)
    }
}

/// Assembles the `OTel`-backed event processors from the registered log and
/// metric destinations. Called from [`ProcessorBuilder::build`].
fn assemble_processors(
    log_destinations: Vec<ErasedLogDestination>,
    metric_destinations: Vec<ErasedMetricDestination>,
    redaction: Option<RedactionPolicy>,
    resource: Option<&Resource>,
) -> DestinationResult<Vec<Arc<dyn EventProcessor>>> {
    let baseline = redaction.unwrap_or_default();
    let mut processors: Vec<Arc<dyn EventProcessor>> = Vec::new();

    // One `SdkLoggerProvider` + one `LogEventProcessor` per log
    // destination, so per-destination `Resource` / `RedactionPolicy`
    // overlays apply to exactly that destination.
    for dest in log_destinations {
        let dest_engine = engine_from_layers(&baseline, dest.redaction_overlay.as_ref());

        // `LoggerProviderBuilder::with_resource` is additive: each call
        // merges the provided resource with what's already set. Apply the
        // sink-level baseline first, then the destination's overlay
        // (overlay wins on key conflicts, via OTel's internal
        // `Resource::merge`).
        let mut builder = SdkLoggerProvider::builder();
        if let Some(r) = resource {
            builder = builder.with_resource(r.clone());
        }
        if let Some(r) = dest.resource_overlay {
            builder = builder.with_resource(r);
        }
        builder = (dest.apply)(builder)?;
        let provider = builder.build();

        processors.push(Arc::new(LogEventProcessor::new(provider, dest_engine, dest.sampler)).coerce::<dyn EventProcessor>(|arc| arc));
    }

    // Same per-destination treatment for metric destinations, plus
    // optional per-destination `InstrumentationScope` (set by the user via
    // `MetricDestinationExt::with_scope`).
    for dest in metric_destinations {
        let dest_engine = engine_from_layers(&baseline, dest.redaction_overlay.as_ref());

        let mut builder = SdkMeterProvider::builder();
        if let Some(r) = resource {
            builder = builder.with_resource(r.clone());
        }
        if let Some(r) = dest.resource_overlay {
            builder = builder.with_resource(r);
        }
        builder = (dest.apply)(builder)?;
        let provider = builder.build();

        processors
            .push(Arc::new(MetricEventProcessor::new(provider, dest_engine, dest.scope_override)).coerce::<dyn EventProcessor>(|arc| arc));
    }

    Ok(processors)
}

/// Type-erased log destination captured at insertion time.
pub struct ErasedLogDestination {
    resource_overlay: Option<Resource>,
    redaction_overlay: Option<RedactionPolicy>,
    sampler: Option<Box<dyn LogSampler>>,
    apply: Box<dyn FnOnce(LoggerProviderBuilder) -> DestinationResult<LoggerProviderBuilder>>,
}

impl std::fmt::Debug for ErasedLogDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("resource_overlay", &self.resource_overlay)
            .field("redaction_overlay", &self.redaction_overlay)
            .field("sampler", &self.sampler.is_some())
            .finish_non_exhaustive()
    }
}

impl<D: IntoLogDestination> From<DestinationOverlay<D>> for ErasedLogDestination {
    fn from(overlay: DestinationOverlay<D>) -> Self {
        let log_destination = overlay.inner.into_log_destination();
        Self {
            resource_overlay: overlay.resource,
            redaction_overlay: overlay.redaction,
            sampler: overlay.sampler,
            apply: Box::new(|builder| log_destination.register(builder)),
        }
    }
}

impl<T: IntoLogDestination> From<T> for ErasedLogDestination {
    fn from(destination: T) -> Self {
        let log_destination = destination.into_log_destination();
        Self {
            resource_overlay: None,
            redaction_overlay: None,
            sampler: None,
            apply: Box::new(|builder| log_destination.register(builder)),
        }
    }
}

/// Type-erased metric destination captured at insertion time.
pub struct ErasedMetricDestination {
    resource_overlay: Option<Resource>,
    redaction_overlay: Option<RedactionPolicy>,
    scope_override: Option<opentelemetry::InstrumentationScope>,
    apply: Box<dyn FnOnce(MeterProviderBuilder) -> DestinationResult<MeterProviderBuilder>>,
}

impl std::fmt::Debug for ErasedMetricDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("resource_overlay", &self.resource_overlay)
            .field("redaction_overlay", &self.redaction_overlay)
            .field("scope_override", &self.scope_override)
            .finish_non_exhaustive()
    }
}

impl<T: IntoMetricDestination> From<T> for ErasedMetricDestination {
    fn from(destination: T) -> Self {
        let metric_destination = destination.into_metric_destination();
        Self {
            resource_overlay: None,
            redaction_overlay: None,
            scope_override: None,
            apply: Box::new(|builder| metric_destination.register(builder)),
        }
    }
}

impl<D: IntoMetricDestination> From<DestinationOverlay<D>> for ErasedMetricDestination {
    fn from(overlay: DestinationOverlay<D>) -> Self {
        let metric_destination = overlay.inner.into_metric_destination();
        Self {
            resource_overlay: overlay.resource,
            redaction_overlay: overlay.redaction,
            scope_override: overlay.scope,
            apply: Box::new(|builder| metric_destination.register(builder)),
        }
    }
}

#[cfg(test)]
mod tests {
    use observed::enrichment::EnrichFnExt;
    use observed::{Enrichment, emit, event};
    use observed_destination_memory::{memory_logs, memory_metrics};
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::logs::InMemoryLogExporter;
    use opentelemetry_sdk::metrics::InMemoryMetricExporter;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics};

    use crate::test_support::TestRequestCount;

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry reads the wall clock to set the log's observed timestamp. This test runs natively and under cargo careful."
    )]
    fn metric_only_fields_and_enrichments_are_excluded_from_logs() {
        use opentelemetry::logs::AnyValue;
        use tick::SimpleClock;

        use crate::builder;

        // Distinct signal names expose cross-signal metadata mixups.
        #[event("test.signal_routing")]
        #[info(name = "test.signal_routing.log")]
        #[counter]
        struct RoutedEvent {
            #[dimension(log = "log.status", metric = "metric.status")]
            #[unredacted]
            status: i64,
            #[dimension(log = exclude, metric = "metric.region")]
            #[unredacted]
            region: &'static str,
        }

        #[derive(Enrichment)]
        struct RoutedEnrichment {
            #[dimension(log = exclude, metric = "metric.tenant")]
            #[unredacted]
            tenant: i64,
            #[dimension(log = "log.request_id")]
            #[unredacted]
            request_id: i64,
        }

        let log_exporter = InMemoryLogExporter::default();
        let metric_exporter = InMemoryMetricExporter::default();
        let sink = builder()
            .log(memory_logs(log_exporter.clone()))
            .metric(memory_metrics(metric_exporter.clone()))
            .clock(SimpleClock::new_frozen())
            .build()
            .unwrap();

        (|| {
            emit!(
                sink,
                RoutedEvent {
                    status: 200,
                    region: "west",
                }
            );
        })
        .enrich(&sink, RoutedEnrichment { tenant: 7, request_id: 42 })();
        sink.flush().unwrap();

        let logs = log_exporter.get_emitted_logs().unwrap();
        assert_eq!(logs.len(), 1);

        let record = &logs[0].record;
        assert_eq!(record.event_name(), Some("test.signal_routing.log"));
        assert!(record.body().is_none());

        let attributes: Vec<_> = record.attributes_iter().cloned().collect();
        assert!(attributes.contains(&("log.status".into(), AnyValue::Int(200))));
        assert!(attributes.contains(&("log.request_id".into(), AnyValue::Int(42))));

        // Excluded fields must not leak under either metric names or default field names.
        for (key, _) in &attributes {
            let key = key.as_str();
            assert!(!key.starts_with("metric."), "metric attribute in log: {key}");
            assert!(!matches!(key, "region" | "tenant"), "excluded field in log: {key}");
        }

        let exported_metrics = metric_exporter.get_finished_metrics().unwrap();
        let metrics: Vec<_> = exported_metrics
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .collect();
        assert_eq!(metrics.len(), 1);

        let metric = metrics[0];
        assert_eq!(metric.name(), "test.signal_routing");
        let AggregatedMetrics::F64(MetricData::Sum(sum)) = metric.data() else {
            panic!("expected event counter");
        };

        let points: Vec<_> = sum.data_points().collect();
        assert_eq!(points.len(), 1);
        let point = points[0];

        // Dimension order is not part of the contract.
        let mut dimensions: Vec<_> = point.attributes().cloned().collect();
        dimensions.sort_unstable_by(|left, right| left.key.as_str().cmp(right.key.as_str()));
        assert_eq!(
            dimensions,
            [
                KeyValue::new("metric.region", "west"),
                KeyValue::new("metric.status", 200_i64),
                KeyValue::new("metric.tenant", 7_i64),
            ]
        );
    }

    /// `metric_dest.with_scope(InstrumentationScope)` flows through to the
    /// `OTel` meter, surfacing on exported metrics' `InstrumentationScope`.
    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry reads the wall clock when metric aggregation starts. This test runs natively and under cargo careful."
    )]
    fn metric_with_scope_propagates_to_meter() {
        use crate::MetricDestinationExt;

        let exporter = InMemoryMetricExporter::default();
        let exporter_clone = exporter.clone();

        let scope = opentelemetry::InstrumentationScope::builder("my.library")
            .with_version("1.2.3")
            .build();

        let sink = crate::builder()
            .metric(memory_metrics(exporter).with_scope(scope))
            .clock(tick::SimpleClock::new_frozen())
            .build()
            .expect("build");

        emit!(sink, TestRequestCount { status: 200i64 });
        sink.flush().expect("flush");

        let metrics = exporter_clone.get_finished_metrics().expect("metrics");
        let scopes: Vec<&str> = metrics
            .iter()
            .flat_map(opentelemetry_sdk::metrics::data::ResourceMetrics::scope_metrics)
            .map(|sm| sm.scope().name())
            .collect();
        assert!(
            scopes.iter().any(|n| n == &"my.library"),
            "expected meter scope name 'my.library', got {scopes:?}",
        );
    }

    /// `metric_dest.with_scope("name")` accepts a string via
    /// [`IntoInstrumentationScope`] and produces a meter scope with just the
    /// given name.
    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry reads the wall clock when metric aggregation starts. This test runs natively and under cargo careful."
    )]
    fn metric_with_scope_accepts_string_name() {
        use crate::MetricDestinationExt;

        let exporter = InMemoryMetricExporter::default();
        let destination = memory_metrics(exporter.clone()).with_scope("original.library");
        let sink = crate::builder()
            .metric(destination.with_scope("name.only.library"))
            .clock(tick::SimpleClock::new_frozen())
            .build()
            .expect("build");

        emit!(sink, TestRequestCount { status: 200i64 });
        sink.flush().unwrap();

        let metrics = exporter.get_finished_metrics().expect("metrics");
        let scopes: Vec<&str> = metrics
            .iter()
            .flat_map(opentelemetry_sdk::metrics::data::ResourceMetrics::scope_metrics)
            .map(|sm| sm.scope().name())
            .collect();
        assert_eq!(scopes, ["name.only.library"]);
    }
}
