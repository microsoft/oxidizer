// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The overlay-carrying wrapper produced by the destination extension traits.
//!
//! [`DestinationOverlay`] keeps a destination together with the optional
//! resource, redaction, instrumentation-scope and log-sampling configuration
//! that the builder applies to that destination.

use std::any::type_name;

use observed_destination_core::IntoLogDestination;

use crate::inst_scope_helpers::IntoInstrumentationScope;
use crate::redaction_policy::RedactionPolicy;
use crate::sampling::LogSampler;

/// A wrapper attaching optional overlays to a destination.
///
/// Carries an optional [`opentelemetry_sdk::Resource`], a
/// [`RedactionPolicy`], (for metric destinations only) an
/// [`opentelemetry::InstrumentationScope`], and (for log destinations only) a
/// [`LogSampler`] alongside the inner
/// [`observed_destination_core::LogDestination`] or [`observed_destination_core::MetricDestination`]
/// — or a `bon` builder for one, which is finished on the caller's behalf.
///
/// Construct via methods of [`crate::MetricDestinationExt`] /
/// [`crate::LogDestinationExt`] extension traits.
/// Chain multiple calls to set several overlays:
///
/// ```ignore
/// stdout_logs()
///     .level(Info)
///     .extend_redaction(debug_passthrough_policy)
///     .extend_resource(extra_attrs_resource)
///
/// stdout_metrics()
///     .with_scope(my_instrumentation_scope)
///     .extend_redaction(metric_redaction_policy)
/// ```
///
/// When passed to [`crate::Builder::log`] or [`crate::Builder::metric`], the builder
/// captures these overlays and calls the inner destination's
/// [`observed_destination_core::LogDestination::register`] or [`observed_destination_core::MetricDestination::register`] method.
pub struct DestinationOverlay<D> {
    pub(crate) inner: D,
    pub(crate) resource: Option<opentelemetry_sdk::Resource>,
    pub(crate) redaction: Option<RedactionPolicy>,
    pub(crate) scope: Option<opentelemetry::InstrumentationScope>,
    pub(crate) sampler: Option<Box<dyn LogSampler>>,
}

impl<D: std::fmt::Debug> std::fmt::Debug for DestinationOverlay<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .field("resource", &self.resource.is_some())
            .field("redaction", &self.redaction.is_some())
            .field("scope", &self.scope.is_some())
            .field("sampler", &self.sampler.is_some())
            .finish()
    }
}

impl<D> DestinationOverlay<D> {
    /// Sets the resource overlay (destination-level). Replaces any
    /// previously-set overlay.
    #[must_use]
    pub fn extend_resource(mut self, resource: opentelemetry_sdk::Resource) -> Self {
        self.resource = Some(resource);
        self
    }

    /// Sets the redaction overlay (destination-level). Replaces any
    /// previously-set overlay. The overlay is merged on top of the
    /// sink-level baseline at build time.
    #[must_use]
    pub fn extend_redaction(mut self, redaction: RedactionPolicy) -> Self {
        self.redaction = Some(redaction);
        self
    }
}

impl<D: IntoLogDestination> DestinationOverlay<D> {
    /// Sets the destination's log sampler, replacing any previous sampler.
    ///
    /// This continues an existing log overlay. See
    /// [`crate::LogDestinationExt::with_sampler`] for the attachment contract.
    #[must_use]
    pub fn with_sampler(mut self, sampler: impl LogSampler) -> Self {
        self.sampler = Some(Box::new(sampler));
        self
    }
}

impl<D: observed_destination_core::IntoMetricDestination> DestinationOverlay<D> {
    /// Sets the instrumentation scope override (destination-level). Replaces any
    /// previously-set override. The override is applied to the meter created for
    /// this destination.
    #[must_use]
    pub fn with_scope(mut self, scope: impl IntoInstrumentationScope) -> Self {
        self.scope = Some(scope.into_instrumentation_scope());
        self
    }
}
