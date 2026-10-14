// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicU64, Ordering};

use observed::metadata::{EventDescription, InstrumentKind, MetricValue, MetricValueKind};
use observed::processing::{EventProcessor, EventView, MetricBindings};
use observed::{Event, FlushError};
use performables::arc::Arc;
use performables::sync::mutex::Mutex;

use crate::Error;
use crate::error::ErrorKind;
use crate::schema::{CounterSchema, CounterSpec};

#[derive(Debug)]
struct MetricTarget {
    index: usize,
    kind: InstrumentKind,
}

/// Builder for typed counters and one Windows category/instance.
///
/// Binding validation failures are retained and returned by `manifest`
/// or `build`, permitting the same fluent configuration in deployment tooling.
pub struct PerfCounterProcessorBuilder {
    category: String,
    instance: String,
    bindings: MetricBindings<MetricTarget>,
    specs: Vec<CounterSpec>,
    error: Option<Error>,
}

impl std::fmt::Debug for PerfCounterProcessorBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PerfCounterProcessorBuilder")
            .field("category", &self.category)
            .field("instance", &self.instance)
            .field("specs", &self.specs)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl Default for PerfCounterProcessorBuilder {
    fn default() -> Self {
        Self {
            category: String::new(),
            instance: String::new(),
            bindings: MetricBindings::new(),
            specs: Vec::new(),
            error: None,
        }
    }
}

impl PerfCounterProcessorBuilder {
    /// Sets the deployed Windows category name and provider identity.
    #[must_use]
    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = category.into();
        self
    }

    /// Sets the runtime instance name. This does not change deployment identities.
    #[must_use]
    pub fn instance(mut self, instance: impl Into<String>) -> Self {
        self.instance = instance.into();
        self
    }

    /// Registers every supported integer metric declared on an event.
    ///
    /// Names, descriptions, and units come from the event's metric attributes.
    /// Counters add unsigned increments, gauges replace unsigned readings, and
    /// up/down counters apply signed deltas to a non-negative `u64` total.
    /// Histograms, floats, signed gauges, and unknown numeric types are rejected.
    /// An unsupported instrument,
    /// an event without metrics, or duplicate registration is a startup error.
    /// Names must be unique across the category, including case variants.
    /// No redactor or general field visitor runs when recording values.
    #[must_use]
    pub fn bind_event<E: Event>(mut self) -> Self {
        if self.error.is_some() {
            return self;
        }
        for metric in E::METRICS {
            if !matches!(
                (metric.kind(), metric.value_kind()),
                (InstrumentKind::Counter | InstrumentKind::Gauge, Some(MetricValueKind::U64))
                    | (InstrumentKind::UpDownCounter, Some(MetricValueKind::I64))
            ) {
                self.error = Some(Error::configuration(format!(
                    "metric `{}` requires an unsigned counter or gauge, or a signed up/down counter",
                    metric.instrument_name(),
                )));
                return self;
            }
        }
        let specs = &mut self.specs;
        if let Err(source) = self.bindings.bind_event::<E>(|metric| {
            let source_index = specs.len();
            specs.push(CounterSpec {
                name: metric.instrument_name().into(),
                unit: metric.unit().into(),
                description: metric.description().into(),
                source_index,
                kind: metric.kind(),
            });
            MetricTarget {
                index: source_index,
                kind: metric.kind(),
            }
        }) {
            self.error = Some(ErrorKind::Binding { source: Arc::new(source) }.into());
        }
        self
    }

    fn schema(&self) -> Result<CounterSchema, Error> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        CounterSchema::new(&self.category, &self.instance, &self.specs)
    }

    /// Generates deployment XML without starting or installing a native provider.
    ///
    /// `application_identity` is the deployed `.dll`/`.exe` resource filename.
    /// Run Windows SDK `ctrpp` on the returned manifest to create its `.rc`.
    ///
    /// # Errors
    ///
    /// Rejects invalid configuration or an invalid resource binary filename.
    pub fn manifest(&self, application_identity: impl AsRef<str>) -> Result<String, Error> {
        self.schema()?.manifest(application_identity.as_ref())
    }

    /// Starts a native provider and registers stable backing storage once.
    ///
    /// Does not install machine-wide schema, resources, or EDS configuration.
    ///
    /// # Errors
    ///
    /// Returns an explicit unsupported-platform error outside 64-bit Windows,
    /// or a configuration/native startup error. Partial startup is cleaned up.
    pub fn build(self) -> Result<PerfCounterProcessor, Error> {
        let schema = self.schema()?;
        #[cfg(all(windows, target_pointer_width = "64"))]
        {
            let totals = (0..self.specs.len()).map(|_| AtomicU64::new(0)).collect::<Vec<_>>().into();
            let native = crate::native::Provider::start(&schema, &self.instance, Arc::clone(&totals))?;
            Ok(PerfCounterProcessor {
                bindings: self.bindings,
                schema,
                totals,
                error: Mutex::new(None),
                native,
            })
        }
        #[cfg(not(all(windows, target_pointer_width = "64")))]
        {
            drop(schema);
            Err(ErrorKind::UnsupportedPlatform.into())
        }
    }
}

/// Native Perflib V2 publishing of non-negative integer metrics.
///
/// Values start at zero per processor, accept concurrent emission, and never
/// wrap: overflow and underflow reject the update and are reported by `flush`.
/// Counter totals increase, up/down totals can decrease as far as zero, and
/// gauges store the latest unsigned reading.
///
/// Event type identities and callback state are local to this Rust executable
/// or library image. This is not a portable interface between Rust DLLs.
pub struct PerfCounterProcessor {
    bindings: MetricBindings<MetricTarget>,
    pub(crate) schema: CounterSchema,
    totals: Arc<[AtomicU64]>,
    error: Mutex<Option<Error>>,
    #[cfg(all(windows, target_pointer_width = "64"))]
    native: crate::native::Provider,
}

impl std::fmt::Debug for PerfCounterProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PerfCounterProcessor")
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

impl PerfCounterProcessor {
    /// Begins configuration without calling a Windows API.
    #[must_use]
    pub fn builder() -> PerfCounterProcessorBuilder {
        PerfCounterProcessorBuilder::default()
    }

    fn report(&self, error: &Error) {
        let poisoned = match self.error.lock_result() {
            Ok(mut pending) if pending.is_none() => {
                *pending = Some(error.clone());
                false
            }
            Ok(_) => return,
            Err(poisoned) => {
                drop(poisoned);
                true
            }
        };
        if poisoned {
            tracing::error!(error = %error, "observed_perf_counters.processing.poisoned");
        } else {
            tracing::error!(error = %error, "observed_perf_counters.processing.failed");
        }
    }

    fn record(&self, target: &MetricTarget, value: MetricValue) {
        let total = &self.totals[target.index];
        let failure = match (target.kind, value) {
            (InstrumentKind::Gauge, MetricValue::U64(value)) => {
                total.store(value, Ordering::Relaxed);
                None
            }
            (InstrumentKind::Counter, MetricValue::U64(delta)) => total
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add(delta))
                .err()
                .map(|_| false),
            (InstrumentKind::UpDownCounter, MetricValue::I64(delta)) => total
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add_signed(delta))
                .err()
                .map(|_| delta.is_negative()),
            _ => {
                self.report(&Error::configuration("measurement does not match the prepared integer metric"));
                return;
            }
        };
        if let Some(underflow) = failure {
            let counter_id = self
                .schema
                .counters
                .iter()
                .find(|counter| counter.source_index == target.index)
                .map_or(0, |counter| counter.id);
            let error = if underflow {
                ErrorKind::Underflow { counter_id }
            } else {
                ErrorKind::Overflow { counter_id }
            };
            self.report(&error.into());
        }
    }
}

impl EventProcessor for PerfCounterProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        self.bindings.is_interested(description)
    }

    fn process(&self, event: &EventView<'_>) {
        if let Err(source) = self.bindings.record(event, |target, value| self.record(target, value)) {
            self.report(&ErrorKind::Read { source: Arc::new(source) }.into());
        }
    }

    fn flush(&self) -> Result<(), FlushError> {
        if let Some(error) = self
            .error
            .lock_result()
            .map_err(|_poisoned| FlushError::new("observed_perf_counters", Error::from(ErrorKind::Poisoned)))?
            .as_ref()
        {
            return Err(FlushError::new("observed_perf_counters", error.clone()));
        }
        #[cfg(all(windows, target_pointer_width = "64"))]
        self.native
            .check()
            .map_err(|error| FlushError::new("observed_perf_counters", error))?;
        Ok(())
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use observed::{Sink, emit, event};

    use super::*;

    #[event("request.completed")]
    #[counter(name = "Requests")]
    #[counter(bytes_sent, name = "Bytes Sent", unit = "By")]
    struct RequestCompleted {
        #[unredacted]
        bytes_sent: u32,
    }

    #[cfg(all(windows, target_pointer_width = "64"))]
    #[event("huge.increment")]
    #[counter(value, name = "Huge")]
    struct HugeIncrement {
        #[unredacted]
        value: u64,
    }

    #[event("integer.measurements")]
    #[counter(bytes, name = "Bytes")]
    #[updown_counter(active, name = "Active")]
    #[gauge(depth, name = "Depth")]
    struct IntegerMeasurements {
        #[unredacted]
        bytes: u32,
        #[unredacted]
        active: i32,
        #[unredacted]
        depth: u32,
    }

    #[event("active.delta")]
    #[updown_counter(delta, name = "Active")]
    struct ActiveDelta {
        #[unredacted]
        delta: i64,
    }

    #[event("wide.gauge")]
    #[gauge(value, name = "Value")]
    struct WideGauge {
        #[unredacted]
        value: u64,
    }

    fn named_values(processor: &PerfCounterProcessor) -> Vec<(String, u64)> {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let values = crate::native::test_snapshot(&processor.native, &processor.schema);
        #[cfg(not(all(windows, target_pointer_width = "64")))]
        let values = processor
            .schema
            .counters
            .iter()
            .map(|counter| processor.totals[counter.source_index].load(Ordering::Relaxed));
        let mut named = processor
            .schema
            .counters
            .iter()
            .zip(values)
            .map(|(counter, value)| (counter.name.clone(), value))
            .collect::<Vec<_>>();
        named.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        named
    }

    fn test_sink(processor: &Arc<PerfCounterProcessor>) -> Sink {
        Sink::new(
            "integers",
            vec![Arc::<PerfCounterProcessor>::into_std_arc(Arc::clone(processor))],
            tick::SimpleClock::new_frozen(),
        )
    }

    #[test]
    fn overflow_diagnostics_identify_the_actual_metric_in_the_sorted_schema() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = build_for_test(
            PerfCounterProcessor::builder()
                .category("Metric identity")
                .instance("worker")
                .bind_event::<IntegerMeasurements>(),
        );
        let target = MetricTarget {
            index: 0,
            kind: InstrumentKind::Counter,
        };
        processor.record(&target, MetricValue::U64(u64::MAX));
        processor.record(&target, MetricValue::U64(1));
        let id = processor.schema.counters.iter().find(|counter| counter.name == "Bytes").unwrap().id;
        assert!(
            processor
                .flush()
                .unwrap_err()
                .to_string()
                .contains(&format!("metric {id} overflowed u64"))
        );
    }

    fn build_for_test(builder: PerfCounterProcessorBuilder) -> PerfCounterProcessor {
        #[cfg(all(windows, target_pointer_width = "64", not(miri)))]
        {
            builder.build().unwrap()
        }
        #[cfg(any(not(all(windows, target_pointer_width = "64")), miri))]
        {
            let schema = builder.schema().unwrap();
            let totals: Arc<[AtomicU64]> = (0..builder.specs.len()).map(|_| AtomicU64::new(0)).collect::<Vec<_>>().into();
            #[cfg(all(windows, target_pointer_width = "64", miri))]
            let native = crate::native::testing::start_mock(&schema, Arc::clone(&totals));
            PerfCounterProcessor {
                bindings: builder.bindings,
                schema,
                totals,
                error: Mutex::new(None),
                #[cfg(all(windows, target_pointer_width = "64", miri))]
                native,
            }
        }
    }

    #[test]
    fn reports_the_first_failure_and_rejects_inconsistent_measurements() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = build_for_test(
            PerfCounterProcessor::builder()
                .category("Error retention")
                .instance("worker")
                .bind_event::<WideGauge>(),
        );
        assert!(processor.is_interested(&WideGauge::DESCRIPTION));
        assert!(!processor.is_interested(&RequestCompleted::DESCRIPTION));
        processor.record(
            &MetricTarget {
                index: 0,
                kind: InstrumentKind::Gauge,
            },
            MetricValue::I64(-1),
        );
        let original = processor.flush().unwrap_err();
        let original = std::error::Error::source(&original).unwrap().to_string();
        assert!(original.contains("measurement does not match the prepared integer metric"));
        processor.report(&ErrorKind::Overflow { counter_id: 7 }.into());
        assert_eq!(
            std::error::Error::source(&processor.flush().unwrap_err()).unwrap().to_string(),
            original,
        );
        assert_eq!(named_values(&processor), [("Value".into(), 0)]);
        assert!(format!("{processor:?}").contains("Error retention"));
        assert_eq!(processor.schema.category, "Error retention");
        assert!(format!("{:?}", PerfCounterProcessor::builder()).contains("PerfCounterProcessorBuilder"));
    }

    #[test]
    fn poisoned_processor_state_surfaces_through_flush_and_diagnostics() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = build_for_test(
            PerfCounterProcessor::builder()
                .category("Poisoned processor")
                .instance("worker")
                .bind_event::<WideGauge>(),
        );
        let _panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = processor.error.lock();
            panic!("inject error-state poison");
        }));
        processor.report(&ErrorKind::Overflow { counter_id: 9 }.into());
        assert!(processor.flush().unwrap_err().to_string().contains("state lock was poisoned"));
    }

    #[test]
    fn processing_diagnostic_can_flush_the_sink_and_observe_the_saved_error() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = Arc::new(build_for_test(
            PerfCounterProcessor::builder()
                .category("Reentrant diagnostic")
                .instance("worker")
                .bind_event::<WideGauge>(),
        ));
        let sink = test_sink(&processor);
        let captured_processor = Arc::clone(&processor);
        let calls = Arc::new(AtomicU64::new(0));
        let captured_calls = Arc::clone(&calls);
        let subscriber = crate::testing::OnEvent(move || {
            assert!(matches!(captured_processor.error.try_lock_result(), Ok(Some(_))));
            assert!(sink.flush().unwrap_err().to_string().contains("metric 42 overflowed u64"));
            captured_calls.fetch_add(1, Ordering::Relaxed);
        });
        tracing::subscriber::with_default(subscriber, || {
            processor.report(&ErrorKind::Overflow { counter_id: 42 }.into());
            processor.report(&ErrorKind::Underflow { counter_id: 99 }.into());
        });
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(processor.flush().unwrap_err().to_string().contains("metric 42 overflowed u64"));
    }

    #[test]
    fn subscriber_panic_does_not_erase_the_first_error_or_poison_its_lock() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = build_for_test(
            PerfCounterProcessor::builder()
                .category("Panicking diagnostic")
                .instance("worker")
                .bind_event::<WideGauge>(),
        );
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tracing::subscriber::with_default(crate::testing::OnEvent(|| panic!("injected subscriber panic")), || {
                processor.report(&ErrorKind::Overflow { counter_id: 7 }.into());
            });
        }));
        assert!(panic.is_err());
        drop(processor.error.lock_result().unwrap());
        assert!(processor.flush().unwrap_err().to_string().contains("metric 7 overflowed u64"));
    }

    #[test]
    fn poisoned_processing_diagnostic_can_reenter_flush_without_holding_the_guard() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = Arc::new(build_for_test(
            PerfCounterProcessor::builder()
                .category("Reentrant poisoned diagnostic")
                .instance("worker")
                .bind_event::<WideGauge>(),
        ));
        let _panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = processor.error.lock();
            panic!("inject error-state poison");
        }));
        let captured_processor = Arc::clone(&processor);
        let calls = Arc::new(AtomicU64::new(0));
        let captured_calls = Arc::clone(&calls);
        let subscriber = crate::testing::OnEvent(move || {
            // A poison error owns the guard too; release it before reentering.
            drop(captured_processor.error.try_lock_result().unwrap_err());
            assert!(
                captured_processor
                    .flush()
                    .unwrap_err()
                    .to_string()
                    .contains("state lock was poisoned")
            );
            captured_calls.fetch_add(1, Ordering::Relaxed);
        });
        tracing::subscriber::with_default(subscriber, || {
            processor.report(&ErrorKind::Overflow { counter_id: 9 }.into());
        });
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    struct MissingSlot;

    impl Event for MissingSlot {
        const DESCRIPTION: EventDescription =
            EventDescription::new("missing.slot", Some(std::any::TypeId::of::<Self>()), None, None, true, false);
        const METRICS: &'static [observed::metadata::MetricDescription] =
            &[observed::metadata::MetricDescription::builder("Missing", InstrumentKind::Counter)
                .value_kind(MetricValueKind::U64)
                .build()];
        fn visit_fields(&self, _: &mut observed::processing::FieldVisitorFn<'_>) -> std::ops::ControlFlow<()> {
            std::ops::ControlFlow::Continue(())
        }
    }

    impl observed::interop::DynEvent for MissingSlot {
        fn name(&self) -> &'static str {
            "missing.slot"
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
        fn description(&self) -> EventDescription {
            Self::DESCRIPTION
        }
        fn visit_fields(&self, _: &mut observed::processing::FieldVisitorFn<'_>) -> std::ops::ControlFlow<()> {
            std::ops::ControlFlow::Continue(())
        }
    }

    #[test]
    fn malformed_measurements_preserve_the_numeric_read_error_source() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = build_for_test(
            PerfCounterProcessor::builder()
                .category("Read error")
                .instance("worker")
                .bind_event::<MissingSlot>(),
        );
        processor.process(&EventView::new_synthetic(&MissingSlot, std::time::SystemTime::UNIX_EPOCH));
        let error = processor.flush().unwrap_err();
        let backend = std::error::Error::source(&error).unwrap().downcast_ref::<Error>().unwrap();
        let source = std::error::Error::source(backend).unwrap();
        assert!(source.is::<observed::processing::MetricReadError>());
        assert_eq!(
            ohno::ErrorExt::message(source.downcast_ref::<observed::processing::MetricReadError>().unwrap()),
            "event `missing.slot` has invalid metric slot 0: no measurement supplied"
        );
    }
    #[test]
    fn integer_updates_publish_add_adjust_and_replace_semantics() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = Arc::new(build_for_test(
            PerfCounterProcessor::builder()
                .category("Observed integer updates")
                .instance("worker")
                .bind_event::<IntegerMeasurements>(),
        ));
        let sink = test_sink(&processor);
        emit!(
            sink,
            IntegerMeasurements {
                bytes: 100,
                active: 3,
                depth: 10
            }
        );
        assert_eq!(
            named_values(&processor),
            [("Active".into(), 3), ("Bytes".into(), 100), ("Depth".into(), 10)]
        );
        emit!(
            sink,
            IntegerMeasurements {
                bytes: 200,
                active: -2,
                depth: 7
            }
        );
        assert_eq!(
            named_values(&processor),
            [("Active".into(), 1), ("Bytes".into(), 300), ("Depth".into(), 7)]
        );
        emit!(
            sink,
            IntegerMeasurements {
                bytes: 0,
                active: -1,
                depth: 0
            }
        );
        assert_eq!(
            named_values(&processor),
            [("Active".into(), 0), ("Bytes".into(), 300), ("Depth".into(), 0)]
        );
        sink.flush().unwrap();
    }

    #[test]
    fn subtraction_below_zero_retains_previous_value_and_other_metrics_continue() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = Arc::new(build_for_test(
            PerfCounterProcessor::builder()
                .category("Observed underflow")
                .instance("worker")
                .bind_event::<IntegerMeasurements>(),
        ));
        let sink = test_sink(&processor);
        emit!(
            sink,
            IntegerMeasurements {
                bytes: 100,
                active: 1,
                depth: 10
            }
        );
        emit!(
            sink,
            IntegerMeasurements {
                bytes: 200,
                active: -2,
                depth: 7
            }
        );
        assert_eq!(
            named_values(&processor),
            [("Active".into(), 1), ("Bytes".into(), 300), ("Depth".into(), 7)]
        );
        assert!(sink.flush().unwrap_err().to_string().contains("below zero"));
        emit!(
            sink,
            IntegerMeasurements {
                bytes: 0,
                active: -1,
                depth: 0
            }
        );
        assert_eq!(
            named_values(&processor),
            [("Active".into(), 0), ("Bytes".into(), 300), ("Depth".into(), 0)]
        );
        assert!(sink.flush().unwrap_err().to_string().contains("below zero"));
    }

    #[test]
    fn signed_extremes_are_checked_without_negation_overflow() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = Arc::new(build_for_test(
            PerfCounterProcessor::builder()
                .category("Observed signed extremes")
                .instance("worker")
                .bind_event::<ActiveDelta>(),
        ));
        let sink = test_sink(&processor);
        emit!(sink, ActiveDelta { delta: i64::MIN });
        assert_eq!(named_values(&processor), [("Active".into(), 0)]);
        assert!(sink.flush().unwrap_err().to_string().contains("below zero"));
        emit!(sink, ActiveDelta { delta: i64::MAX });
        emit!(sink, ActiveDelta { delta: i64::MAX });
        emit!(sink, ActiveDelta { delta: i64::MIN });
        assert_eq!(named_values(&processor), [("Active".into(), (1_u64 << 63) - 2)]);
        emit!(sink, ActiveDelta { delta: i64::MAX });
        emit!(sink, ActiveDelta { delta: 2 });
        assert_eq!(named_values(&processor), [("Active".into(), u64::MAX)]);
    }

    #[test]
    fn upward_overflow_retains_previous_value_and_surfaces_through_flush() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = Arc::new(build_for_test(
            PerfCounterProcessor::builder()
                .category("Observed updown overflow")
                .instance("worker")
                .bind_event::<ActiveDelta>(),
        ));
        let sink = test_sink(&processor);
        emit!(sink, ActiveDelta { delta: i64::MAX });
        emit!(sink, ActiveDelta { delta: i64::MAX });
        emit!(sink, ActiveDelta { delta: 1 });
        emit!(sink, ActiveDelta { delta: 1 });
        assert_eq!(named_values(&processor), [("Active".into(), u64::MAX)]);
        assert!(sink.flush().unwrap_err().to_string().contains("overflowed"));
    }

    #[test]
    fn gauges_accept_the_full_unsigned_range_without_accumulating() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = Arc::new(build_for_test(
            PerfCounterProcessor::builder()
                .category("Observed wide gauge")
                .instance("worker")
                .bind_event::<WideGauge>(),
        ));
        let sink = test_sink(&processor);
        emit!(sink, WideGauge { value: u64::MAX });
        assert_eq!(named_values(&processor), [("Value".into(), u64::MAX)]);
        emit!(sink, WideGauge { value: 7 });
        assert_eq!(named_values(&processor), [("Value".into(), 7)]);
        sink.flush().unwrap();
    }

    #[test]
    fn concurrent_integer_updates_do_not_lose_deltas_or_wrap() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = Arc::new(build_for_test(
            PerfCounterProcessor::builder()
                .category("Observed concurrent integers")
                .instance("worker")
                .bind_event::<IntegerMeasurements>(),
        ));
        let sink = test_sink(&processor);
        let iterations = if cfg!(miri) { 8 } else { 1000 };
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let sink = &sink;
                scope.spawn(move || {
                    for _ in 0..iterations {
                        emit!(
                            sink,
                            IntegerMeasurements {
                                bytes: 37,
                                active: 1,
                                depth: 10
                            }
                        );
                        emit!(
                            sink,
                            IntegerMeasurements {
                                bytes: 0,
                                active: -1,
                                depth: 7
                            }
                        );
                    }
                });
            }
        });
        emit!(
            sink,
            IntegerMeasurements {
                bytes: 0,
                active: 0,
                depth: 3
            }
        );
        assert_eq!(
            named_values(&processor),
            [("Active".into(), 0), ("Bytes".into(), 4 * iterations * 37), ("Depth".into(), 3)]
        );
        sink.flush().unwrap();
    }

    #[test]
    #[cfg(all(windows, target_pointer_width = "64"))]
    #[cfg_attr(miri, ignore = "Perflib FFI runs natively; snapshot storage is tested separately under Miri")]
    fn concurrent_typed_emission_publishes_exact_native_values_and_rejects_overflow() {
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = Arc::new(
            PerfCounterProcessor::builder()
                .category("Observed typed native test")
                .instance("worker-中文🦀")
                .bind_event::<RequestCompleted>()
                .bind_event::<HugeIncrement>()
                .build()
                .unwrap(),
        );
        let sink = Sink::new(
            "app",
            vec![Arc::<PerfCounterProcessor>::into_std_arc(Arc::clone(&processor))],
            tick::SimpleClock::new_frozen(),
        );
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let sink = &sink;
                scope.spawn(move || {
                    for _ in 0..1000 {
                        emit!(sink, RequestCompleted { bytes_sent: 37 });
                    }
                });
            }
            for _ in 0..20 {
                let snapshot = crate::native::test_snapshot(&processor.native, &processor.schema);
                assert!(snapshot.into_iter().all(|value| value <= 148_000));
            }
        });
        let expected = processor
            .schema
            .counters
            .iter()
            .map(|counter| match counter.name.as_str() {
                "Huge" => 0,
                "Requests" => 4000,
                _ => 148_000,
            })
            .collect::<Vec<_>>();
        assert_eq!(crate::native::test_snapshot(&processor.native, &processor.schema), expected);
        sink.flush().unwrap();
        emit!(sink, HugeIncrement { value: u64::MAX });
        emit!(sink, HugeIncrement { value: 1 });
        let expected = processor
            .schema
            .counters
            .iter()
            .map(|counter| match counter.name.as_str() {
                "Huge" => u64::MAX,
                "Requests" => 4000,
                _ => 148_000,
            })
            .collect::<Vec<_>>();
        assert_eq!(crate::native::test_snapshot(&processor.native, &processor.schema), expected);
        assert!(sink.flush().unwrap_err().to_string().contains("overflowed"));
        assert!(sink.flush().unwrap_err().to_string().contains("overflowed"));
    }
}
