// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Prepared destinations for numeric metric measurements.

use std::any::TypeId;
use std::ops::ControlFlow;

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};

use crate::metadata::{EventDescription, MetricDescription, MetricValueKind};
use crate::processing::EventView;
use crate::{Event, Value};

/// Backend-neutral bindings from complete event types to prepared metric targets.
///
/// Configure bindings at startup, then share them read-only across processors.
/// Recording selects registered Rust type identities and stable numeric field
/// indices. The lazy field visitor evaluates only bound measurement getters.
/// Successful recording performs no string matching, hashing, allocation,
/// redaction, or enrichment visits; other event fields are still enumerated.
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
    /// declares inconsistent field indices, or is already registered.
    /// No target is prepared and no bindings are
    /// changed when validation fails.
    pub fn bind_event<E: Event>(&mut self, mut prepare: impl FnMut(&MetricDescription) -> T) -> Result<(), MetricBindingError> {
        let description = E::DESCRIPTION;
        let identity = description
            .type_id()
            .ok_or_else(|| MetricBindingError::from(MissingIdentity::new(description.name())))?;
        if description.metric().is_none()
            && !description
                .fields()
                .iter()
                .any(|field| field.metric().is_some_and(|entry| entry.instrument_description().is_some()))
        {
            return Err(MissingInstruments::new(description.name()).into());
        }
        for (index, field) in description.fields().iter().enumerate() {
            if field.field_index() != Some(index) {
                return Err(InvalidFieldIndex::new(description.name(), index, field.field_index()).into());
            }
        }
        // Sorted vectors avoid hashing and first-use cache mutation at emit time.
        let index = self
            .events
            .binary_search_by_key(&identity, |event| event.identity)
            .err()
            .ok_or_else(|| MetricBindingError::from(DuplicateEvent::new(description.name())))?;
        let counter = description.metric().map(&mut prepare);
        let fields = description
            .fields()
            .iter()
            .map(|field| {
                field
                    .metric()
                    .and_then(|entry| entry.instrument_description())
                    .map(|metric| MetricTarget {
                        value_kind: metric.value_kind(),
                        target: prepare(metric),
                    })
            })
            .collect();
        self.events.insert(index, EventTargets { identity, counter, fields });
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
    /// follow that sink's timestamp, context, and sampling rules. Numeric
    /// recording never inspects enrichment or constructs signal records.
    /// Measurements use [`Value::U64`], [`Value::I64`], or [`Value::F64`]
    /// according to their declared [`MetricValueKind`]. Unsigned fields whose
    /// ordinary getters widen to `I64` are normalized to `U64` without loss.
    ///
    /// # Errors
    ///
    /// Returns an error if an accepted event omits, repeats, or reorders bound
    /// fields, or supplies a non-numeric or incompatible value. Earlier targets
    /// have already received their measurements.
    pub fn record(&self, event: &EventView<'_>, mut record: impl FnMut(&T, Value)) -> Result<(), MetricReadError> {
        let description = event.description();
        if description.is_disabled() {
            return Ok(());
        }
        let Some(targets) = self.targets(&description) else {
            return Ok(());
        };
        if let Some(counter) = &targets.counter {
            record(counter, Value::U64(1));
        }
        let mut remaining = targets
            .fields
            .iter()
            .enumerate()
            .filter_map(|(index, field)| field.as_ref().map(|_| index));
        let mut expected = remaining.next();
        if expected.is_none() {
            return Ok(());
        }
        let mut failure = None;
        // Classified getters erase rather than disclose data if a manual event
        // violates the required unredacted numeric measurement contract.
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);
        let _ = event.visit_fields(&mut |descriptor, getter| {
            let Some(index) = descriptor.field_index() else {
                return ControlFlow::Continue(());
            };
            let Some(Some(metric)) = targets.fields.get(index) else {
                return ControlFlow::Continue(());
            };
            if expected != Some(index) {
                failure = Some(InvalidFieldVisit::new(description.name(), expected, index).into());
                return ControlFlow::Break(());
            }
            match Self::measurement(getter(&redactor), metric.value_kind, description.name(), index) {
                Ok(value) => record(&metric.target, value),
                Err(error) => {
                    failure = Some(error);
                    return ControlFlow::Break(());
                }
            }
            expected = remaining.next();
            ControlFlow::Continue(())
        });
        if let Some(error) = failure {
            return Err(error);
        }
        if let Some(index) = expected {
            return Err(MissingMeasurement::new(description.name(), index).into());
        }
        Ok(())
    }

    fn measurement(value: Value, expected: MetricValueKind, event_name: &'static str, slot: usize) -> Result<Value, MetricReadError> {
        let (value, actual) = match value {
            // Value widens u8/u16/u32 into I64. Static metadata retains their
            // unsigned origin so counters can recover it without losing bits.
            Value::I64(value) if expected == MetricValueKind::U64 && value >= 0 => {
                (Value::U64(value.cast_unsigned()), MetricValueKind::U64)
            }
            value @ Value::I64(_) => (value, MetricValueKind::I64),
            value @ Value::U64(_) => (value, MetricValueKind::U64),
            value @ Value::F64(_) => (value, MetricValueKind::F64),
            _ => return Err(NonnumericMeasurement::new(event_name, slot).into()),
        };
        if expected != actual {
            return Err(MismatchedRepresentation::new(event_name, slot, expected, actual).into());
        }
        Ok(value)
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
#[from(MissingIdentity, MissingInstruments, DuplicateEvent, InvalidFieldIndex)]
pub struct MetricBindingError;

/// A bound event did not visit its declared numeric fields correctly.
///
/// This indicates an inconsistent hand-written event implementation.
/// Processors must report the failure through their diagnostics or flush API.
#[ohno::error]
#[from(MissingMeasurement, MismatchedRepresentation, NonnumericMeasurement, InvalidFieldVisit)]
pub struct MetricReadError;

// One prepared destination and its declared measurement representation.
#[derive(Debug)]
struct MetricTarget<T> {
    value_kind: MetricValueKind,
    target: T,
}

// Prepared event-level target and indexed field targets for one Rust identity.
#[derive(Debug)]
struct EventTargets<T> {
    identity: TypeId,
    counter: Option<T>,
    fields: Vec<Option<MetricTarget<T>>>,
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
#[display("cannot bind metrics on event `{event_name}`: field {index} declares invalid index {actual:?}")]
struct InvalidFieldIndex {
    event_name: &'static str,
    index: usize,
    actual: Option<usize>,
}

#[ohno::error]
#[display("event `{event_name}` visited metric field {actual} out of order (expected {expected:?})")]
struct InvalidFieldVisit {
    event_name: &'static str,
    expected: Option<usize>,
    actual: usize,
}

#[ohno::error]
#[display("event `{event_name}` has invalid metric field {slot}: nonnumeric measurement supplied")]
struct NonnumericMeasurement {
    event_name: &'static str,
    slot: usize,
}

#[ohno::error]
#[display("event `{event_name}` has invalid metric field {slot}: no measurement supplied")]
struct MissingMeasurement {
    event_name: &'static str,
    slot: usize,
}

#[ohno::error]
#[display(
    "event `{event_name}` has invalid metric field {slot}: numeric representation differs from its declaration (expected {expected:?}, got {actual:?})"
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
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::time::SystemTime;

    use ohno::{EnrichableExt as _, ErrorExt as _};

    use super::*;
    use crate::event;
    use crate::interop::DynEvent;
    use crate::processing::FieldVisitorFn;

    #[event("bound")]
    #[counter(value)]
    struct Bound {
        #[unredacted]
        value: u64,
    }

    #[event("empty")]
    #[info]
    struct Empty;

    struct Unidentified;

    impl Event for Unidentified {
        const DESCRIPTION: EventDescription = EventDescription::new("untyped").with_fields(Bound::DESCRIPTION.fields());

        fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }
    }

    // Supplies missing or deliberately incompatible values to the real recording path.
    struct ReadProbe(Option<Value>);

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
        fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            if let Some(value) = &self.0 {
                visitor(&Bound::DESCRIPTION.fields()[0], &|_| value.clone())?;
            }
            ControlFlow::Continue(())
        }
        fn description(&self) -> EventDescription {
            Bound::DESCRIPTION
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
            "event `request` has invalid metric field 2: no measurement supplied"
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
        let signed_value = ReadProbe(Some(Value::I64(-1)));
        let view = EventView::new_synthetic(&signed_value, SystemTime::UNIX_EPOCH);
        let mismatch = bindings.record(&view, |(), _| unreachable!()).unwrap_err();
        let cause = mismatch.find_source::<MismatchedRepresentation>().unwrap();
        assert_eq!(
            (cause.event_name, cause.slot, cause.expected, cause.actual),
            ("bound", 0, MetricValueKind::U64, MetricValueKind::I64),
        );
    }
}
