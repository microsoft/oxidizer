// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Whole-event registration, numeric delivery, and legacy processor compatibility.

#![expect(clippy::unwrap_used, reason = "test helpers use unwrap for informative failure backtraces")]
#![expect(clippy::panic, reason = "test adapters trip on unexpected getters or metric delivery")]

use std::any::TypeId;
use std::borrow::Cow;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use data_privacy::RedactionEngine;
use observed::__private::{event_description, event_fields, field_descriptor, log_field, metric_description, metric_field};
use observed::interop::{DynEvent, emit_dyn_event};
use observed::metadata::{EventDescription, FieldDescriptor, InstrumentKind, MetricDescription, MetricValueKind};
use observed::processing::{EventProcessor, EventView, FieldVisitorFn, MetricBindings};
use observed::{Event, FlushError, Sink, Value, emit, event};
use ohno::ErrorExt;
use tick::SimpleClock;

#[global_allocator]
#[cfg(not(miri))]
static ALLOCATOR: alloc_tracker::Allocator<std::alloc::System> = alloc_tracker::Allocator::system();

// Presentation assertions cover the primary diagnostic; typed causes are
// checked in the binding module, independently of nested backtrace rendering.
fn primary_message(error: &impl ErrorExt) -> String {
    error.message().lines().next().unwrap().to_owned()
}

#[event("request.completed")]
#[counter(name = "requests")]
#[counter(bytes_sent, name = "bytes_sent", unit = "By", desc = "completed request bytes")]
#[counter(packets, name = "packets")]
struct RequestCompleted {
    #[unredacted]
    bytes_sent: u32,
    #[unredacted]
    packets: u8,
}

#[event("another.request")]
#[counter(bytes_sent, name = "bytes_sent")]
struct AnotherRequest {
    #[unredacted]
    bytes_sent: u64,
}

#[event("request.disabled", disabled)]
#[counter(name = "requests")]
struct Disabled;

#[event("request.generic")]
#[counter(bytes_sent)]
struct Generic<T> {
    #[unredacted]
    bytes_sent: u16,
    _payload: T,
}

#[event("request.borrowed")]
#[counter(bytes_sent)]
struct Borrowed<'a> {
    #[unredacted]
    bytes_sent: usize,
    _payload: &'a str,
}

#[event("request.log")]
#[info]
struct LogOnly;

#[event("request.mixed")]
#[counter(bytes_sent)]
#[gauge(depth)]
struct Mixed {
    #[unredacted]
    bytes_sent: u32,
    #[unredacted]
    depth: u32,
}

/// Records prepared metric bindings into shared atomic accumulators.
struct Processor {
    bindings: MetricBindings<Arc<AtomicU64>>,
}

impl EventProcessor for Processor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        self.bindings.is_interested(description)
    }

    fn process(&self, event: &EventView<'_>) {
        self.bindings
            .record(event, |target, value| {
                let Value::U64(value) = value else {
                    panic!("this test processor records unsigned values")
                };
                target.fetch_add(value, Ordering::Relaxed);
            })
            .unwrap();
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

fn sink(bindings: MetricBindings<Arc<AtomicU64>>) -> Sink {
    Sink::builder("counters")
        .processors(vec![Arc::new(Processor { bindings })])
        .clock(SimpleClock::new_frozen())
        .build()
}

#[test]
fn registration_prepares_every_metric_with_declared_metadata_once() {
    let mut bindings = MetricBindings::new();
    let mut prepared = Vec::new();
    bindings
        .bind_event::<RequestCompleted>(|metric| {
            prepared.push((metric.instrument_name(), metric.kind(), metric.unit(), metric.description()));
        })
        .unwrap();
    assert_eq!(
        prepared,
        [
            ("requests", InstrumentKind::Counter, "", ""),
            ("bytes_sent", InstrumentKind::Counter, "By", "completed request bytes"),
            ("packets", InstrumentKind::Counter, "", ""),
        ],
    );
}

#[test]
fn emits_every_field_and_fieldless_counter_on_the_registered_event() {
    let targets: [Arc<AtomicU64>; 3] = std::array::from_fn(|_| Arc::new(AtomicU64::new(0)));
    let mut bindings = MetricBindings::new();
    let mut next = 0;
    bindings
        .bind_event::<RequestCompleted>(|_| {
            let target = Arc::clone(&targets[next]);
            next += 1;
            target
        })
        .unwrap();
    let sink = sink(bindings);
    emit!(
        sink,
        RequestCompleted {
            bytes_sent: 4096,
            packets: 3
        }
    );
    emit!(
        sink,
        RequestCompleted {
            bytes_sent: 512,
            packets: 7
        }
    );
    assert_eq!(targets.map(|target| target.load(Ordering::Relaxed)), [2, 4608, 10]);
}

#[test]
fn equal_instrument_names_on_different_events_remain_independent() {
    let first = Arc::new(AtomicU64::new(0));
    let second = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<AnotherRequest>(|_| Arc::clone(&second)).unwrap();
    bindings
        .bind_event::<RequestCompleted>(|metric| {
            if metric.instrument_name() == "bytes_sent" {
                Arc::clone(&first)
            } else {
                Arc::new(AtomicU64::new(0))
            }
        })
        .unwrap();
    let sink = sink(bindings);
    emit!(
        sink,
        RequestCompleted {
            bytes_sent: 41,
            packets: 1
        }
    );
    emit!(sink, AnotherRequest { bytes_sent: 1_u64 << 40 });
    assert_eq!((first.load(Ordering::Relaxed), second.load(Ordering::Relaxed)), (41, 1_u64 << 40),);
}

#[test]
fn sinks_register_the_same_event_with_independent_targets() {
    let first = Arc::new(AtomicU64::new(0));
    let second = Arc::new(AtomicU64::new(0));
    let mut a = MetricBindings::new();
    let mut b = MetricBindings::new();
    a.bind_event::<AnotherRequest>(|_| Arc::clone(&first)).unwrap();
    b.bind_event::<AnotherRequest>(|_| Arc::clone(&second)).unwrap();
    let a = sink(a);
    let b = sink(b);
    emit!(a, AnotherRequest { bytes_sent: 5 });
    emit!(b, AnotherRequest { bytes_sent: 9 });
    assert_eq!((first.load(Ordering::Relaxed), second.load(Ordering::Relaxed)), (5, 9));
}

#[test]
fn disabled_and_unbound_events_do_not_evaluate_the_expression() {
    let total = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<Disabled>(|_| Arc::clone(&total)).unwrap();
    let sink = sink(bindings);
    emit!(sink, {
        panic!("disabled event construction must not run");
        #[expect(unreachable_code, reason = "pins lazy construction")]
        Disabled
    });
    emit!(sink, {
        panic!("unbound event construction must not run");
        #[expect(unreachable_code, reason = "pins lazy construction")]
        AnotherRequest { bytes_sent: 1 }
    });
    assert_eq!(total.load(Ordering::Relaxed), 0);
}

#[test]
fn generic_and_borrowed_events_need_neither_clone_nor_any() {
    let total = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<Generic<AtomicU64>>(|_| Arc::clone(&total)).unwrap();
    bindings.bind_event::<Borrowed<'_>>(|_| Arc::clone(&total)).unwrap();
    let sink = sink(bindings);
    let text = "owned payload".to_owned();
    emit!(
        sink,
        Generic {
            bytes_sent: 7,
            _payload: AtomicU64::new(0)
        }
    );
    emit!(
        sink,
        Borrowed {
            bytes_sent: 11,
            _payload: &text
        }
    );
    assert_eq!(total.load(Ordering::Relaxed), 18);
}

/// Supplies an indexed measurement through the same visitor used by other processors.
struct NumericOnly {
    amount: u32,
}

impl Event for NumericOnly {
    const DESCRIPTION: EventDescription = event_description(
        "numeric",
        Some(TypeId::of::<Self>()),
        None,
        None,
        false,
        &event_fields([field_descriptor(
            "amount",
            None,
            Some(metric_field(
                "amount",
                Some(metric_description("numeric", InstrumentKind::Counter, "", "", MetricValueKind::U64)),
            )),
        )]),
    );

    fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        visitor(&Self::DESCRIPTION.fields()[0], &|_| Value::from(self.amount))
    }
}

#[test]
fn numeric_recording_uses_the_indexed_field_visitor() {
    let total = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<NumericOnly>(|_| Arc::clone(&total)).unwrap();
    let sink = sink(bindings);
    emit!(sink, NumericOnly { amount: 0 });
    emit!(sink, NumericOnly { amount: 123 });
    assert_eq!(total.load(Ordering::Relaxed), 123);
}

#[test]
fn optional_fields_keep_the_static_index_of_later_measurements() {
    #[event("optional.route")]
    #[info]
    #[counter(bytes, name = "bytes")]
    struct OptionalRoute {
        #[unredacted]
        #[if_none(drop)]
        route: Option<&'static str>,
        #[unredacted]
        bytes: u32,
    }
    let fields = OptionalRoute::DESCRIPTION.fields();
    assert_eq!(
        fields
            .iter()
            .map(|field| (field.field_index(), field.field_name()))
            .collect::<Vec<_>>(),
        [(Some(0), "route"), (Some(1), "bytes")],
    );
    let mut visited = Vec::new();
    let result = OptionalRoute { route: None, bytes: 7 }.visit_fields(&mut |field, _| {
        visited.push((field.field_index(), field.field_name()));
        ControlFlow::Continue(())
    });
    assert_eq!(result, ControlFlow::Continue(()));
    assert_eq!(visited, [(Some(1), "bytes")]);
    let total = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<OptionalRoute>(|_| Arc::clone(&total)).unwrap();
    let sink = sink(bindings);
    emit!(sink, OptionalRoute { route: None, bytes: 7 });
    emit!(
        sink,
        OptionalRoute {
            route: Some("/items"),
            bytes: 11
        }
    );
    assert_eq!(total.load(Ordering::Relaxed), 18);
}

#[test]
fn generated_visitors_use_trait_metadata_when_an_event_has_its_own_description_constant() {
    #[event("description.shadow")]
    #[counter(value)]
    struct Shadowed {
        #[unredacted]
        value: u32,
    }
    impl Shadowed {
        const DESCRIPTION: &str = "application metadata";
    }
    assert_eq!(Shadowed::DESCRIPTION, "application metadata");
    let total = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<Shadowed>(|_| Arc::clone(&total)).unwrap();
    emit!(sink(bindings), Shadowed { value: 17 });
    assert_eq!(total.load(Ordering::Relaxed), 17);
}

#[test]
fn bound_recording_never_evaluates_unbound_field_getters() {
    struct Guarded;
    impl Event for Guarded {
        const DESCRIPTION: EventDescription = event_description(
            "guarded",
            Some(TypeId::of::<Self>()),
            None,
            None,
            false,
            &event_fields([
                field_descriptor("payload", Some(log_field("payload")), None),
                field_descriptor(
                    "amount",
                    None,
                    Some(metric_field(
                        "amount",
                        Some(metric_description("amount", InstrumentKind::Counter, "", "", MetricValueKind::U64)),
                    )),
                ),
            ]),
        );
        fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            const FOREIGN_FIELDS: &[FieldDescriptor] = &event_fields([
                field_descriptor("first", Some(log_field("first")), None),
                field_descriptor("second", Some(log_field("second")), None),
                field_descriptor("foreign", Some(log_field("foreign")), None),
            ]);
            visitor(&Self::DESCRIPTION.fields()[0], &|_| panic!("unbound field getter"))?;
            visitor(&field_descriptor("runtime-only", Some(log_field("runtime-only")), None), &|_| {
                panic!("unindexed getter")
            })?;
            visitor(&FOREIGN_FIELDS[2], &|_| panic!("unknown getter"))?;
            visitor(&Self::DESCRIPTION.fields()[1], &|_| Value::U64(23))
        }
    }
    assert!(Guarded::DESCRIPTION.contains_metrics());
    assert!(!event_description("guarded", None, None, None, false, &[]).contains_metrics());
    assert_eq!(
        field_descriptor("enrichment", Some(log_field("enrichment")), None).field_index(),
        None
    );
    let total = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<Guarded>(|_| Arc::clone(&total)).unwrap();
    emit!(sink(bindings), Guarded);
    assert_eq!(total.load(Ordering::Relaxed), 23);
}

#[test]
fn runtime_field_inventory_determines_metric_presence() {
    const FIELDS: &[FieldDescriptor] = &event_fields([
        field_descriptor("label", Some(log_field("label")), None),
        field_descriptor("region", None, Some(metric_field("region", None))),
        field_descriptor(
            "amount",
            None,
            Some(metric_field(
                "amount",
                Some(metric_description("amount", InstrumentKind::Counter, "", "", MetricValueKind::U64)),
            )),
        ),
    ]);
    let name = std::hint::black_box("runtime");
    let description = event_description(name, None, None, None, false, FIELDS);
    assert!(description.contains_metrics());
    assert_eq!(description.fields().len(), 3);
    assert_eq!(description.fields()[2].field_index(), Some(2));
    assert!(!event_description(name, None, None, None, false, &FIELDS[..2]).contains_metrics());
}

#[test]
fn fieldless_counters_record_one_without_visiting_unneeded_fields() {
    struct CountOnly;
    impl Event for CountOnly {
        const DESCRIPTION: EventDescription = event_description(
            "count.only",
            Some(TypeId::of::<Self>()),
            None,
            Some(metric_description("count", InstrumentKind::Counter, "", "", MetricValueKind::U64)),
            false,
            &event_fields([field_descriptor("unused", Some(log_field("unused")), None)]),
        );
        fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            panic!("a fieldless counter needs no field values")
        }
    }
    let total = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<CountOnly>(|_| Arc::clone(&total)).unwrap();
    let sink = sink(bindings);
    emit!(sink, CountOnly);
    emit!(sink, CountOnly);
    assert_eq!(total.load(Ordering::Relaxed), 2);
}

#[test]
fn invalid_static_field_indices_fail_before_preparing_targets() {
    struct Unindexed;
    impl Event for Unindexed {
        const DESCRIPTION: EventDescription = event_description(
            "unindexed",
            Some(TypeId::of::<Self>()),
            None,
            None,
            false,
            &[field_descriptor(
                "value",
                None,
                Some(metric_field(
                    "value",
                    Some(metric_description("value", InstrumentKind::Counter, "", "", MetricValueKind::U64)),
                )),
            )],
        );
        fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }
    }
    let mut bindings = MetricBindings::<()>::new();
    let error = bindings
        .bind_event::<Unindexed>(|_| panic!("invalid metadata must fail before preparing targets"))
        .unwrap_err();
    assert_eq!(
        primary_message(&error),
        "cannot bind metrics on event `unindexed`: field 0 declares invalid index None"
    );
}

#[test]
fn standard_arc_processor_delegates_interest_and_delivery() {
    let total = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<AnotherRequest>(|_| Arc::clone(&total)).unwrap();
    bindings.bind_event::<NumericOnly>(|_| Arc::clone(&total)).unwrap();
    let processor = Arc::new(Processor { bindings });
    assert!(EventProcessor::is_interested(&processor, &AnotherRequest::DESCRIPTION));
    assert!(!EventProcessor::is_interested(&processor, &LogOnly::DESCRIPTION));
    let adapter = NumericAdapter(&NumericOnly { amount: 99 });
    let view = EventView::new_synthetic(&adapter, SystemTime::UNIX_EPOCH);
    EventProcessor::process(&processor, &view);
    assert_eq!(total.load(Ordering::Relaxed), 99);
    EventProcessor::flush(&processor).unwrap();
}

#[test]
fn unbound_events_are_ignored_and_bound_unsigned_getters_are_normalized() {
    let mut bindings = MetricBindings::<()>::default();
    let adapter = NumericAdapter(&NumericOnly { amount: 19 });
    let view = EventView::new_synthetic(&adapter, SystemTime::UNIX_EPOCH);
    bindings
        .record(&view, |(), _| panic!("unbound measurement must not be delivered"))
        .unwrap();
    bindings.bind_event::<NumericOnly>(|_| ()).unwrap();
    let mut values = Vec::new();
    bindings.record(&view, |(), value| values.push(value)).unwrap();
    assert_eq!(values, [Value::U64(19)]);
}

/// Declares numeric metadata but never visits its measurement.
struct MissingSlot;

impl Event for MissingSlot {
    const DESCRIPTION: EventDescription = event_description(
        "missing",
        Some(TypeId::of::<Self>()),
        None,
        None,
        false,
        NumericOnly::DESCRIPTION.fields(),
    );

    fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }
}

#[test]
fn missing_numeric_field_visits_are_reported() {
    struct Inspect(MetricBindings<()>);
    impl EventProcessor for Inspect {
        fn is_interested(&self, _: &EventDescription) -> bool {
            true
        }
        fn process(&self, event: &EventView<'_>) {
            let error = self
                .0
                .record(event, |(), _| panic!("missing slot must not be delivered"))
                .unwrap_err();
            assert_eq!(
                primary_message(&error),
                "event `missing` has invalid metric field 0: no measurement supplied"
            );
        }
        fn flush(&self) -> Result<(), FlushError> {
            Ok(())
        }
    }
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<MissingSlot>(|_| ()).unwrap();
    let sink = Sink::builder("inspect")
        .processors(vec![Arc::new(Inspect(bindings))])
        .clock(SimpleClock::new_frozen())
        .build();
    emit!(sink, MissingSlot);
}

/// Forwards the indexed-field fixture through the dynamic event boundary.
struct NumericAdapter<'a>(&'a NumericOnly);

impl DynEvent for NumericAdapter<'_> {
    fn name(&self) -> &'static str {
        "numeric"
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
        self.0.visit_fields(visitor)
    }
    fn description(&self) -> EventDescription {
        NumericOnly::DESCRIPTION
    }
}

#[test]
fn synthetic_views_and_dynamic_emission_forward_indexed_fields() {
    let total = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<NumericOnly>(|_| Arc::clone(&total)).unwrap();
    let event = NumericOnly { amount: 31 };
    let dynamic = NumericAdapter(&event);
    let view = EventView::new_synthetic(&dynamic, SystemTime::UNIX_EPOCH);
    bindings
        .record(&view, |target, value| {
            let Value::U64(value) = value else {
                panic!("adapter supplies unsigned values")
            };
            target.fetch_add(value, Ordering::Relaxed);
        })
        .unwrap();
    let disabled = EventView::new_synthetic(&DisabledAdapter, SystemTime::UNIX_EPOCH);
    bindings
        .record(&disabled, |_, _| panic!("disabled event must not be delivered"))
        .unwrap();
    emit_dyn_event(&sink(bindings), &dynamic);
    assert_eq!(total.load(Ordering::Relaxed), 62);
}

struct DisabledAdapter;

impl DynEvent for DisabledAdapter {
    fn name(&self) -> &'static str {
        "disabled"
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
        Disabled::DESCRIPTION
    }
}

#[test]
fn mixed_event_kinds_are_bound_and_empty_events_are_rejected() {
    let mut bindings = MetricBindings::<()>::new();
    bindings.bind_event::<Mixed>(|_| ()).unwrap();
    let empty = bindings
        .bind_event::<LogOnly>(|_| panic!("empty event must be rejected before preparation"))
        .unwrap_err();
    assert_eq!(
        (primary_message(&empty), bindings.is_interested(&Mixed::DESCRIPTION)),
        (
            "cannot bind metrics on event `request.log`: the event must declare at least one metric instrument".into(),
            true,
        ),
    );
}

#[test]
fn registering_an_event_twice_fails_without_changing_its_existing_targets() {
    let total = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<AnotherRequest>(|_| Arc::clone(&total)).unwrap();
    let error = bindings
        .bind_event::<AnotherRequest>(|_| panic!("duplicate registration must not prepare targets"))
        .unwrap_err();
    emit!(sink(bindings), AnotherRequest { bytes_sent: 123 });
    assert_eq!(
        (primary_message(&error), total.load(Ordering::Relaxed)),
        (
            "cannot bind metrics on event `another.request`: the event is already registered".into(),
            123
        ),
    );
}

struct Unidentified;

impl Event for Unidentified {
    const DESCRIPTION: EventDescription = event_description("unidentified", None, None, None, false, NumericOnly::DESCRIPTION.fields());
    fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }
}

#[test]
fn unidentified_events_are_rejected_during_binding() {
    let mut bindings = MetricBindings::<()>::new();
    assert!(!bindings.is_interested(&Unidentified::DESCRIPTION));
    assert_eq!(
        primary_message(&bindings.bind_event::<Unidentified>(|_| ()).unwrap_err()),
        "cannot bind metrics on event `unidentified`: a typed event identity is required",
    );
}

#[test]
#[cfg_attr(miri, ignore = "allocation tracker requires OS thread and clock APIs")]
fn counter_emission_does_not_allocate() {
    let total = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<AnotherRequest>(|_| Arc::clone(&total)).unwrap();
    let sink = sink(bindings);
    let session = alloc_tracker::Session::new().no_stdout().no_file();
    let operation = session.operation("counter_emission");
    {
        let _measurement = operation.measure_thread().iterations(100);
        for _ in 0..100 {
            emit!(sink, AnotherRequest { bytes_sent: 7 });
        }
    }
    let allocated = session
        .to_report()
        .operations()
        .find_map(|(name, operation)| (name == "counter_emission").then(|| operation.total_bytes_allocated()))
        .unwrap();
    assert_eq!((allocated, total.load(Ordering::Relaxed)), (0, 700));
}

#[test]
fn legacy_field_processors_and_bound_counters_receive_the_same_event() {
    struct Legacy(Arc<AtomicU64>);
    impl EventProcessor for Legacy {
        fn is_interested(&self, description: &EventDescription) -> bool {
            description.contains_metrics()
        }
        fn process(&self, event: &EventView<'_>) {
            let redactor = RedactionEngine::default();
            let _ = event.visit_fields(&mut |descriptor, get_value| {
                if descriptor.field_name() == "bytes_sent" {
                    let Value::U64(value) = get_value(&redactor) else {
                        panic!("legacy field getter must still supply its numeric value");
                    };
                    self.0.fetch_add(value, Ordering::Relaxed);
                }
                ControlFlow::Continue(())
            });
        }
        fn flush(&self) -> Result<(), FlushError> {
            Ok(())
        }
    }
    let bound = Arc::new(AtomicU64::new(0));
    let legacy = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<AnotherRequest>(|_| Arc::clone(&bound)).unwrap();
    let sink = Sink::builder("mixed")
        .processors(vec![Arc::new(Processor { bindings }), Arc::new(Legacy(Arc::clone(&legacy)))])
        .clock(SimpleClock::new_frozen())
        .build();
    emit!(sink, AnotherRequest { bytes_sent: 123 });
    assert_eq!((bound.load(Ordering::Relaxed), legacy.load(Ordering::Relaxed)), (123, 123));
}

#[test]
fn every_supported_unsigned_width_widens_without_losing_bits() {
    #[event("counter.widths")]
    #[counter(narrow8)]
    #[counter(narrow16)]
    #[counter(medium32)]
    #[counter(wide64)]
    #[counter(pointer)]
    struct Widths {
        #[unredacted]
        narrow8: u8,
        #[unredacted]
        narrow16: u16,
        #[unredacted]
        medium32: u32,
        #[unredacted]
        wide64: u64,
        #[unredacted]
        pointer: usize,
    }

    let targets: [Arc<AtomicU64>; 5] = std::array::from_fn(|_| Arc::new(AtomicU64::new(0)));
    let mut bindings = MetricBindings::new();
    let mut next = 0;
    bindings
        .bind_event::<Widths>(|_| {
            let target = Arc::clone(&targets[next]);
            next += 1;
            target
        })
        .unwrap();
    emit!(
        sink(bindings),
        Widths {
            narrow8: u8::MAX,
            narrow16: u16::MAX,
            medium32: u32::MAX,
            wide64: u64::MAX,
            pointer: usize::MAX,
        }
    );
    assert_eq!(
        targets.map(|target| target.load(Ordering::Relaxed)),
        [
            u64::from(u8::MAX),
            u64::from(u16::MAX),
            u64::from(u32::MAX),
            u64::MAX,
            usize::MAX as u64
        ],
    );
}

#[event("numeric.kinds")]
#[counter(name = "Count")]
#[updown_counter(small, name = "Small")]
#[updown_counter(medium, name = "Medium")]
#[updown_counter(large, name = "Large")]
#[updown_counter(wide, name = "Wide")]
#[updown_counter(pointer, name = "Pointer")]
#[gauge(level, name = "Level")]
#[histogram(duration, name = "Duration")]
struct NumericKinds {
    #[unredacted]
    small: i8,
    #[unredacted]
    medium: i16,
    #[unredacted]
    large: i32,
    #[unredacted]
    wide: i64,
    #[unredacted]
    pointer: isize,
    #[unredacted]
    level: u64,
    #[unredacted]
    duration: f32,
}

#[test]
fn visitor_recording_preserves_signed_deltas_gauges_and_float_observations() {
    struct Capture(MetricBindings<usize>, Arc<Mutex<Vec<Value>>>);
    impl EventProcessor for Capture {
        fn is_interested(&self, description: &EventDescription) -> bool {
            self.0.is_interested(description)
        }
        fn process(&self, event: &EventView<'_>) {
            let mut values = self.1.lock().unwrap();
            self.0
                .record(event, |index, value| {
                    assert_eq!(*index, values.len());
                    values.push(value);
                })
                .unwrap();
        }
        fn flush(&self) -> Result<(), FlushError> {
            Ok(())
        }
    }
    let values = Arc::new(Mutex::new(Vec::new()));
    let mut bindings = MetricBindings::new();
    let mut index = 0;
    bindings
        .bind_event::<NumericKinds>(|_| {
            let next = index;
            index += 1;
            next
        })
        .unwrap();
    let sink = Sink::builder("numeric")
        .processors(vec![Arc::new(Capture(bindings, Arc::clone(&values)))])
        .clock(SimpleClock::new_frozen())
        .build();
    emit!(
        sink,
        NumericKinds {
            small: i8::MIN,
            medium: i16::MIN,
            large: i32::MIN,
            wide: i64::MIN,
            pointer: isize::MIN,
            level: u64::MAX,
            duration: 1.5,
        }
    );
    assert_eq!(
        *values.lock().unwrap(),
        [
            Value::U64(1),
            Value::I64(i64::from(i8::MIN)),
            Value::I64(i64::from(i16::MIN)),
            Value::I64(i64::from(i32::MIN)),
            Value::I64(i64::MIN),
            Value::I64(isize::MIN as i64),
            Value::U64(u64::MAX),
            Value::F64(1.5),
        ]
    );
    assert_eq!(
        NumericKinds::DESCRIPTION
            .metric()
            .into_iter()
            .chain(
                NumericKinds::DESCRIPTION
                    .fields()
                    .iter()
                    .filter_map(|field| field.metric()?.instrument_description()),
            )
            .map(MetricDescription::value_kind)
            .collect::<Vec<_>>(),
        [
            MetricValueKind::U64,
            MetricValueKind::I64,
            MetricValueKind::I64,
            MetricValueKind::I64,
            MetricValueKind::I64,
            MetricValueKind::I64,
            MetricValueKind::U64,
            MetricValueKind::F64,
        ]
    );
}

#[test]
fn inconsistent_numeric_representation_is_reported_before_delivery() {
    struct WrongType;
    impl Event for WrongType {
        const DESCRIPTION: EventDescription = event_description(
            "wrong.type",
            Some(TypeId::of::<Self>()),
            None,
            None,
            false,
            NumericOnly::DESCRIPTION.fields(),
        );
        fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            visitor(&Self::DESCRIPTION.fields()[0], &|_| Value::I64(-1))
        }
    }
    struct Inspect(MetricBindings<()>);
    impl EventProcessor for Inspect {
        fn is_interested(&self, _: &EventDescription) -> bool {
            true
        }
        fn process(&self, event: &EventView<'_>) {
            let error = self
                .0
                .record(event, |(), _| panic!("wrong representation must not be delivered"))
                .unwrap_err();
            assert_eq!(
                primary_message(&error),
                "event `wrong.type` has invalid metric field 0: numeric representation differs from its declaration (expected U64, got I64)"
            );
        }
        fn flush(&self) -> Result<(), FlushError> {
            Ok(())
        }
    }
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<WrongType>(|_| ()).unwrap();
    let sink = Sink::builder("wrong.type")
        .processors(vec![Arc::new(Inspect(bindings))])
        .clock(SimpleClock::new_frozen())
        .build();
    emit!(sink, WrongType);
}

#[test]
fn nonnumeric_getters_and_incompatible_float_values_are_reported() {
    struct InvalidValue(Value);
    impl Event for InvalidValue {
        const DESCRIPTION: EventDescription = NumericOnly::DESCRIPTION;
        fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            visitor(&Self::DESCRIPTION.fields()[0], &|_| self.0.clone())
        }
    }
    impl DynEvent for InvalidValue {
        fn name(&self) -> &'static str {
            "numeric"
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
        fn description(&self) -> EventDescription {
            Self::DESCRIPTION
        }
        fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            Event::visit_fields(self, visitor)
        }
    }
    let mut bindings = MetricBindings::<()>::new();
    bindings.bind_event::<InvalidValue>(|_| ()).unwrap();
    for (value, expected) in [
        (Value::Bool(true), "nonnumeric measurement supplied"),
        (
            Value::F64(1.5),
            "numeric representation differs from its declaration (expected U64, got F64)",
        ),
    ] {
        let probe = InvalidValue(value);
        let view = EventView::new_synthetic(&probe, SystemTime::UNIX_EPOCH);
        let error = bindings
            .record(&view, |(), _| panic!("invalid value must not be delivered"))
            .unwrap_err();
        assert_eq!(
            primary_message(&error),
            format!("event `numeric` has invalid metric field 0: {expected}")
        );
    }
}

#[test]
fn normalized_values_follow_the_required_numeric_kind_without_losing_bits() {
    #[event("signed")]
    #[gauge(value)]
    struct Signed {
        #[unredacted]
        value: i64,
    }
    #[event("float")]
    #[histogram(value)]
    struct Float {
        #[unredacted]
        value: f64,
    }
    struct Probe(EventDescription, Value);
    impl DynEvent for Probe {
        fn name(&self) -> &'static str {
            self.0.name()
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
        fn description(&self) -> EventDescription {
            self.0
        }
        fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            visitor(&self.0.fields()[0], &|_| self.1.clone())
        }
    }
    let mut bindings = MetricBindings::<()>::new();
    bindings.bind_event::<NumericOnly>(|_| ()).unwrap();
    bindings.bind_event::<Signed>(|_| ()).unwrap();
    bindings.bind_event::<Float>(|_| ()).unwrap();
    for (description, supplied, expected) in [
        (NumericOnly::DESCRIPTION, Value::I64(0), Value::U64(0)),
        (NumericOnly::DESCRIPTION, Value::I64(i64::MAX), Value::U64(i64::MAX.cast_unsigned())),
        (NumericOnly::DESCRIPTION, Value::U64(u64::MAX), Value::U64(u64::MAX)),
        (Signed::DESCRIPTION, Value::I64(0), Value::I64(0)),
        (Signed::DESCRIPTION, Value::I64(i64::MAX), Value::I64(i64::MAX)),
        (Signed::DESCRIPTION, Value::I64(i64::MIN), Value::I64(i64::MIN)),
        (Float::DESCRIPTION, Value::F64(f64::MAX), Value::F64(f64::MAX)),
        (Float::DESCRIPTION, Value::F64(f64::NEG_INFINITY), Value::F64(f64::NEG_INFINITY)),
    ] {
        let probe = Probe(description, supplied);
        let view = EventView::new_synthetic(&probe, SystemTime::UNIX_EPOCH);
        let mut recorded = Vec::new();
        bindings.record(&view, |(), value| recorded.push(value)).unwrap();
        assert_eq!(recorded, [expected]);
    }
    let probe = Probe(Signed::DESCRIPTION, Value::U64(u64::MAX));
    let view = EventView::new_synthetic(&probe, SystemTime::UNIX_EPOCH);
    let error = bindings
        .record(&view, |(), _| panic!("incompatible unsigned value must not be delivered"))
        .unwrap_err();
    assert_eq!(
        primary_message(&error),
        "event `signed` has invalid metric field 0: numeric representation differs from its declaration (expected I64, got U64)"
    );
}

#[test]
fn repeated_and_reordered_metric_visits_do_not_update_the_wrong_target() {
    struct Visits(&'static [usize]);
    const VISIT_FIELDS: &[FieldDescriptor] = &event_fields([NumericOnly::DESCRIPTION.fields()[0], NumericOnly::DESCRIPTION.fields()[0]]);
    impl Event for Visits {
        const DESCRIPTION: EventDescription = event_description("visits", Some(TypeId::of::<Self>()), None, None, false, VISIT_FIELDS);
        fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            for &index in self.0 {
                visitor(&Self::DESCRIPTION.fields()[index], &|_| Value::U64(7))?;
            }
            ControlFlow::Continue(())
        }
    }
    impl DynEvent for Visits {
        fn name(&self) -> &'static str {
            "visits"
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
        fn description(&self) -> EventDescription {
            Self::DESCRIPTION
        }
        fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            Event::visit_fields(self, visitor)
        }
    }
    let mut bindings = MetricBindings::new();
    let mut index = 0;
    bindings
        .bind_event::<Visits>(|_| {
            let target = index;
            index += 1;
            target
        })
        .unwrap();
    for (indices, delivered) in [
        (&[1, 0][..], vec![]),
        (&[0, 0][..], vec![(0, Value::U64(7))]),
        (&[0, 1, 1][..], vec![(0, Value::U64(7)), (1, Value::U64(7))]),
    ] {
        let probe = Visits(indices);
        let view = EventView::new_synthetic(&probe, SystemTime::UNIX_EPOCH);
        let mut values = Vec::new();
        let error = bindings.record(&view, |target, value| values.push((*target, value))).unwrap_err();
        assert!(primary_message(&error).contains("out of order"));
        assert_eq!(values, delivered);
    }
}
