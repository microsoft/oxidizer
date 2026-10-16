// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use observed::metadata::{EventDescription, InstrumentKind, MetricValueKind};
use observed::processing::{EventProcessor, EventView, MetricBindings};
use observed::{Event, FlushError, Value};
use performables::arc::Arc;
use performables::sync::mutex::Mutex;

use crate::Error;
#[cfg(not(all(windows, target_pointer_width = "64")))]
use crate::error::UnsupportedPlatform;
use crate::error::configuration::{EmptyField, ManifestDefinition, MeasurementMismatch, UnsupportedMetric};
use crate::error::{ManifestRead, Overflow, Poisoned, Underflow};
#[cfg(all(windows, target_pointer_width = "64"))]
use crate::native::Provider;
use crate::schema::{CounterSchema, CounterSpec, Manifest};

// Bound both input storage and XML parsing work during startup.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// Builder for typed counters and one Windows category/instance.
///
/// Load an externally authored `.man` file, select its counter set by category
/// name, and bind Rust instruments to matching declared counter names.
/// Binding validation failures are retained and returned by `build`.
pub struct PerfCounterProcessorBuilder {
    category: String,
    instance: String,
    bindings: MetricBindings<MetricTarget>,
    specs: Vec<CounterSpec>,
    manifest: Option<Manifest>,
    error: Option<Error>,
}

impl Debug for PerfCounterProcessorBuilder {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>().rsplit("::").next().unwrap_or_else(type_name::<Self>))
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
            manifest: None,
            error: None,
        }
    }
}

impl PerfCounterProcessorBuilder {
    /// Reads and parses a Windows V2 `.man` definition file during startup.
    ///
    /// The supplied provider and counter-set identifiers remain authoritative.
    /// This does not install definitions, load the resource DLL, or start a
    /// native provider. The file is read once; emission never reads it.
    ///
    /// Supports UTF-8 XML with inline names and user-mode, multiple-instance
    /// sets of zero-scale, standard-detail `perf_counter_large_rawcount` reference
    /// counters with `callback="custom"`. Other formats are rejected rather than
    /// silently reinterpreted. Parsing checks this backend profile, not the full
    /// Windows SDK schema; deployment should also validate with `ctrpp`.
    /// Files larger than one MiB and document type declarations are rejected.
    ///
    /// # Errors
    ///
    /// Returns a path-bearing I/O error or an explicit invalid-manifest error.
    pub fn manifest_file(mut self, path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let mut xml = String::new();
        File::open(path)
            .and_then(|file| file.take(MAX_MANIFEST_BYTES + 1).read_to_string(&mut xml))
            .map_err(|source| Error::from(ManifestRead::caused_by(path.to_path_buf(), source)))?;
        if xml.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(Error::configuration(ManifestDefinition::new("manifest exceeds one MiB")));
        }
        self.manifest = Some(Manifest::parse(&xml)?);
        Ok(self)
    }

    /// Selects a counter set by its category name in the supplied manifest.
    ///
    /// Does not derive or override deployment identities. Unknown or ambiguous
    /// category names are rejected by `build`.
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
    /// Each event instrument name must exactly match one counter name in the
    /// selected set. The manifest supplies IDs, names, and Windows formats;
    /// Rust attributes supply the aggregation operation, not deployment metadata.
    /// Counters add unsigned increments, gauges replace unsigned readings, and
    /// up/down counters apply signed deltas to a non-negative `u64` total.
    /// Histograms, floats, and signed gauges are rejected.
    /// An unsupported instrument,
    /// an event without metrics, or duplicate registration is a startup error.
    /// Names must be unique across the category, including case variants.
    /// A selected set must contain between one and 256 counters. Unbound declared
    /// counters remain at zero; every instrument on a bound event must be mapped.
    /// Recording visits event fields but evaluates only bound numeric getters,
    /// without redaction or enrichment visits.
    #[must_use]
    pub fn bind_event<E: Event>(mut self) -> Self {
        if self.error.is_some() {
            return self;
        }
        let description = E::DESCRIPTION;
        for metric in description.metric().into_iter().chain(
            description
                .fields()
                .iter()
                .filter_map(|field| field.metric()?.instrument_description()),
        ) {
            if !matches!(
                (metric.kind(), metric.value_kind()),
                (InstrumentKind::Counter | InstrumentKind::Gauge, MetricValueKind::U64)
                    | (InstrumentKind::UpDownCounter, MetricValueKind::I64)
            ) {
                self.error = Some(Error::configuration(UnsupportedMetric::new(
                    metric.instrument_name(),
                    metric.kind(),
                    metric.value_kind(),
                )));
                return self;
            }
        }
        let specs = &mut self.specs;
        if let Err(source) = self.bindings.bind_event::<E>(|metric| {
            let source_index = specs.len();
            specs.push(CounterSpec {
                name: metric.instrument_name().into(),
                source_index,
            });
            MetricTarget {
                index: source_index,
                kind: metric.kind(),
            }
        }) {
            self.error = Some(Error::caused_by(source));
        }
        self
    }

    // Reuse the first binding failure before performing shared deployment validation.
    fn schema(&self) -> Result<CounterSchema, Error> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let manifest = self
            .manifest
            .as_ref()
            .ok_or_else(|| Error::configuration(EmptyField::new("manifest")))?;
        CounterSchema::from_manifest(manifest, &self.category, &self.instance, &self.specs)
    }

    /// Starts a native provider and registers stable backing storage once.
    ///
    /// Does not install machine-wide schema, resources, or collection-service configuration.
    ///
    /// # Errors
    ///
    /// Returns an explicit unsupported-platform error outside 64-bit Windows,
    /// or a configuration/native startup error. Partial startup is cleaned up.
    pub fn build(self) -> Result<PerfCounterProcessor, Error> {
        let schema = self.schema()?;
        #[cfg(all(windows, target_pointer_width = "64"))]
        {
            let totals = (0..schema.counters.len()).map(|_| AtomicU64::new(0)).collect::<Vec<_>>().into();
            let native = Provider::start(&schema, &self.instance, Arc::clone(&totals))?;
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
            Err(UnsupportedPlatform::new().into())
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
    native: Provider,
}

impl Debug for PerfCounterProcessor {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>().rsplit("::").next().unwrap_or_else(type_name::<Self>))
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

    // Retain the first failure before diagnostics can reenter the processor.
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

    // Apply one checked aggregation; rejected arithmetic leaves its prior total intact.
    fn record(&self, target: &MetricTarget, value: &Value) {
        let total = &self.totals[target.index];
        let failure = match (target.kind, value) {
            (InstrumentKind::Gauge, Value::U64(value)) => {
                total.store(*value, Ordering::Relaxed);
                None
            }
            (InstrumentKind::Counter, Value::U64(delta)) => total
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add(*delta))
                .err()
                .map(|_| false),
            (InstrumentKind::UpDownCounter, Value::I64(delta)) => total
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add_signed(*delta))
                .err()
                .map(|_| delta.is_negative()),
            _ => {
                let value_kind = match value {
                    Value::U64(_) => Some(MetricValueKind::U64),
                    Value::I64(_) => Some(MetricValueKind::I64),
                    Value::F64(_) => Some(MetricValueKind::F64),
                    _ => None,
                };
                self.report(&Error::configuration(MeasurementMismatch::new(target.kind, value_kind)));
                return;
            }
        };
        if let Some(underflow) = failure {
            let counter_id = self
                .schema
                .counters
                .iter()
                .find(|counter| counter.source_index == target.index)
                .map_or(0, |counter| counter.id.get());
            let error = if underflow {
                Error::from(Underflow::new(counter_id))
            } else {
                Error::from(Overflow::new(counter_id))
            };
            self.report(&error);
        }
    }
}

impl EventProcessor for PerfCounterProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        self.bindings.is_interested(description)
    }

    fn process(&self, event: &EventView<'_>) {
        if let Err(source) = self.bindings.record(event, |target, value| self.record(target, &value)) {
            self.report(&Error::caused_by(source));
        }
    }

    fn flush(&self) -> Result<(), FlushError> {
        if let Some(error) = self
            .error
            .lock_result()
            .map_err(|_poisoned| FlushError::new("observed_perf_counters", Error::from(Poisoned::new())))?
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

// Prepared slot destination: its atomic total and the operation applied to it.
#[derive(Debug)]
struct MetricTarget {
    index: usize,
    kind: InstrumentKind,
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::borrow::Cow;
    use std::ops::ControlFlow;
    use std::panic::{AssertUnwindSafe, RefUnwindSafe, UnwindSafe, catch_unwind};
    use std::time::SystemTime;

    use observed::interop::DynEvent;
    use observed::processing::{FieldVisitorFn, MetricReadError};
    use observed::{Sink, emit, event};
    use tick::SimpleClock;

    use super::*;

    // Both types may retain arbitrary ohno sources; catching a panic requires
    // the caller to decide whether the surrounding operation can be recovered.
    static_assertions::assert_not_impl_any!(PerfCounterProcessor: UnwindSafe, RefUnwindSafe);
    static_assertions::assert_not_impl_any!(PerfCounterProcessorBuilder: UnwindSafe, RefUnwindSafe);

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

    #[test]
    fn supplied_guids_and_counter_ids_map_to_typed_slots_without_regenerating_metadata() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        // Inject the parsed file content so the same native-layout test runs
        // under Miri isolation; public file-system loading is tested separately.
        let mut builder = PerfCounterProcessor::builder();
        builder.manifest = Some(Manifest::parse(include_str!("../tests/test_data/ConsumerCounters.man")).unwrap());
        let builder = builder
            .category("Observed Windows consumer test <&\"'> 中文🦀")
            .instance("mapped")
            .bind_event::<RequestCompleted>();
        let processor = Arc::new(build_for_test(builder));
        assert_eq!(processor.schema.provider_id.as_u128(), 0x9fc72d85_9d1e_4c4b_a031_6d9288c3b405);
        assert_eq!(processor.schema.counter_set_id.as_u128(), 0x1b623456_789a_4bcd_8ef0_123456789abc);
        assert_eq!(
            processor.schema.counters.iter().map(|counter| counter.id.get()).collect::<Vec<_>>(),
            [2, 7, 13, 31, 41, 53, 59],
        );
        let sink = test_sink(&processor);
        emit!(sink, RequestCompleted { bytes_sent: 37 });
        assert_eq!(
            named_values(&processor),
            [
                ("Active Requests".into(), 0),
                ("Bytes Sent".into(), 37),
                ("Disabled".into(), 0),
                ("Huge".into(), 0),
                ("Malformed".into(), 0),
                ("Queue Depth".into(), 0),
                ("Requests".into(), 1),
            ],
        );
        sink.flush().unwrap();
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
            SimpleClock::new_frozen(),
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
        processor.record(&target, &Value::U64(u64::MAX));
        processor.record(&target, &Value::U64(1));
        let id = processor.schema.counters.iter().find(|counter| counter.name == "Bytes").unwrap().id;
        assert!(
            processor
                .flush()
                .unwrap_err()
                .to_string()
                .contains(&format!("metric {id} overflowed u64"))
        );
    }

    // Assemble local aggregation tests without acquiring real Windows providers.
    fn build_for_test(mut builder: PerfCounterProcessorBuilder) -> PerfCounterProcessor {
        {
            if builder.manifest.is_none() {
                let names = builder.specs.iter().map(|spec| spec.name.as_str()).collect::<Vec<_>>();
                builder.manifest = Some(Manifest::parse(&crate::testing::manifest(&builder.category, &names)).unwrap());
            }
            let schema = builder.schema().unwrap();
            let totals: Arc<[AtomicU64]> = (0..schema.counters.len()).map(|_| AtomicU64::new(0)).collect::<Vec<_>>().into();
            #[cfg(all(windows, target_pointer_width = "64"))]
            let native = crate::native::testing::start_mock(&schema, Arc::clone(&totals));
            PerfCounterProcessor {
                bindings: builder.bindings,
                schema,
                totals,
                error: Mutex::new(None),
                #[cfg(all(windows, target_pointer_width = "64"))]
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
            &Value::I64(-1),
        );
        let original = processor.flush().unwrap_err();
        let original = std::error::Error::source(&original).unwrap().to_string();
        assert!(original.contains("measurement does not match the prepared integer metric"));
        processor.report(&Overflow::new(7_u32).into());
        assert_eq!(
            std::error::Error::source(&processor.flush().unwrap_err()).unwrap().to_string(),
            original,
        );
        assert_eq!(named_values(&processor), [("Value".into(), 0)]);
        assert!(format!("{processor:?}").contains("Error retention"));
        assert_eq!(processor.schema.category, "Error retention");
        assert!(
            format!("{:?}", PerfCounterProcessor::builder())
                .contains(type_name::<PerfCounterProcessorBuilder>().rsplit("::").next().unwrap())
        );
    }

    #[test]
    fn incompatible_values_report_only_representation_and_keep_the_previous_total() {
        #[cfg(all(windows, target_pointer_width = "64"))]
        let _native_tests = crate::native::TEST_NATIVE.lock();
        for (kind, value, expected) in [
            (InstrumentKind::Counter, Value::I64(-1), Some(MetricValueKind::I64)),
            (InstrumentKind::UpDownCounter, Value::U64(u64::MAX), Some(MetricValueKind::U64)),
            (InstrumentKind::Gauge, Value::F64(1.5), Some(MetricValueKind::F64)),
            (InstrumentKind::Gauge, Value::String("not a measurement".into()), None),
        ] {
            let processor = build_for_test(
                PerfCounterProcessor::builder()
                    .category("Mismatch")
                    .instance("worker")
                    .bind_event::<WideGauge>(),
            );
            processor.record(&MetricTarget { index: 0, kind }, &value);
            let error = processor.error.lock().as_ref().unwrap().clone();
            let cause = ohno::ErrorExt::find_source::<MeasurementMismatch>(&error).unwrap();
            assert_eq!((cause.kind, cause.value_kind), (kind, expected));
            assert_eq!(named_values(&processor), [("Value".into(), 0)]);
            assert!(!error.to_string().contains("not a measurement"));
        }
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
        let _panic = catch_unwind(AssertUnwindSafe(|| {
            let _guard = processor.error.lock();
            panic!("inject error-state poison");
        }));
        processor.report(&Overflow::new(9_u32).into());
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
        let calls = Arc::new(AtomicU64::new(0));
        let subscriber = crate::testing::OnEvent({
            let processor = Arc::clone(&processor);
            let calls = Arc::clone(&calls);
            move || {
                assert!(matches!(processor.error.try_lock_result(), Ok(Some(_))));
                assert!(sink.flush().unwrap_err().to_string().contains("metric 42 overflowed u64"));
                calls.fetch_add(1, Ordering::Relaxed);
            }
        });
        tracing::subscriber::with_default(subscriber, || {
            processor.report(&Overflow::new(42_u32).into());
            processor.report(&Underflow::new(99_u32).into());
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
        let panic = catch_unwind(AssertUnwindSafe(|| {
            tracing::subscriber::with_default(crate::testing::OnEvent(|| panic!("injected subscriber panic")), || {
                processor.report(&Overflow::new(7_u32).into());
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
        let _panic = catch_unwind(AssertUnwindSafe(|| {
            let _guard = processor.error.lock();
            panic!("inject error-state poison");
        }));
        let calls = Arc::new(AtomicU64::new(0));
        let subscriber = crate::testing::OnEvent({
            let processor = Arc::clone(&processor);
            let calls = Arc::clone(&calls);
            move || {
                // A poison error owns the guard too; release it before reentering.
                drop(processor.error.try_lock_result().unwrap_err());
                assert!(processor.flush().unwrap_err().to_string().contains("state lock was poisoned"));
                calls.fetch_add(1, Ordering::Relaxed);
            }
        });
        tracing::subscriber::with_default(subscriber, || {
            processor.report(&Overflow::new(9_u32).into());
        });
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    struct MissingSlot;

    impl Event for MissingSlot {
        const DESCRIPTION: EventDescription = observed::__private::event_description(
            "missing.slot",
            Some(TypeId::of::<Self>()),
            None,
            None,
            false,
            &observed::__private::event_fields([observed::__private::field_descriptor(
                "value",
                None,
                Some(observed::__private::metric_field(
                    "value",
                    Some(observed::__private::metric_description(
                        "Missing",
                        InstrumentKind::Counter,
                        "",
                        "",
                        MetricValueKind::U64,
                    )),
                )),
            )]),
        );
        fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }
    }

    impl DynEvent for MissingSlot {
        fn name(&self) -> &'static str {
            "missing.slot"
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
        fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            ControlFlow::Continue(())
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
        processor.process(&EventView::new_synthetic(&MissingSlot, SystemTime::UNIX_EPOCH));
        let error = processor.flush().unwrap_err();
        let backend = std::error::Error::source(&error).unwrap().downcast_ref::<Error>().unwrap();
        let source = std::error::Error::source(backend).unwrap();
        assert!(source.is::<MetricReadError>());
        assert_eq!(
            ohno::ErrorExt::message(source.downcast_ref::<MetricReadError>().unwrap())
                .lines()
                .next()
                .unwrap(),
            "event `missing.slot` has invalid metric field 0: no measurement supplied"
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
    fn concurrent_typed_emission_publishes_exact_mock_reference_values_and_rejects_overflow() {
        let _native_tests = crate::native::TEST_NATIVE.lock();
        let processor = Arc::new(build_for_test(
            PerfCounterProcessor::builder()
                .category("Observed typed native test")
                .instance("worker-中文🦀")
                .bind_event::<RequestCompleted>()
                .bind_event::<HugeIncrement>(),
        ));
        let sink = Sink::new(
            "app",
            vec![Arc::<PerfCounterProcessor>::into_std_arc(Arc::clone(&processor))],
            SimpleClock::new_frozen(),
        );
        // Miri retains concurrent emission and publication with less repeated work.
        let iterations = if cfg!(miri) { 8_u64 } else { 1000 };
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let sink = &sink;
                scope.spawn(move || {
                    for _ in 0..iterations {
                        emit!(sink, RequestCompleted { bytes_sent: 37 });
                    }
                });
            }
            for _ in 0..20 {
                let snapshot = crate::native::test_snapshot(&processor.native, &processor.schema);
                assert!(snapshot.into_iter().all(|value| value <= 4 * iterations * 37));
            }
        });
        let expected = processor
            .schema
            .counters
            .iter()
            .map(|counter| match counter.name.as_str() {
                "Huge" => 0,
                "Requests" => 4 * iterations,
                _ => 4 * iterations * 37,
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
                "Requests" => 4 * iterations,
                _ => 4 * iterations * 37,
            })
            .collect::<Vec<_>>();
        assert_eq!(crate::native::test_snapshot(&processor.native, &processor.schema), expected);
        assert!(sink.flush().unwrap_err().to_string().contains("overflowed"));
        assert!(sink.flush().unwrap_err().to_string().contains("overflowed"));
    }
}
