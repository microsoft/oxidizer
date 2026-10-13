// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The exact consumer test needs installer-provided schema; tests never install it.

use observed::processing::EventProcessor;
use observed::{Event, Sink, emit, event};
use performables::arc::Arc;
use windows_sys::Win32::Foundation::{ERROR_NOT_ENOUGH_MEMORY, ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::System::Performance::{
    PERF_COUNTER_DATA, PERF_COUNTER_HEADER, PERF_COUNTER_IDENTIFIER, PERF_DATA_HEADER, PERF_INSTANCE_HEADER, PERF_MULTI_INSTANCES,
    PERF_MULTIPLE_INSTANCES, PERF_SINGLE_COUNTER, PerfAddCounters, PerfCloseQueryHandle, PerfOpenQueryHandle, PerfQueryCounterData,
};
use windows_sys::core::GUID;

use crate::PerfCounterProcessor;
use crate::schema::CounterSchema;

#[event("request.completed")]
#[counter(name = "Requests")]
#[counter(bytes_sent, name = "Bytes Sent", unit = "By")]
struct RequestCompleted {
    #[unredacted]
    bytes_sent: u32,
}

#[event("huge.increment")]
#[counter(value, name = "Huge")]
struct HugeIncrement {
    #[unredacted]
    value: u64,
}

#[event("disabled", disabled)]
#[counter(bytes_sent, name = "Disabled", unit = "By")]
struct Disabled {
    #[unredacted]
    bytes_sent: u32,
}

#[event("requests.active")]
#[updown_counter(delta, name = "Active Requests")]
struct ActiveRequests {
    #[unredacted]
    delta: i32,
}

#[event("queue.depth")]
#[gauge(depth, name = "Queue Depth")]
struct QueueDepth {
    #[unredacted]
    depth: u32,
}

struct Query(HANDLE);

impl Query {
    fn open(schema: &CounterSchema, counter_name: &str, instance: &str) -> Self {
        let mut handle = std::ptr::null_mut();
        // SAFETY: null denotes this machine; handle is a valid output parameter.
        let code = unsafe { PerfOpenQueryHandle(std::ptr::null(), &raw mut handle) };
        assert_eq!(code, ERROR_SUCCESS, "PerfOpenQueryHandle");
        let query = Self(handle);
        let counter = schema.counters.iter().find(|counter| counter.name == counter_name).unwrap();
        let name = instance.encode_utf16().chain(std::iter::once(0)).collect::<Vec<_>>();
        let bytes = (size_of::<PERF_COUNTER_IDENTIFIER>() + name.len() * 2).next_multiple_of(8);
        let mut buffer = vec![0_u64; bytes / 8];
        let identifier = PERF_COUNTER_IDENTIFIER {
            CounterSetGuid: GUID::from_u128(schema.counter_set_id.as_u128()),
            Size: u32::try_from(bytes).unwrap(),
            CounterId: counter.id,
            InstanceId: std::process::id(),
            ..PERF_COUNTER_IDENTIFIER::default()
        };
        let base = buffer.as_mut_ptr();
        // SAFETY: buffer is aligned and large enough for the native identifier.
        unsafe { base.cast::<PERF_COUNTER_IDENTIFIER>().write(identifier) };
        // SAFETY: the aligned name destination follows the identifier and fits
        // in the allocated buffer, including its terminating UTF-16 zero.
        let name_address = unsafe { base.add(size_of::<PERF_COUNTER_IDENTIFIER>() / 8).cast::<u16>() };
        // SAFETY: these allocations do not overlap and the destination fits name.
        unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), name_address, name.len()) };
        // SAFETY: identifier block and terminated name are initialized and valid;
        // query owns a live native query handle.
        let code = unsafe { PerfAddCounters(query.0, base.cast(), u32::try_from(bytes).unwrap()) };
        assert_eq!(code, ERROR_SUCCESS, "PerfAddCounters");
        // SAFETY: Windows updated the still-live native identifier in this buffer.
        let added = unsafe { base.cast::<PERF_COUNTER_IDENTIFIER>().read() };
        assert_eq!(added.Status, ERROR_SUCCESS, "GUID-based discovery requires installed metadata");
        query
    }

    fn value(&self) -> u64 {
        let mut words = vec![0_u64; 512];
        let mut actual = 0;
        // SAFETY: words is initialized, eight-byte aligned writable storage;
        // its full byte length is passed and actual is a valid output parameter.
        let mut code = unsafe {
            PerfQueryCounterData(
                self.0,
                words.as_mut_ptr().cast(),
                u32::try_from(words.len() * 8).unwrap(),
                &raw mut actual,
            )
        };
        if code == ERROR_NOT_ENOUGH_MEMORY {
            words.resize(usize::try_from(actual).unwrap().div_ceil(8), 0);
            // SAFETY: the resized buffer is initialized and the new size fits.
            code = unsafe {
                PerfQueryCounterData(
                    self.0,
                    words.as_mut_ptr().cast(),
                    u32::try_from(words.len() * 8).unwrap(),
                    &raw mut actual,
                )
            };
        }
        assert_eq!(code, ERROR_SUCCESS, "PerfQueryCounterData");
        let bytes = words
            .into_iter()
            .flat_map(u64::to_le_bytes)
            .take(actual as usize)
            .collect::<Vec<_>>();
        assert_eq!(dword(&bytes, 4), 1);
        let counter_header = size_of::<PERF_DATA_HEADER>();
        assert_eq!(dword(&bytes, counter_header), ERROR_SUCCESS, "counter collection status");
        let kind = i32::try_from(dword(&bytes, counter_header + 4)).unwrap();
        let mut data = counter_header + size_of::<PERF_COUNTER_HEADER>();
        match kind {
            PERF_SINGLE_COUNTER => {}
            PERF_MULTIPLE_INSTANCES => {
                assert_eq!(dword(&bytes, data + 4), 1);
                data += size_of::<PERF_MULTI_INSTANCES>();
                assert_eq!(dword(&bytes, data + 4), std::process::id());
                let instance_size = usize::try_from(dword(&bytes, data)).unwrap();
                assert!(instance_size >= size_of::<PERF_INSTANCE_HEADER>());
                data += instance_size;
            }
            other => assert_eq!(other, PERF_SINGLE_COUNTER, "unexpected consumer block type"),
        }
        assert_eq!(dword(&bytes, data), 8, "u64 counter size");
        let value = data + size_of::<PERF_COUNTER_DATA>();
        u64::from_le_bytes(bytes[value..value + 8].try_into().unwrap())
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: this guard owns exactly one live query handle.
        let code = unsafe { PerfCloseQueryHandle(self.0) };
        assert_eq!(code, ERROR_SUCCESS, "PerfCloseQueryHandle");
    }
}

fn dword(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
#[ignore = "Requires installed schema/resources: examples/consumer_manifest.rs; uninstalled GUID discovery returns Windows 4200"]
fn consumer_reads_exact_integer_values_from_concurrent_emitters() {
    let _native_tests = super::TEST_NATIVE.lock();
    let category = "Observed Windows consumer test <&\"'> 中文🦀";
    let instance = "worker-中文🦀";
    let processor = Arc::new(
        PerfCounterProcessor::builder()
            .category(category)
            .instance(instance)
            .bind_event::<RequestCompleted>()
            .bind_event::<HugeIncrement>()
            .bind_event::<Disabled>()
            .bind_event::<ActiveRequests>()
            .bind_event::<QueueDepth>()
            .build()
            .unwrap(),
    );
    let query = Query::open(processor.schema(), "Bytes Sent", instance);
    let requests = Query::open(processor.schema(), "Requests", instance);
    let huge = Query::open(processor.schema(), "Huge", instance);
    let disabled = Query::open(processor.schema(), "Disabled", instance);
    let active = Query::open(processor.schema(), "Active Requests", instance);
    let depth = Query::open(processor.schema(), "Queue Depth", instance);
    let sink = Sink::new(
        "app",
        vec![Arc::<PerfCounterProcessor>::into_std_arc(Arc::clone(&processor))],
        tick::SimpleClock::new_frozen(),
    );
    assert_eq!(query.value(), 0);
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
            let value = query.value();
            assert!(value <= 148_000);
        }
    });
    assert_eq!(query.value(), 148_000);
    assert_eq!(requests.value(), 4000);
    emit!(sink, Disabled { bytes_sent: 999 });
    assert_eq!(disabled.value(), 0);
    emit!(sink, ActiveRequests { delta: 3 });
    assert_eq!(active.value(), 3);
    emit!(sink, ActiveRequests { delta: -2 });
    assert_eq!(active.value(), 1);
    emit!(sink, QueueDepth { depth: 10 });
    assert_eq!(depth.value(), 10);
    emit!(sink, QueueDepth { depth: 7 });
    assert_eq!(depth.value(), 7);
    sink.flush().unwrap();

    emit!(sink, HugeIncrement { value: u64::MAX });
    emit!(sink, HugeIncrement { value: 1 });
    assert_eq!(huge.value(), u64::MAX);
    assert!(sink.flush().unwrap_err().to_string().contains("overflowed"));
    assert!(sink.flush().unwrap_err().to_string().contains("overflowed"));
    drop((query, requests, huge, disabled, active, depth, sink, processor));

    // Successful teardown allows the same native category to be registered again.
    let recreated = PerfCounterProcessor::builder()
        .category(category)
        .instance(instance)
        .bind_event::<RequestCompleted>()
        .build()
        .unwrap();
    recreated.flush().unwrap();
}

#[test]
fn malformed_slot_surfaces_through_flush_instead_of_being_swallowed() {
    struct Malformed;
    impl Event for Malformed {
        const DESCRIPTION: observed::metadata::EventDescription =
            observed::metadata::EventDescription::new("malformed", Some(std::any::TypeId::of::<Self>()), None, None, true, false);
        const METRICS: &'static [observed::metadata::MetricDescription] =
            &[
                observed::metadata::MetricDescription::new("Malformed", observed::metadata::InstrumentKind::Counter, "", "")
                    .with_value_kind(observed::metadata::MetricValueKind::U64),
            ];
        fn visit_fields(&self, _: &mut observed::processing::FieldVisitorFn<'_>) -> std::ops::ControlFlow<()> {
            std::ops::ControlFlow::Continue(())
        }
    }
    let _native_tests = super::TEST_NATIVE.lock();
    let processor = Arc::new(
        PerfCounterProcessor::builder()
            .category(format!("Observed malformed {}", std::process::id()))
            .instance("worker")
            .bind_event::<Malformed>()
            .build()
            .unwrap(),
    );
    let sink = Sink::new(
        "app",
        vec![Arc::<PerfCounterProcessor>::into_std_arc(Arc::clone(&processor))],
        tick::SimpleClock::new_frozen(),
    );
    emit!(sink, Malformed);
    assert!(
        processor
            .flush()
            .unwrap_err()
            .to_string()
            .contains("cannot read a bound observed counter")
    );
}
