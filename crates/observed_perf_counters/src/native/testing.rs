// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;

#[derive(Default)]
struct State {
    calls: Vec<&'static str>,
    ids: Vec<u32>,
    instance: Vec<u64>,
}

#[derive(Default)]
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

impl Api for MockApi {
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
                InstanceId: std::process::id(),
                ..PERF_COUNTERSET_INSTANCE::default()
            });
        }
        Ok(NonNull::new(instance).unwrap())
    }

    unsafe fn set_reference(&self, _: HANDLE, _: NonNull<PERF_COUNTERSET_INSTANCE>, id: u32, address: *mut c_void) -> u32 {
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
    CounterSchema::new(
        "Failure tests",
        "worker",
        &[crate::schema::CounterSpec {
            name: "Value".into(),
            description: String::new(),
            unit: String::new(),
            source_index: 0,
            kind: observed::metadata::InstrumentKind::Counter,
        }],
    )
    .unwrap()
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
        .map(|provider| provider.published.len())
        .sum::<usize>();
    let filler = Arc::new(CounterStorage {
        totals: Vec::<AtomicU64>::new().into(),
        published: (0..MAX_PUBLISHED_COUNTERS - count - 1).map(|_| UnsafeCell::new(0)).collect(),
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
    assert_eq!(
        error.to_string(),
        "invalid performance-counter configuration: too many native counters active in this executable image"
    );
    drop(last);
    remove_storage(&filler);
}

#[test]
fn callback_faults_are_sticky_and_prevent_further_snapshot_writes() {
    let registry = Mutex::new(Registry::default());
    let fault_state = AtomicU32::new(ERROR_SUCCESS);
    let readers = AtomicU32::new(0);
    assert_eq!(collect_with(&registry, &fault_state, &readers, 999), ERROR_SUCCESS);
    assert_eq!(
        collect_with(&registry, &fault_state, &readers, PERF_COLLECT_END),
        ERROR_INVALID_DATA
    );
    assert_eq!(
        collect_with(&registry, &fault_state, &readers, PERF_COLLECT_START),
        ERROR_INVALID_DATA
    );
    assert_eq!(fault(&fault_state, ERROR_INVALID_DATA), ERROR_INVALID_DATA);
    assert_eq!(fault_state.load(Ordering::Acquire), ERROR_INVALID_DATA);
    assert_eq!(readers.load(Ordering::Acquire), 0);

    let fault_state = AtomicU32::new(ERROR_SUCCESS);
    let readers = AtomicU32::new(SNAPSHOT_IN_PROGRESS - 1);
    assert_eq!(
        collect_with(&registry, &fault_state, &readers, PERF_COLLECT_START),
        ERROR_INVALID_DATA
    );
    assert_eq!(fault_state.load(Ordering::Acquire), ERROR_INVALID_DATA);
    assert_eq!(readers.load(Ordering::Acquire), SNAPSHOT_IN_PROGRESS - 1);
    let fault_state = AtomicU32::new(ERROR_BUSY);
    assert_eq!(snapshot(&registry, &fault_state), ERROR_BUSY);
}

#[test]
fn registry_contention_preserves_readers_and_the_next_collection_refreshes() {
    let storage = Arc::new(CounterStorage {
        totals: vec![AtomicU64::new(37)].into(),
        published: vec![UnsafeCell::new(0)].into(),
    });
    let registry = Mutex::new(Registry {
        providers: vec![Arc::clone(&storage)],
    });
    let fault_state = AtomicU32::new(ERROR_SUCCESS);
    let readers = AtomicU32::new(0);
    let guard = registry.lock();
    assert_eq!(collect_with(&registry, &fault_state, &readers, PERF_COLLECT_START), ERROR_SUCCESS);
    assert_eq!(readers.load(Ordering::Acquire), 1);
    drop(guard);
    assert_eq!(collect_with(&registry, &fault_state, &readers, PERF_COLLECT_START), ERROR_SUCCESS);
    assert_eq!(readers.load(Ordering::Acquire), 2);
    // SAFETY: both admitted collectors freeze the reference cells.
    assert_eq!(unsafe { storage.published[0].get().read() }, 0);
    let guard = registry.lock();
    assert_eq!(collect_with(&registry, &fault_state, &readers, PERF_COLLECT_END), ERROR_SUCCESS);
    assert_eq!(readers.load(Ordering::Acquire), 1);
    assert_eq!(collect_with(&registry, &fault_state, &readers, PERF_COLLECT_END), ERROR_SUCCESS);
    assert_eq!(readers.load(Ordering::Acquire), 0);
    drop(guard);
    assert_eq!(fault_state.load(Ordering::Acquire), ERROR_SUCCESS);
    assert_eq!(collect_with(&registry, &fault_state, &readers, PERF_COLLECT_START), ERROR_SUCCESS);
    // SAFETY: the admitted collector freezes the refreshed reference cells.
    assert_eq!(unsafe { storage.published[0].get().read() }, 37);
    assert_eq!(collect_with(&registry, &fault_state, &readers, PERF_COLLECT_END), ERROR_SUCCESS);
    assert_eq!(readers.load(Ordering::Acquire), 0);
}

#[test]
fn overlapping_snapshot_writer_returns_busy_without_poisoning_future_collections() {
    let registry = Mutex::new(Registry::default());
    let fault_state = AtomicU32::new(ERROR_SUCCESS);
    let readers = AtomicU32::new(SNAPSHOT_IN_PROGRESS);
    assert_eq!(collect_with(&registry, &fault_state, &readers, PERF_COLLECT_START), ERROR_BUSY);
    assert_eq!(fault_state.load(Ordering::Acquire), ERROR_SUCCESS);
    assert_eq!(readers.load(Ordering::Acquire), SNAPSHOT_IN_PROGRESS);
    // Finish the first writer's admission, then its collection.
    readers.store(1, Ordering::Release);
    assert_eq!(collect_with(&registry, &fault_state, &readers, PERF_COLLECT_END), ERROR_SUCCESS);
    assert_eq!(collect_with(&registry, &fault_state, &readers, PERF_COLLECT_START), ERROR_SUCCESS);
    assert_eq!(collect_with(&registry, &fault_state, &readers, PERF_COLLECT_END), ERROR_SUCCESS);
    assert_eq!(fault_state.load(Ordering::Acquire), ERROR_SUCCESS);
    assert_eq!(readers.load(Ordering::Acquire), 0);
}

#[test]
fn unmatched_collection_end_cannot_release_an_active_snapshot_writer() {
    let registry = Mutex::new(Registry::default());
    let fault_state = AtomicU32::new(ERROR_SUCCESS);
    let readers = AtomicU32::new(SNAPSHOT_IN_PROGRESS);
    assert_eq!(
        collect_with(&registry, &fault_state, &readers, PERF_COLLECT_END),
        ERROR_INVALID_DATA,
    );
    assert_eq!(readers.load(Ordering::Acquire), SNAPSHOT_IN_PROGRESS);
    assert_eq!(fault_state.load(Ordering::Acquire), ERROR_INVALID_DATA);
}

#[test]
fn concurrent_collectors_balance_admission_and_keep_reference_reads_race_free() {
    let storage = Arc::new(CounterStorage {
        totals: vec![AtomicU64::new(0)].into(),
        published: vec![UnsafeCell::new(0)].into(),
    });
    let registry = Mutex::new(Registry {
        providers: vec![Arc::clone(&storage)],
    });
    let fault_state = AtomicU32::new(ERROR_SUCCESS);
    let readers = AtomicU32::new(0);
    let iterations = if cfg!(miri) { 8 } else { 1000 };
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let registry = &registry;
            let fault_state = &fault_state;
            let readers = &readers;
            let storage = &storage;
            scope.spawn(move || {
                for _ in 0..iterations {
                    storage.totals[0].fetch_add(1, Ordering::Relaxed);
                    match collect_with(registry, fault_state, readers, PERF_COLLECT_START) {
                        ERROR_SUCCESS => {
                            // SAFETY: this successful START owns one reader
                            // until END; no snapshot writer can enter meanwhile.
                            let value = unsafe { storage.published[0].get().read() };
                            assert!(value <= 4 * iterations);
                            assert_eq!(collect_with(registry, fault_state, readers, PERF_COLLECT_END), ERROR_SUCCESS);
                        }
                        ERROR_BUSY => {}
                        other => panic!("collection failed with {other}"),
                    }
                }
            });
        }
    });
    assert_eq!(readers.load(Ordering::Acquire), 0);
    assert_eq!(fault_state.load(Ordering::Acquire), ERROR_SUCCESS);
    assert_eq!(collect_with(&registry, &fault_state, &readers, PERF_COLLECT_START), ERROR_SUCCESS);
    // SAFETY: the final collector freezes the refreshed reference value.
    assert_eq!(unsafe { storage.published[0].get().read() }, 4 * iterations);
    assert_eq!(collect_with(&registry, &fault_state, &readers, PERF_COLLECT_END), ERROR_SUCCESS);
}

#[test]
fn poisoned_registry_is_reported_without_entering_a_collection() {
    let registry = Mutex::new(Registry::default());
    let _panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = registry.lock();
        panic!("inject poison");
    }));
    let state = AtomicU32::new(ERROR_SUCCESS);
    assert_eq!(
        collect_with(&registry, &state, &AtomicU32::new(0), PERF_COLLECT_START),
        ERROR_INVALID_DATA
    );
    assert_eq!(state.load(Ordering::Acquire), ERROR_INVALID_DATA);
}

#[test]
fn callback_catches_panics_and_provider_flush_reports_the_collection_failure() {
    let _serial = TEST_NATIVE.lock();
    let provider = start_mock(&schema(), vec![AtomicU64::new(0)].into());
    // SAFETY: the callback ignores its buffer and this request has no payload.
    assert_eq!(unsafe { control_callback(999, std::ptr::null_mut(), 0) }, ERROR_SUCCESS);
    assert_eq!(callback_with(|| ERROR_SUCCESS), ERROR_SUCCESS);
    let old = COLLECTION_FAULT.load(Ordering::Acquire);
    let result = callback_with(|| panic!("injected callback failure"));
    let error = provider.check().unwrap_err();
    COLLECTION_FAULT.store(old, Ordering::Release);
    assert_eq!(result, ERROR_UNHANDLED_EXCEPTION);
    assert_eq!(error.windows_code(), Some(ERROR_UNHANDLED_EXCEPTION));
    provider.check().unwrap();
}

#[test]
fn teardown_can_remove_its_cells_from_a_poisoned_registry() {
    let storage = Arc::new(CounterStorage {
        totals: Vec::<AtomicU64>::new().into(),
        published: Vec::new().into(),
    });
    let registry = Mutex::new(Registry {
        providers: vec![storage.clone()],
    });
    let _panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
        published: Vec::new().into(),
    });
    let registry = Arc::new(Mutex::new(Registry {
        providers: vec![Arc::clone(&storage)],
    }));
    let _panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = registry.lock();
        panic!("inject registry poison");
    }));
    let captured_registry = Arc::clone(&registry);
    let calls = Arc::new(AtomicU32::new(0));
    let captured_calls = Arc::clone(&calls);
    let subscriber = crate::testing::OnEvent(move || {
        match captured_registry.try_lock_result() {
            Err(poisoned) => assert_eq!(poisoned.into_inner().providers.len(), 0),
            Ok(_) => panic!("diagnostic must run after releasing the poisoned registry"),
        }
        let provider = start_mock(&schema(), vec![AtomicU64::new(0)].into());
        drop(provider);
        captured_calls.fetch_add(1, Ordering::Relaxed);
    });
    tracing::subscriber::with_default(subscriber, || remove_storage_with(&registry, &storage));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "this test verifies actual OS error propagation; the injected lifecycle suite runs under Miri"
)]
fn native_instance_creation_preserves_the_os_error_for_an_unknown_counter_set() {
    let _serial = TEST_NATIVE.lock();
    let schema = schema();
    let provider = Provider::start(&schema, "worker", vec![AtomicU64::new(0)].into()).unwrap();
    // SAFETY: provider handle is live and name is terminated. An unknown GUID
    // is an ordinary Windows validation failure, not an invalid memory access.
    let result = unsafe { SystemApi.create(provider.handle, &GUID::from_u128(u128::MAX), &[u16::from(b'x'), 0]) };
    let error = result.unwrap_err();
    assert!(!error.is_invalid_configuration());
    assert!(error.windows_code().is_some_and(|code| code != ERROR_SUCCESS));
}
