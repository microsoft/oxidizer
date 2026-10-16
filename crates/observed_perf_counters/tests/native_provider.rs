// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The exact consumer test needs installer-provided schema; tests never install it.

#![cfg(all(windows, target_pointer_width = "64", not(miri)))]

use std::any::TypeId;
use std::iter::once;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::process::id;
use std::ptr::{copy_nonoverlapping, null, null_mut};
use std::sync::Mutex;
use std::thread;

use observed::metadata::{EventDescription, InstrumentKind, MetricValueKind};
use observed::processing::{EventProcessor, FieldVisitorFn};
use observed::{Event, Sink, emit, event};
use observed_perf_counters::PerfCounterProcessor;
use performables::arc::Arc;
use tick::SimpleClock;
use uuid::Uuid;
use windows_sys::Win32::Foundation::{ERROR_NOT_ENOUGH_MEMORY, ERROR_SUCCESS, GetLastError, HANDLE};
use windows_sys::Win32::System::Performance::{
    PERF_COUNTER_DATA, PERF_COUNTER_HEADER, PERF_COUNTER_IDENTIFIER, PERF_DATA_HEADER, PERF_INSTANCE_HEADER, PERF_MULTI_INSTANCES,
    PERF_MULTIPLE_INSTANCES, PERF_SINGLE_COUNTER, PerfAddCounters, PerfCloseQueryHandle, PerfCreateInstance, PerfOpenQueryHandle,
    PerfQueryCounterData, PerfStartProvider, PerfStopProvider,
};
use windows_sys::core::GUID;

testing_aids::init_tracing!();

static NATIVE_TESTS: Mutex<()> = Mutex::new(());

const CATEGORY: &str = "Observed Windows consumer test <&\"'> 中文🦀";
const COUNTER_SET_ID: u128 = 0x1b623456_789a_4bcd_8ef0_123456789abc;

fn manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("test_data")
        .join("ConsumerCounters.man")
}

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

// Owns a real Perflib consumer query; unlike the lifecycle mock, it needs installed definitions.
struct Query(HANDLE);

#[expect(clippy::unwrap_used, reason = "native test fixture requires complete returned native buffers")]
impl Query {
    fn open(counter_id: u32, instance: &str) -> Self {
        let mut handle = null_mut();
        // SAFETY: null denotes this machine; handle is a valid output parameter.
        let code = unsafe { PerfOpenQueryHandle(null(), &raw mut handle) };
        assert_eq!(code, ERROR_SUCCESS, "PerfOpenQueryHandle");
        let query = Self(handle);
        let name = instance.encode_utf16().chain(once(0)).collect::<Vec<_>>();
        let bytes = (size_of::<PERF_COUNTER_IDENTIFIER>() + name.len() * 2).next_multiple_of(8);
        let mut buffer = vec![0_u64; bytes / 8];
        let identifier = PERF_COUNTER_IDENTIFIER {
            CounterSetGuid: GUID::from_u128(COUNTER_SET_ID),
            Size: u32::try_from(bytes).unwrap(),
            CounterId: counter_id,
            InstanceId: id(),
            ..PERF_COUNTER_IDENTIFIER::default()
        };
        let base = buffer.as_mut_ptr();
        // SAFETY: buffer is aligned and large enough for the native identifier.
        unsafe { base.cast::<PERF_COUNTER_IDENTIFIER>().write(identifier) };
        // SAFETY: the aligned name destination follows the identifier and fits
        // in the allocated buffer, including its terminating UTF-16 zero.
        let name_address = unsafe { base.add(size_of::<PERF_COUNTER_IDENTIFIER>() / 8).cast::<u16>() };
        // SAFETY: these allocations do not overlap and the destination fits name.
        unsafe { copy_nonoverlapping(name.as_ptr(), name_address, name.len()) };
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
        let mut buffer = vec![0_u64; 512];
        let mut actual = 0;
        // SAFETY: buffer is initialized, eight-byte aligned writable storage;
        // its full byte length is passed and actual is a valid output parameter.
        let mut code = unsafe {
            PerfQueryCounterData(
                self.0,
                buffer.as_mut_ptr().cast(),
                u32::try_from(buffer.len() * 8).unwrap(),
                &raw mut actual,
            )
        };
        if code == ERROR_NOT_ENOUGH_MEMORY {
            buffer.resize(usize::try_from(actual).unwrap().div_ceil(8), 0);
            // SAFETY: the resized buffer is initialized and the new size fits.
            code = unsafe {
                PerfQueryCounterData(
                    self.0,
                    buffer.as_mut_ptr().cast(),
                    u32::try_from(buffer.len() * 8).unwrap(),
                    &raw mut actual,
                )
            };
        }
        assert_eq!(code, ERROR_SUCCESS, "PerfQueryCounterData");
        let buffer = buffer
            .into_iter()
            .flat_map(u64::to_le_bytes)
            .take(actual as usize)
            .collect::<Vec<_>>();
        assert_eq!(dword(&buffer, 4), 1);
        let counter_header = size_of::<PERF_DATA_HEADER>();
        assert_eq!(dword(&buffer, counter_header), ERROR_SUCCESS, "counter collection status");
        let kind = i32::try_from(dword(&buffer, counter_header + 4)).unwrap();
        let mut data = counter_header + size_of::<PERF_COUNTER_HEADER>();
        match kind {
            PERF_SINGLE_COUNTER => {}
            PERF_MULTIPLE_INSTANCES => {
                assert_eq!(dword(&buffer, data + 4), 1);
                data += size_of::<PERF_MULTI_INSTANCES>();
                assert_eq!(dword(&buffer, data + 4), id());
                let instance_size = usize::try_from(dword(&buffer, data)).unwrap();
                assert!(instance_size >= size_of::<PERF_INSTANCE_HEADER>());
                data += instance_size;
            }
            other => assert_eq!(other, PERF_SINGLE_COUNTER, "unexpected consumer block type"),
        }
        assert_eq!(dword(&buffer, data), 8, "u64 counter size");
        let value = data + size_of::<PERF_COUNTER_DATA>();
        u64::from_le_bytes(buffer[value..value + 8].try_into().unwrap())
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: this guard owns exactly one live query handle.
        let code = unsafe { PerfCloseQueryHandle(self.0) };
        assert_eq!(code, ERROR_SUCCESS, "PerfCloseQueryHandle");
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "native test fixture reads complete DWORD fields from a returned buffer"
)]
fn dword(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
// TODO: Run discovery and exact-read validation on a host with the supplied
// schema/resources installed; compiling the resource DLL alone does not cover it.
#[ignore = "Requires tests/test_data/ConsumerCounters.man and its resources installed; uninstalled discovery returns Windows 4200"]
fn consumer_reads_exact_integer_values_from_concurrent_emitters() {
    let _native_tests = NATIVE_TESTS.lock().unwrap();
    let category = CATEGORY;
    let instance = "worker-中文🦀";
    let builder = PerfCounterProcessor::builder()
        .manifest_file(manifest_path())
        .unwrap()
        .category(category)
        .instance(instance)
        .bind_event::<RequestCompleted>()
        .bind_event::<HugeIncrement>()
        .bind_event::<Disabled>()
        .bind_event::<ActiveRequests>()
        .bind_event::<QueueDepth>();
    let processor = Arc::new(builder.build().unwrap());
    // IDs are deliberately fixed by the externally authored fixture, not derived
    // from application metric metadata or from the implementation under test.
    let query = Query::open(7, instance);
    let requests = Query::open(41, instance);
    let huge = Query::open(31, instance);
    let disabled = Query::open(13, instance);
    let active = Query::open(2, instance);
    let depth = Query::open(53, instance);
    let sink = Sink::new(
        "app",
        vec![Arc::<PerfCounterProcessor>::into_std_arc(Arc::clone(&processor))],
        SimpleClock::new_frozen(),
    );
    assert_eq!(query.value(), 0);
    thread::scope(|scope| {
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
        .manifest_file(manifest_path())
        .unwrap()
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
        const DESCRIPTION: EventDescription = observed::__private::event_description(
            "malformed",
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
                        "Malformed",
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
    let _native_tests = NATIVE_TESTS.lock().unwrap();
    let processor = Arc::new(
        PerfCounterProcessor::builder()
            .manifest_file(manifest_path())
            .unwrap()
            .category(CATEGORY)
            .instance("worker")
            .bind_event::<Malformed>()
            .build()
            .unwrap(),
    );
    let sink = Sink::new(
        "app",
        vec![Arc::<PerfCounterProcessor>::into_std_arc(Arc::clone(&processor))],
        SimpleClock::new_frozen(),
    );
    emit!(sink, Malformed);
    assert!(
        processor
            .flush()
            .unwrap_err()
            .to_string()
            .contains("event `malformed` has invalid metric field 0: no measurement supplied")
    );
}

#[test]
fn native_provider_can_recreate_a_unicode_instance_after_shutdown() {
    let _native_tests = NATIVE_TESTS.lock().unwrap();
    let builder = || {
        PerfCounterProcessor::builder()
            .manifest_file(manifest_path())
            .unwrap()
            .category(CATEGORY)
            .instance("worker-中文🦀")
            .bind_event::<RequestCompleted>()
    };
    let processor = builder().build().unwrap();
    processor.flush().unwrap();
    drop(processor);
    builder().build().unwrap().flush().unwrap();
}

// Owns a real provider registration for native failure-path validation.
struct Registration(HANDLE);

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: this guard owns its live provider registration exclusively.
        assert_eq!(unsafe { PerfStopProvider(self.0) }, ERROR_SUCCESS);
    }
}

#[test]
fn an_unknown_counter_set_returns_a_native_creation_error() {
    let _native_tests = NATIVE_TESTS.lock().unwrap();
    let provider = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("observed unknown set {}", id()).as_bytes());
    let mut handle = null_mut();
    // SAFETY: the GUID and output handle are valid, and no callback is retained.
    let code = unsafe { PerfStartProvider(&GUID::from_u128(provider.as_u128()), None, &raw mut handle) };
    assert_eq!(code, ERROR_SUCCESS);
    let registration = Registration(handle);
    let name = [u16::from(b'x'), 0];
    // SAFETY: registration is live and the UTF-16 name is terminated. An
    // unregistered set is an ordinary native validation error.
    let instance = unsafe { PerfCreateInstance(registration.0, &GUID::from_u128(u128::MAX), name.as_ptr(), id()) };
    // SAFETY: read on this thread immediately after the failed native call.
    let code = unsafe { GetLastError() };
    assert!(instance.is_null());
    assert_ne!(code, ERROR_SUCCESS);
}
