// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Prepared destinations for numeric metric measurements.

use std::any::TypeId;
use std::error::Error;
use std::fmt;

use super::EventView;
use crate::Event;
use crate::metadata::{EventDescription, MetricDescription, MetricValue, MetricValueKind};

/// An event has no type identity, no metrics, or is
/// already registered with this destination.
#[derive(Debug)]
pub struct MetricBindingError {
    event_name: &'static str,
    reason: &'static str,
}

impl fmt::Display for MetricBindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot bind metrics on event `{}`: {}", self.event_name, self.reason)
    }
}

impl Error for MetricBindingError {}

/// A bound event did not supply a numeric slot with the declared representation.
///
/// This indicates an inconsistent hand-written event implementation.
/// Processors must report the failure through their diagnostics or flush API.
#[derive(Debug)]
pub struct MetricReadError {
    event_name: &'static str,
    slot: usize,
    reason: &'static str,
}

impl fmt::Display for MetricReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "event `{}` has invalid metric slot {}: {}",
            self.event_name, self.slot, self.reason
        )
    }
}

impl Error for MetricReadError {}

#[derive(Debug)]
struct MetricTarget<T> {
    value_kind: Option<MetricValueKind>,
    target: T,
}

#[derive(Debug)]
struct EventTargets<T> {
    identity: TypeId,
    metrics: Vec<MetricTarget<T>>,
}

/// Backend-neutral bindings from complete event types to prepared metric targets.
///
/// Configure bindings at startup, then share them read-only across processors.
/// Event selection uses binary search over numeric Rust type identities; slot
/// selection and extraction use generated numeric accessors. Recording performs
/// no string matching, hashing, allocation, redaction, or generic field visits.
///
/// Type identities are process-local Rust identities, not a cross-DLL ABI.
/// Dynamic events without a matching typed identity are not accepted.
#[derive(Debug)]
pub struct MetricBindings<T> {
    events: Vec<EventTargets<T>>,
}

impl<T> Default for MetricBindings<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> MetricBindings<T> {
    /// Creates an empty set of bindings.
    #[must_use]
    pub const fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// Registers every metric on an event with a prepared backend target.
    ///
    /// Calls `prepare` once per instrument using its declared metadata. Backend
    /// implementations must validate supported kinds before calling this method.
    /// Each event can be registered once per destination; different destinations
    /// remain independent.
    /// This method allocates and belongs in initialization, never on emission.
    ///
    /// # Errors
    ///
    /// Returns an error if the event has no typed identity or no instruments,
    /// or is already registered. No target is prepared and no bindings are
    /// changed when validation fails.
    pub fn bind_event<E: Event>(&mut self, mut prepare: impl FnMut(&MetricDescription) -> T) -> Result<(), MetricBindingError> {
        let description = E::DESCRIPTION;
        let identity = description.type_id().ok_or_else(|| MetricBindingError {
            event_name: description.name(),
            reason: "a typed event identity is required",
        })?;
        if E::METRICS.is_empty() {
            return Err(MetricBindingError {
                event_name: description.name(),
                reason: "the event must declare at least one metric instrument",
            });
        }
        // Sorted vectors avoid hashing and first-use cache mutation at emit time.
        let index = self
            .events
            .binary_search_by_key(&identity, |event| event.identity)
            .err()
            .ok_or_else(|| MetricBindingError {
                event_name: description.name(),
                reason: "the event is already registered",
            })?;
        let metrics = E::METRICS
            .iter()
            .map(|metric| MetricTarget {
                value_kind: metric.value_kind(),
                target: prepare(metric),
            })
            .collect();
        self.events.insert(index, EventTargets { identity, metrics });
        Ok(())
    }

    /// Whether a non-disabled event has any bound metrics.
    #[must_use]
    pub fn is_interested(&self, description: &EventDescription) -> bool {
        !description.is_disabled() && self.targets(description).is_some()
    }

    /// Reads bound numeric measurements and passes them to backend targets.
    ///
    /// Unbound and disabled events are ignored. Existing sink context and
    /// sampling behavior is unchanged; this operation itself never inspects
    /// enrichment or constructs signal records.
    ///
    /// # Errors
    ///
    /// Returns an error if an accepted event lacks a bound numeric slot or its
    /// representation differs from the declaration. Earlier targets have already
    /// received their measurements.
    pub fn record(&self, event: &EventView<'_>, mut record: impl FnMut(&T, MetricValue)) -> Result<(), MetricReadError> {
        let description = event.description();
        if description.is_disabled() {
            return Ok(());
        }
        let Some(targets) = self.targets(&description) else {
            return Ok(());
        };
        for (slot, metric) in targets.metrics.iter().enumerate() {
            let value = event.metric_value(slot).ok_or_else(|| MetricReadError {
                event_name: description.name(),
                slot,
                reason: "no measurement supplied",
            })?;
            if metric.value_kind.is_some_and(|kind| kind != value.kind()) {
                return Err(MetricReadError {
                    event_name: description.name(),
                    slot,
                    reason: "numeric representation differs from its declaration",
                });
            }
            record(&metric.target, value);
        }
        Ok(())
    }

    fn targets(&self, description: &EventDescription) -> Option<&EventTargets<T>> {
        let identity = description.type_id()?;
        self.events
            .binary_search_by_key(&identity, |event| event.identity)
            .ok()
            .map(|index| &self.events[index])
    }
}
