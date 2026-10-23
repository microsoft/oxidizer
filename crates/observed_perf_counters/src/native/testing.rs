// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::id;
use std::ptr::null_mut;
use std::sync::Barrier;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;

use ohno::ErrorExt as _;
use windows_sys::Win32::Foundation::{ERROR_BUSY, ERROR_INVALID_DATA};
use windows_sys::Win32::System::Performance::{PERF_COLLECT_END, PERF_COLLECT_START};

use super::*;

#[derive(Default)]
// Recorded lifecycle operations and an aligned instance buffer owned by the mock.
struct State {
    calls: Vec<&'static str>,
    ids: Vec<u32>,
    instance: Vec<u64>,
}

#[derive(Default)]
// Injected native lifecycle/reference model; never registers an operating-system provider.
struct MockApi {
    failure: Option<&'static str>,
    delete_code: u32,
    stop_code: u32,
    state: Mutex<State>,
}

impl MockApi {
    fn code(&self, operation: &'static str) -> u32 {
        self.state.lock().calls.push(operation);
        if self.failure == Some(operation) {
            ERROR_INVALID_DATA
        } else {
            ERROR_SUCCESS
        }
    }
}

// SAFETY: the mock owns fixed aligned storage through provider teardown. Calls
// serialize access through its mutex, returned resources are not thread-affine,
// native reference reads use full-width atomic loads, and it never accesses
// registered storage after successful stop.
unsafe impl Api for MockApi {
    fn start(&self, _: &GUID) -> Result<HANDLE, Error> {
        status("PerfStartProvider", self.code("start"))?;
        Ok(NonNull::<u8>::dangling().as_ptr().cast())
    }

    unsafe fn set_info(&self, _: HANDLE, template: &mut [u64], bytes: u32) -> u32 {
        assert_eq!(bytes as usize, size_of_val(template));
        // SAFETY: Provider supplied its initialized, aligned template.
        let header = unsafe { template.as_ptr().cast::<PERF_COUNTERSET_INFO>().read() };
        let mut state = self.state.lock();
        for index in 0..header.NumCounters as usize {
            let offset = (size_of::<PERF_COUNTERSET_INFO>() + index * size_of::<PERF_COUNTER_INFO>()) / 8;
            // SAFETY: each aligned, complete record follows the template header.
            let address = unsafe { template.as_ptr().add(offset) };
            // SAFETY: address points at the initialized native counter record.
            let record = unsafe { address.cast::<PERF_COUNTER_INFO>().read() };
            state.ids.push(record.CounterId);
        }
        drop(state);
        self.code("set_info")
    }

    unsafe fn create(&self, _: HANDLE, _: &GUID, name: &[u16]) -> Result<NonNull<PERF_COUNTERSET_INSTANCE>, Error> {
        assert_eq!(name.last(), Some(&0));
        status("PerfCreateInstance", self.code("create"))?;
        let mut state = self.state.lock();
        let bytes = size_of::<PERF_COUNTERSET_INSTANCE>() + state.ids.len() * 8;
        state.instance = vec![0_u64; bytes / 8];
        let instance = state.instance.as_mut_ptr().cast::<PERF_COUNTERSET_INSTANCE>();
        // SAFETY: allocation is aligned, large enough, and owned by this API
        // for the provider lifetime. All native header fields are initialized.
        unsafe {
            instance.write(PERF_COUNTERSET_INSTANCE {
                dwSize: u32::try_from(bytes).unwrap(),
                InstanceId: id(),
                ..PERF_COUNTERSET_INSTANCE::default()
            });
        }
        Ok(NonNull::new(instance).unwrap())
    }

    unsafe fn set_reference(&self, _: HANDLE, _: NonNull<PERF_COUNTERSET_INSTANCE>, id: u32, address: *mut c_void) -> u32 {
        assert_eq!(address.align_offset(align_of::<AtomicU64>()), 0);
        let mut state = self.state.lock();
        let index = state.ids.iter().position(|known| *known == id).unwrap();
        let offset = size_of::<PERF_COUNTERSET_INSTANCE>() + index * 8;
        // SAFETY: index is a registered slot within the aligned, fixed allocation.
        let slot = unsafe { state.instance.as_mut_ptr().add(offset / 8) };
        // SAFETY: this initialized slot holds a reference-sized value. Store the
        // pointer with provenance, not an integer representation.
        unsafe { slot.cast::<*mut c_void>().write(address) };
        drop(state);
        self.code("set_reference")
    }

    unsafe fn delete(&self, _: HANDLE, _: NonNull<PERF_COUNTERSET_INSTANCE>) -> u32 {
        self.state.lock().calls.push("delete");
        self.delete_code
    }

    unsafe fn stop(&self, _: HANDLE) -> u32 {
        self.state.lock().calls.push("stop");
        self.stop_code
    }
}

pub(crate) fn start_mock(schema: &CounterSchema, totals: Arc<[AtomicU64]>) -> Provider {
    Provider::start_with(schema, "worker", totals, Arc::new(MockApi::default()).coerce::<dyn Api>(|api| api)).unwrap()
}

fn schema() -> CounterSchema {
    crate::testing::counter_schema("Failure tests", &["Value"])
}

#[test]
fn startup_failures_release_registered_storage_and_started_resources() {
    let _serial = TEST_NATIVE.lock();
    let schema = schema();
    for (failure, expected) in [
        ("start", vec!["start"]),
        ("set_info", vec!["start", "set_info", "stop"]),
        ("create", vec!["start", "set_info", "create", "stop"]),
        (
            "set_reference",
            vec!["start", "set_info", "create", "set_reference", "delete", "stop"],
        ),
    ] {
        let api = Arc::new(MockApi {
            failure: Some(failure),
            ..MockApi::default()
        });
        let before = REGISTRY.lock().providers.len();
        let result = Provider::start_with(
            &schema,
            "worker",
            vec![AtomicU64::new(0)].into(),
            api.clone().coerce::<dyn Api>(|api| api),
        );
        let error = result.err().unwrap();
        assert_eq!(error.windows_code(), Some(ERROR_INVALID_DATA));
        assert_eq!(api.state.lock().calls, expected);
        assert_eq!(REGISTRY.lock().providers.len(), before);
    }
}

#[test]
fn teardown_removes_only_its_storage_and_retains_cells_when_stop_fails() {
    let _serial = TEST_NATIVE.lock();
    let schema = schema();
    let api = Arc::new(MockApi::default());
    let first = Provider::start_with(
        &schema,
        "one",
        vec![AtomicU64::new(0)].into(),
        api.clone().coerce::<dyn Api>(|api| api),
    )
    .unwrap();
    let retained = Provider::start_with(
        &schema,
        "two",
        vec![AtomicU64::new(0)].into(),
        Arc::new(MockApi {
            delete_code: ERROR_INVALID_DATA,
            stop_code: ERROR_BUSY,
            ..MockApi::default()
        })
        .coerce::<dyn Api>(|api| api),
    )
    .unwrap();
    let cells = retained.storage.clone();
    let before = REGISTRY.lock().providers.len();
    let success_logs = testing_aids::tracing_logs::Capture::new();
    tracing::subscriber::with_default(success_logs.subscriber(), || drop(first));
    assert!(!success_logs.output().contains("delete_failed"));
    assert_eq!(
        api.state.lock().calls,
        ["start", "set_info", "create", "set_reference", "delete", "stop"]
    );
    assert_eq!(REGISTRY.lock().providers.len(), before - 1);
    assert!(REGISTRY.lock().providers.iter().any(|storage| Arc::ptr_eq(storage, &cells)));
    let failure_logs = testing_aids::tracing_logs::Capture::new();
    tracing::subscriber::with_default(failure_logs.subscriber(), || drop(retained));
    failure_logs.assert_contains("observed_perf_counters.instance.delete_failed");
    failure_logs.assert_contains("observed_perf_counters.provider.stop_failed_storage_retained");
    assert!(REGISTRY.lock().providers.iter().any(|storage| Arc::ptr_eq(storage, &cells)));
    remove_storage(&cells);
}

#[test]
fn native_counter_limit_accepts_exactly_the_bound_and_rejects_one_more() {
    let _serial = TEST_NATIVE.lock();
    let schema = schema();
    let count = REGISTRY
        .lock()
        .providers
        .iter()
        .map(|provider| provider.totals.len())
        .sum::<usize>();
    let filler = Arc::new(CounterStorage {
        totals: (0..MAX_PUBLISHED_COUNTERS - count - 1).map(|_| AtomicU64::new(0)).collect(),
    });
    REGISTRY.lock().providers.push(filler.clone());
    let last = start_mock(&schema, vec![AtomicU64::new(0)].into());
    let error = Provider::start_with(
        &schema,
        "overflow",
        vec![AtomicU64::new(0)].into(),
        Arc::new(MockApi::default()).coerce::<dyn Api>(|api| api),
    )
    .err()
    .unwrap();
    let cause = error.find_source::<PublishedCapacity>().unwrap();
    assert_eq!((cause.count, cause.maximum), (MAX_PUBLISHED_COUNTERS + 1, MAX_PUBLISHED_COUNTERS));
    drop(last);
    remove_storage(&filler);
}

#[test]
fn callbacks_do_not_lock_storage_or_freeze_emission() {
    let _serial = TEST_NATIVE.lock();
    let schema = schema();
    let totals: Arc<[AtomicU64]> = vec![AtomicU64::new(0)].into();
    let provider = start_mock(&schema, Arc::clone(&totals));
    let guard = REGISTRY.lock();
    for (request, value) in [(PERF_COLLECT_END, 7), (PERF_COLLECT_START, 37), (PERF_COLLECT_START, 73), (999, 99)] {
        // SAFETY: the no-op callback never dereferences its buffer.
        assert_eq!(unsafe { control_callback(request, null_mut(), 0) }, ERROR_SUCCESS);
        totals[0].store(value, Ordering::Relaxed);
        assert_eq!(tests::registered_values(&provider, &schema), [value]);
    }
    drop(guard);
}

#[test]
fn collection_before_a_delayed_start_callback_reads_the_live_atomic_counter() {
    let _serial = TEST_NATIVE.lock();
    let schema = schema();
    let totals: Arc<[AtomicU64]> = vec![AtomicU64::new(0)].into();
    let provider = start_mock(&schema, Arc::clone(&totals));
    let entered = Barrier::new(2);
    let resume = Barrier::new(2);
    thread::scope(|scope| {
        let callback = scope.spawn(|| {
            entered.wait();
            resume.wait();
            // SAFETY: model a START callback descheduled before executing its
            // body. The buffer is unused, and no Windows provider is involved.
            unsafe { control_callback(PERF_COLLECT_START, null_mut(), 0) }
        });
        entered.wait();
        // Model the documented timeout without sleeping or racing a raw read.
        // The consumer proceeds before START and may finish before START returns.
        totals[0].store(u64::MAX, Ordering::Relaxed);
        assert_eq!(tests::registered_values(&provider, &schema), [u64::MAX]);
        // SAFETY: END has no required buffer in our no-op implementation.
        assert_eq!(unsafe { control_callback(PERF_COLLECT_END, null_mut(), 0) }, ERROR_SUCCESS);
        resume.wait();
        assert_eq!(callback.join().unwrap(), ERROR_SUCCESS);
    });
    assert_eq!(tests::registered_values(&provider, &schema), [u64::MAX]);
}

#[test]
fn concurrent_reference_reads_use_the_same_full_width_atomic_as_emission() {
    let _serial = TEST_NATIVE.lock();
    let schema = schema();
    let patterns = [0x5555_AAAA_5555_AAAA_u64, 0xAAAA_5555_AAAA_5555];
    let totals: Arc<[AtomicU64]> = vec![AtomicU64::new(patterns[0])].into();
    let provider = start_mock(&schema, Arc::clone(&totals));
    let iterations = if cfg!(miri) { 8 } else { 1000 };
    let start = Barrier::new(4);
    thread::scope(|scope| {
        scope.spawn(|| {
            start.wait();
            for index in 0..iterations {
                totals[0].store(patterns[index % patterns.len()], Ordering::Relaxed);
            }
        });
        for _ in 0..3 {
            let start = &start;
            let provider = &provider;
            let schema = &schema;
            scope.spawn(move || {
                start.wait();
                for _ in 0..iterations {
                    let reading = tests::registered_values(provider, schema);
                    assert!(patterns.contains(&reading[0]));
                }
            });
        }
    });
}

#[test]
fn teardown_can_remove_its_cells_from_a_poisoned_registry() {
    let storage = Arc::new(CounterStorage {
        totals: Vec::<AtomicU64>::new().into(),
    });
    let registry = Mutex::new(Registry {
        providers: vec![storage.clone()],
    });
    let _panic = catch_unwind(AssertUnwindSafe(|| {
        let _guard = registry.lock();
        panic!("inject registry poison");
    }));
    remove_storage_with(&registry, &storage);
    assert_eq!(registry.lock_result().err().unwrap().into_inner().providers.len(), 0);
}

#[test]
fn poisoned_registry_diagnostic_runs_after_unlocking_and_removing_cells() {
    let _serial = TEST_NATIVE.lock();
    let storage = Arc::new(CounterStorage {
        totals: Vec::<AtomicU64>::new().into(),
    });
    let registry = Arc::new(Mutex::new(Registry {
        providers: vec![Arc::clone(&storage)],
    }));
    let _panic = catch_unwind(AssertUnwindSafe(|| {
        let _guard = registry.lock();
        panic!("inject registry poison");
    }));
    let calls = Arc::new(AtomicU32::new(0));
    let subscriber = crate::testing::OnEvent({
        let registry = Arc::clone(&registry);
        let calls = Arc::clone(&calls);
        move || {
            match registry.try_lock_result() {
                Err(poisoned) => assert_eq!(poisoned.into_inner().providers.len(), 0),
                Ok(_) => panic!("diagnostic must run after releasing the poisoned registry"),
            }
            let provider = start_mock(&schema(), vec![AtomicU64::new(0)].into());
            drop(provider);
            calls.fetch_add(1, Ordering::Relaxed);
        }
    });
    tracing::subscriber::with_default(subscriber, || remove_storage_with(&registry, &storage));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn failed_instance_creation_preserves_the_status_captured_at_the_native_boundary() {
    let error = api::created_instance(null_mut(), ERROR_INVALID_DATA).unwrap_err();
    assert!(!error.is_invalid_configuration());
    assert_eq!(error.windows_code(), Some(ERROR_INVALID_DATA));
    let cause = error.find_source::<NativeFailure>().unwrap();
    assert_eq!(cause.operation, "PerfCreateInstance");
}
