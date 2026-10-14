// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Prepared destinations for numeric metric measurements.

use std::any::TypeId;

use crate::Event;
use crate::metadata::{EventDescription, MetricDescription, MetricValue, MetricValueKind};
use crate::processing::EventView;

/// Backend-neutral bindings from complete event types to prepared metric targets.
///
/// Configure bindings at startup, then share them read-only across processors.
/// Recording selects registered Rust type identities and numeric slots; slot
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
        let identity = description
            .type_id()
            .ok_or_else(|| MetricBindingError::from(MissingIdentity::new(description.name())))?;
        if E::METRICS.is_empty() {
            return Err(MissingInstruments::new(description.name()).into());
        }
        // Sorted vectors avoid hashing and first-use cache mutation at emit time.
        let index = self
            .events
            .binary_search_by_key(&identity, |event| event.identity)
            .err()
            .ok_or_else(|| MetricBindingError::from(DuplicateEvent::new(description.name())))?;
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
    /// Unbound and disabled events are ignored. Events emitted through a sink
    /// follow that sink's timestamp, context, and sampling rules. Direct numeric
    /// recording never inspects enrichment or constructs signal records.
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
            let value = event
                .metric_value(slot)
                .ok_or_else(|| MetricReadError::from(MissingMeasurement::new(description.name(), slot)))?;
            if let Some(expected) = metric.value_kind
                && expected != value.kind()
            {
                return Err(MismatchedRepresentation::new(description.name(), slot, expected, value.kind()).into());
            }
            record(&metric.target, value);
        }
        Ok(())
    }

    // Common identity lookup for interest checks and numeric recording.
    fn targets(&self, description: &EventDescription) -> Option<&EventTargets<T>> {
        let identity = description.type_id()?;
        self.events
            .binary_search_by_key(&identity, |event| event.identity)
            .ok()
            .map(|index| &self.events[index])
    }
}

/// An event cannot be registered with this metric destination.
///
/// The source retains the specific registration failure.
#[ohno::error]
#[from(MissingIdentity, MissingInstruments, DuplicateEvent)]
pub struct MetricBindingError;

/// A bound event did not supply a numeric slot with the declared representation.
///
/// This indicates an inconsistent hand-written event implementation.
/// Processors must report the failure through their diagnostics or flush API.
#[ohno::error]
#[from(MissingMeasurement, MismatchedRepresentation)]
pub struct MetricReadError;

// One prepared destination and its declared measurement representation.
#[derive(Debug)]
struct MetricTarget<T> {
    value_kind: Option<MetricValueKind>,
    target: T,
}

// Ordered metric slots belonging to one registered Rust event identity.
#[derive(Debug)]
struct EventTargets<T> {
    identity: TypeId,
    metrics: Vec<MetricTarget<T>>,
}

#[ohno::error]
#[display("cannot bind metrics on event `{event_name}`: a typed event identity is required")]
struct MissingIdentity {
    event_name: &'static str,
}

#[ohno::error]
#[display("cannot bind metrics on event `{event_name}`: the event must declare at least one metric instrument")]
struct MissingInstruments {
    event_name: &'static str,
}

#[ohno::error]
#[display("cannot bind metrics on event `{event_name}`: the event is already registered")]
struct DuplicateEvent {
    event_name: &'static str,
}

#[ohno::error]
#[display("event `{event_name}` has invalid metric slot {slot}: no measurement supplied")]
struct MissingMeasurement {
    event_name: &'static str,
    slot: usize,
}

#[ohno::error]
#[display(
    "event `{event_name}` has invalid metric slot {slot}: numeric representation differs from its declaration (expected {expected:?}, got {actual:?})"
)]
struct MismatchedRepresentation {
    event_name: &'static str,
    slot: usize,
    expected: MetricValueKind,
    actual: MetricValueKind,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::borrow::Cow;
    use std::cell::Cell;
    use std::ops::ControlFlow;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::time::SystemTime;

    use ohno::{EnrichableExt as _, ErrorExt as _};

    use super::*;
    use crate::event;
    use crate::interop::DynEvent;
    use crate::processing::FieldVisitorFn;

    #[event("bound")]
    #[counter]
    struct Bound;

    #[event("empty")]
    #[info]
    struct Empty;

    struct Unidentified;

    impl Event for Unidentified {
        const DESCRIPTION: EventDescription = EventDescription::new("untyped", None, None, None, true, false);
        const METRICS: &'static [MetricDescription] = Bound::METRICS;

        fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }
    }

    // Supplies missing or deliberately incompatible values to the real recording path.
    struct ReadProbe(Option<MetricValue>);

    impl DynEvent for ReadProbe {
        fn name(&self) -> &'static str {
            "bound"
        }
        fn body(&self) -> Option<Cow<'static, str>> {
            None
        }
        fn source_file(&self) -> Option<Cow<'static, str>> {
            None
        }
        fn source_line(&self) -> Option<u32> {
            None
        }
        fn source_crate(&self) -> Option<Cow<'static, str>> {
            None
        }
        fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }
        fn description(&self) -> EventDescription {
            Bound::DESCRIPTION
        }
        fn metric_value(&self, _: usize) -> Option<MetricValue> {
            self.0
        }
    }

    #[test]
    fn metric_errors_support_ohno_enrichment_without_changing_the_diagnostic() {
        static_assertions::assert_impl_all!(MetricBindingError: Send, Sync, std::error::Error, ohno::ErrorExt, ohno::Enrichable);
        static_assertions::assert_impl_all!(MetricReadError: Send, Sync, std::error::Error, ohno::ErrorExt, ohno::Enrichable);
        static_assertions::assert_impl_all!(MetricBindings<()>: UnwindSafe, RefUnwindSafe);
        static_assertions::assert_impl_all!(MetricBindings<Cell<u64>>: UnwindSafe);
        static_assertions::assert_not_impl_any!(MetricBindings<Cell<u64>>: RefUnwindSafe);
        // ohno retains arbitrary source errors and enrichment closures.
        static_assertions::assert_not_impl_any!(MetricBindingError: UnwindSafe, RefUnwindSafe);
        static_assertions::assert_not_impl_any!(MetricReadError: UnwindSafe, RefUnwindSafe);
        let binding = MetricBindingError::from(DuplicateEvent::new("request")).enrich("configuring counters");
        assert_eq!(
            binding.message().lines().next().unwrap(),
            "cannot bind metrics on event `request`: the event is already registered"
        );
        assert_eq!(binding.find_source::<DuplicateEvent>().unwrap().event_name, "request");
        assert!(binding.to_string().contains("configuring counters"));
        let read = MetricReadError::from(MissingMeasurement::new("request", 2_usize)).enrich("recording counters");
        assert_eq!(
            read.message().lines().next().unwrap(),
            "event `request` has invalid metric slot 2: no measurement supplied"
        );
        assert!(read.to_string().contains("recording counters"));
    }

    #[test]
    fn typed_causes_retain_validation_context() {
        let mut bindings = MetricBindings::new();
        let missing = bindings.bind_event::<Unidentified>(|_| ()).unwrap_err();
        assert_eq!(missing.find_source::<MissingIdentity>().unwrap().event_name, "untyped");
        let empty = bindings.bind_event::<Empty>(|_| ()).unwrap_err();
        assert_eq!(empty.find_source::<MissingInstruments>().unwrap().event_name, "empty");
        bindings.bind_event::<Bound>(|_| ()).unwrap();
        let duplicate = bindings.bind_event::<Bound>(|_| ()).unwrap_err();
        assert_eq!(duplicate.find_source::<DuplicateEvent>().unwrap().event_name, "bound");
        let missing_value = ReadProbe(None);
        let view = EventView::new_synthetic(&missing_value, SystemTime::UNIX_EPOCH);
        let read = bindings.record(&view, |(), _| unreachable!()).unwrap_err();
        let cause = read.find_source::<MissingMeasurement>().unwrap();
        assert_eq!((cause.event_name, cause.slot), ("bound", 0));
        let signed_value = ReadProbe(Some(MetricValue::I64(-1)));
        let view = EventView::new_synthetic(&signed_value, SystemTime::UNIX_EPOCH);
        let mismatch = bindings.record(&view, |(), _| unreachable!()).unwrap_err();
        let cause = mismatch.find_source::<MismatchedRepresentation>().unwrap();
        assert_eq!(
            (cause.event_name, cause.slot, cause.expected, cause.actual),
            ("bound", 0, MetricValueKind::U64, MetricValueKind::I64),
        );
    }
}
