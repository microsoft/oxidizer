// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event-level metadata.

use std::any::TypeId;

use crate::metadata::log::LogDescription;
use crate::metadata::metric::MetricDescription;

/// Description of a telemetry event type.
///
/// Available as a `const` on every type that implements [`crate::Event`],
/// providing compile-time metadata about the event's shape. Dynamic adaptors
/// construct the same description at runtime and may omit Rust type identity.
///
/// The event name is shared across all signals; per-signal metadata lives
/// in `log` / `metric`.
#[derive(Debug, Clone, Copy)]
pub struct EventDescription {
    name: &'static str,
    type_id: Option<TypeId>,
    log: Option<LogDescription>,
    metric: Option<MetricDescription>,
    has_field_metrics: bool,
    disabled: bool,
}

impl EventDescription {
    /// Creates a new event description.
    #[must_use]
    pub const fn new(
        name: &'static str,
        type_id: Option<TypeId>,
        log: Option<LogDescription>,
        metric: Option<MetricDescription>,
        has_field_metrics: bool,
        disabled: bool,
    ) -> Self {
        Self {
            name,
            type_id,
            log,
            metric,
            has_field_metrics,
            disabled,
        }
    }

    /// Returns the event name (from `#[event("...")]`).
    #[must_use]
    #[inline]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the [`TypeId`] of the event struct, if available.
    ///
    /// Present for all compile-time events (`#[event(...)]`).
    /// `None` for dynamic events (e.g. from the tracing bridge).
    #[must_use]
    #[inline]
    pub const fn type_id(&self) -> Option<TypeId> {
        self.type_id
    }

    /// Returns the per-signal log description, if the event produces logs.
    #[must_use]
    #[inline]
    pub const fn log(&self) -> Option<&LogDescription> {
        self.log.as_ref()
    }

    /// Returns the event-level metric description, if any.
    ///
    /// This is the metric declared directly on the event (records `1` per
    /// emission). Field-level metrics are exposed on each
    /// [`FieldDescriptor`](crate::metadata::FieldDescriptor) via
    /// [`FieldDescriptor::metric`](crate::metadata::FieldDescriptor::metric).
    #[must_use]
    #[inline]
    pub const fn metric(&self) -> Option<&MetricDescription> {
        self.metric.as_ref()
    }

    /// Returns `true` if this event is disabled by default.
    ///
    /// The sink does not enforce this flag. Processors interpret it, normally
    /// from [`EventProcessor::is_interested`](crate::processing::EventProcessor::is_interested);
    /// `process` can only discard an event after accepting delivery.
    #[must_use]
    #[inline]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Returns `true` if this event produces log records.
    #[must_use]
    #[inline]
    pub const fn is_log(&self) -> bool {
        self.log.is_some()
    }

    /// Returns `true` if this event produces metric data points
    /// (either an event-level metric or at least one field-level metric).
    #[must_use]
    #[inline]
    pub const fn contains_metrics(&self) -> bool {
        self.metric.is_some() || self.has_field_metrics
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::metric::InstrumentKind;
    use crate::severity::Severity;

    #[test]
    fn event_description_type_id_roundtrips() {
        let with = EventDescription::new("e", Some(TypeId::of::<u32>()), None, None, false, false);
        assert_eq!(with.type_id(), Some(TypeId::of::<u32>()));

        let without = EventDescription::new("e", None, None, None, false, false);
        assert_eq!(without.type_id(), None);
    }

    #[test]
    fn event_description_reports_each_signal_independently() {
        // A description is what a processor selects on, so every accessor must
        // report its own field: a signal answered from the wrong field (or from
        // a constant) silently mis-routes the event.
        let plain = EventDescription::new("plain", None, None, None, false, false);
        assert_eq!(plain.name(), "plain");
        assert!(plain.metric().is_none());
        assert!(!plain.is_log());
        assert!(!plain.is_disabled());
        assert!(!plain.contains_metrics());

        let logged = EventDescription::new(
            "logged",
            None,
            Some(LogDescription::new("logged", Severity::Info, None)),
            None,
            false,
            false,
        );
        assert!(logged.is_log());

        assert!(EventDescription::new("off", None, None, None, false, true).is_disabled());
    }

    #[test]
    fn an_event_level_or_field_level_metric_alone_makes_the_event_metric_producing() {
        // The two metric sources are alternatives, not requirements: an event
        // carrying only one of them still produces metric data points.
        let event_metric = EventDescription::new(
            "event.metric",
            None,
            None,
            Some(MetricDescription::new("m", InstrumentKind::Counter, "", "")),
            false,
            false,
        );
        assert_eq!(event_metric.metric().expect("metric is set").instrument_name(), "m");
        assert!(event_metric.contains_metrics());

        let field_metric = EventDescription::new("field.metric", None, None, None, true, false);
        assert!(field_metric.metric().is_none());
        assert!(field_metric.contains_metrics());
    }
}
