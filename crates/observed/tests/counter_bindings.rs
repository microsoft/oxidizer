// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Whole-event registration, numeric delivery, and legacy processor compatibility.

#![expect(clippy::unwrap_used, reason = "test helpers use unwrap for informative failure backtraces")]
#![expect(clippy::panic, reason = "test adapters trip if numeric delivery falls back to generic visitors")]

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use observed::metadata::{EventDescription, InstrumentKind, MetricDescription, MetricValue, MetricValueKind};
use observed::processing::{EventProcessor, EventView, FieldVisitorFn, MetricBindings};
use observed::{Event, FlushError, Sink, emit, event};
use ohno::ErrorExt as _;

#[global_allocator]
#[cfg(not(miri))]
static ALLOCATOR: alloc_tracker::Allocator<std::alloc::System> = alloc_tracker::Allocator::system();

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
                let MetricValue::U64(value) = value else {
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
    Sink::new("counters", vec![Arc::new(Processor { bindings })], tick::SimpleClock::new_frozen())
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

struct NumericOnly {
    amount: u32,
}

impl Event for NumericOnly {
    const DESCRIPTION: EventDescription = EventDescription::new("numeric", Some(std::any::TypeId::of::<Self>()), None, None, true, false);
    const METRICS: &'static [MetricDescription] = &[MetricDescription::builder("numeric", InstrumentKind::Counter)
        .value_kind(MetricValueKind::U64)
        .build()];

    fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        panic!("numeric bindings must never visit generic fields")
    }

    fn metric_value(&self, slot: usize) -> Option<MetricValue> {
        (slot == 0).then_some(MetricValue::U64(u64::from(self.amount)))
    }
}

#[test]
fn numeric_recording_does_not_visit_generic_fields() {
    let total = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<NumericOnly>(|_| Arc::clone(&total)).unwrap();
    emit!(sink(bindings), NumericOnly { amount: 123 });
    assert_eq!(total.load(Ordering::Relaxed), 123);
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
    let view = EventView::new_synthetic(&adapter, std::time::SystemTime::UNIX_EPOCH);
    EventProcessor::process(&processor, &view);
    assert_eq!(total.load(Ordering::Relaxed), 99);
    EventProcessor::flush(&processor).unwrap();
}

#[test]
fn unspecified_numeric_kind_accepts_measurements_and_unbound_events_are_ignored() {
    struct LegacyNumeric;
    impl Event for LegacyNumeric {
        const DESCRIPTION: EventDescription = NumericOnly::DESCRIPTION;
        const METRICS: &'static [MetricDescription] = &[MetricDescription::new("numeric", InstrumentKind::Counter, "", "")];
        fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }
    }
    let mut bindings = MetricBindings::<()>::default();
    let adapter = NumericAdapter(&NumericOnly { amount: 19 });
    let view = EventView::new_synthetic(&adapter, std::time::SystemTime::UNIX_EPOCH);
    bindings
        .record(&view, |(), _| panic!("unbound measurement must not be delivered"))
        .unwrap();
    bindings.bind_event::<LegacyNumeric>(|_| ()).unwrap();
    let mut values = Vec::new();
    bindings.record(&view, |(), value| values.push(value)).unwrap();
    assert_eq!(values, [MetricValue::U64(19)]);
}

struct MissingSlot;

impl Event for MissingSlot {
    const DESCRIPTION: EventDescription = EventDescription::new("missing", Some(std::any::TypeId::of::<Self>()), None, None, true, false);
    const METRICS: &'static [MetricDescription] = NumericOnly::METRICS;

    fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        panic!("numeric bindings must never visit generic fields")
    }
}

#[test]
fn missing_numeric_slots_are_reported() {
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
                error.message(),
                "event `missing` has invalid metric slot 0: no measurement supplied"
            );
        }
        fn flush(&self) -> Result<(), FlushError> {
            Ok(())
        }
    }
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<MissingSlot>(|_| ()).unwrap();
    let sink = Sink::new("inspect", vec![Arc::new(Inspect(bindings))], tick::SimpleClock::new_frozen());
    emit!(sink, MissingSlot);
}

struct NumericAdapter<'a>(&'a NumericOnly);

impl observed::interop::DynEvent for NumericAdapter<'_> {
    fn name(&self) -> &'static str {
        "numeric"
    }
    fn body(&self) -> Option<std::borrow::Cow<'static, str>> {
        None
    }
    fn source_file(&self) -> Option<std::borrow::Cow<'static, str>> {
        None
    }
    fn source_line(&self) -> Option<u32> {
        None
    }
    fn source_crate(&self) -> Option<std::borrow::Cow<'static, str>> {
        None
    }
    fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        panic!("numeric bindings must not visit adapter fields")
    }
    fn description(&self) -> EventDescription {
        NumericOnly::DESCRIPTION
    }
    fn metric_value(&self, slot: usize) -> Option<MetricValue> {
        self.0.metric_value(slot)
    }
}

#[test]
fn synthetic_views_and_dynamic_emission_forward_numeric_slots() {
    let total = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<NumericOnly>(|_| Arc::clone(&total)).unwrap();
    let event = NumericOnly { amount: 31 };
    let dynamic = NumericAdapter(&event);
    let view = EventView::new_synthetic(&dynamic, std::time::SystemTime::UNIX_EPOCH);
    bindings
        .record(&view, |target, value| {
            let MetricValue::U64(value) = value else {
                panic!("adapter supplies unsigned values")
            };
            target.fetch_add(value, Ordering::Relaxed);
        })
        .unwrap();
    let disabled = EventView::new_synthetic(&DisabledAdapter, std::time::SystemTime::UNIX_EPOCH);
    bindings
        .record(&disabled, |_, _| panic!("disabled event must not be delivered"))
        .unwrap();
    observed::interop::emit_dyn_event(&sink(bindings), &dynamic);
    assert_eq!(total.load(Ordering::Relaxed), 62);
}

struct DisabledAdapter;

impl observed::interop::DynEvent for DisabledAdapter {
    fn name(&self) -> &'static str {
        "disabled"
    }
    fn body(&self) -> Option<std::borrow::Cow<'static, str>> {
        None
    }
    fn source_file(&self) -> Option<std::borrow::Cow<'static, str>> {
        None
    }
    fn source_line(&self) -> Option<u32> {
        None
    }
    fn source_crate(&self) -> Option<std::borrow::Cow<'static, str>> {
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
        (empty.message(), bindings.is_interested(&Mixed::DESCRIPTION)),
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
        (error.message(), total.load(Ordering::Relaxed)),
        (
            "cannot bind metrics on event `another.request`: the event is already registered".into(),
            123
        ),
    );
}

struct Unidentified;

impl Event for Unidentified {
    const DESCRIPTION: EventDescription = EventDescription::new("unidentified", None, None, None, true, false);
    const METRICS: &'static [MetricDescription] = NumericOnly::METRICS;
    fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }
}

#[test]
fn unidentified_events_are_rejected_during_binding() {
    let mut bindings = MetricBindings::<()>::new();
    assert!(!bindings.is_interested(&Unidentified::DESCRIPTION));
    assert_eq!(
        bindings.bind_event::<Unidentified>(|_| ()).unwrap_err().message(),
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
            let redactor = data_privacy::RedactionEngine::default();
            let _ = event.visit_fields(&mut |descriptor, get_value| {
                if descriptor.field_name() == "bytes_sent" {
                    let observed::Value::U64(value) = get_value(&redactor) else {
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
    let sink = Sink::new(
        "mixed",
        vec![Arc::new(Processor { bindings }), Arc::new(Legacy(Arc::clone(&legacy)))],
        tick::SimpleClock::new_frozen(),
    );
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

#[test]
fn numeric_access_preserves_signed_deltas_gauges_and_legacy_float_observations() {
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

    struct Capture(MetricBindings<usize>, Arc<std::sync::Mutex<Vec<MetricValue>>>);
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
    let values = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut bindings = MetricBindings::new();
    let mut index = 0;
    bindings
        .bind_event::<NumericKinds>(|_| {
            let next = index;
            index += 1;
            next
        })
        .unwrap();
    let sink = Sink::new(
        "numeric",
        vec![Arc::new(Capture(bindings, Arc::clone(&values)))],
        tick::SimpleClock::new_frozen(),
    );
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
            MetricValue::U64(1),
            MetricValue::I64(i64::from(i8::MIN)),
            MetricValue::I64(i64::from(i16::MIN)),
            MetricValue::I64(i64::from(i32::MIN)),
            MetricValue::I64(i64::MIN),
            MetricValue::I64(isize::MIN as i64),
            MetricValue::U64(u64::MAX),
            MetricValue::F64(1.5),
        ]
    );
    assert_eq!(
        NumericKinds::METRICS.iter().map(MetricDescription::value_kind).collect::<Vec<_>>(),
        [
            Some(MetricValueKind::U64),
            Some(MetricValueKind::I64),
            Some(MetricValueKind::I64),
            Some(MetricValueKind::I64),
            Some(MetricValueKind::I64),
            Some(MetricValueKind::I64),
            Some(MetricValueKind::U64),
            Some(MetricValueKind::F64),
        ]
    );
}

#[test]
fn inconsistent_numeric_representation_is_reported_before_delivery() {
    struct WrongType;
    impl Event for WrongType {
        const DESCRIPTION: EventDescription =
            EventDescription::new("wrong.type", Some(std::any::TypeId::of::<Self>()), None, None, true, false);
        const METRICS: &'static [MetricDescription] = NumericOnly::METRICS;
        fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            panic!("numeric access must not visit general fields")
        }
        fn metric_value(&self, _: usize) -> Option<MetricValue> {
            Some(MetricValue::I64(-1))
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
                error.message(),
                "event `wrong.type` has invalid metric slot 0: numeric representation differs from its declaration"
            );
        }
        fn flush(&self) -> Result<(), FlushError> {
            Ok(())
        }
    }
    let mut bindings = MetricBindings::new();
    bindings.bind_event::<WrongType>(|_| ()).unwrap();
    let sink = Sink::new("wrong.type", vec![Arc::new(Inspect(bindings))], tick::SimpleClock::new_frozen());
    emit!(sink, WrongType);
}
