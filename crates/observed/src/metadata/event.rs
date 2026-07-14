// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event-level metadata.

use std::any::TypeId;

use crate::metadata::log::LogDescription;
use crate::metadata::metric::MetricDescription;

/// Static description of a telemetry event type.
///
/// Available as a `const` on every type that implements [`crate::Event`],
/// providing compile-time metadata about the event's shape.
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

    /// Returns the event name (from `#[event(name = "...")]`).
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the [`TypeId`] of the event struct, if available.
    ///
    /// Present for all compile-time events (`#[derive(Event)]`).
    /// `None` for dynamic events (e.g. from the tracing bridge).
    #[must_use]
    pub const fn type_id(&self) -> Option<TypeId> {
        self.type_id
    }

    /// Returns the [`TypeId`] for a compile-time event type `T`.
    ///
    /// Convenience for building lookup tables keyed by event type.
    #[must_use]
    pub fn type_id_of<T: crate::event::Event>() -> Option<TypeId> {
        T::DESCRIPTION.type_id
    }

    /// Returns the per-signal log description, if the event produces logs.
    #[must_use]
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
    pub const fn metric(&self) -> Option<&MetricDescription> {
        self.metric.as_ref()
    }

    /// Returns `true` if this event is disabled by default.
    ///
    /// Disabled events are neither logs nor metrics unless a processor
    /// explicitly opts in via `is_interested` and/or filters in `process()`.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Returns `true` if this event produces log records.
    #[must_use]
    pub const fn is_log(&self) -> bool {
        self.log.is_some()
    }

    /// Returns `true` if this event produces metric data points
    /// (either an event-level metric or at least one field-level metric).
    #[must_use]
    pub const fn contains_metrics(&self) -> bool {
        self.metric.is_some() || self.has_field_metrics
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_description_type_id_roundtrips() {
        let with = EventDescription::new("e", Some(TypeId::of::<u32>()), None, None, false, false);
        assert_eq!(with.type_id(), Some(TypeId::of::<u32>()));

        let without = EventDescription::new("e", None, None, None, false, false);
        assert_eq!(without.type_id(), None);
    }
}
