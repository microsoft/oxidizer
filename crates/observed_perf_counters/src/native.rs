// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The only module that lends pointers to Perflib.
//!
//! Perflib dereferences reference-counter pointers; its documentation does not
//! establish Rust atomic-load semantics for those accesses.
//! Emission only modifies separate atomic totals. On `PERF_COLLECT_START` we
//! copy totals to native cells before collection and freeze those cells until
//! all matching `PERF_COLLECT_END` notifications. Overlapping collections share
//! the snapshot when callbacks complete on time.
//!
//! Publication safety is unresolved for a timed-out START: Windows may continue
//! collection before that callback returns. The internal writer marker cannot
//! exclude such native reads. A bounded work count is not a scheduling guarantee.
//! See `docs/design.md`, "Publication safety boundary", and the native contract:
//! <https://learn.microsoft.com/windows/win32/api/perflib/nc-perflib-perflibrequest>
//!
//! PERFLIBREQUEST has no user context or provider identity. Consequently one
//! image-local registry gates all providers using this callback. Callbacks do
//! not wait for locks, allocate snapshots, emit `observed` events, or call user code.
//! Registry contention keeps the previous snapshot; an overlapping snapshot writer
//! returns a retryable `ERROR_BUSY`. Invalid collection state permanently freezes
//! snapshots; flush reports it rather than risking a write during an active read.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::iter::once;
use std::panic::{UnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use performables::arc::Arc;
use performables::sync::mutex::Mutex;
use windows_sys::Win32::Foundation::{ERROR_BUSY, ERROR_INVALID_DATA, ERROR_SUCCESS, ERROR_UNHANDLED_EXCEPTION, HANDLE};
use windows_sys::Win32::System::Performance::{
    PERF_ATTRIB_BY_REFERENCE, PERF_COLLECT_END, PERF_COLLECT_START, PERF_COUNTER_INFO, PERF_COUNTERSET_INFO, PERF_COUNTERSET_INSTANCE,
    PERF_COUNTERSET_MULTI_INSTANCES, PERF_DETAIL_NOVICE,
};
use windows_sys::core::GUID;

use crate::Error;
use crate::error::configuration::{NativeCounterCount, NativeTemplateSize, PublishedCapacity};
use crate::error::{CollectionFailure, NativeFailure, Poisoned};
use crate::schema::CounterSchema;

mod api;
use api::{Api, SystemApi};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod testing;

// Conservative resource policy: at most sixteen full categories per image.
// This bounds copy work, not wall-clock time or time spent descheduled.
const MAX_PUBLISHED_COUNTERS: usize = 4096;
// The high bit excludes readers while the first collector refreshes the cells.
const SNAPSHOT_IN_PROGRESS: u32 = 1 << 31;
// winperf.h's PERF_COUNTER_LARGE_RAWCOUNT is PERF_SIZE_LARGE | PERF_TYPE_NUMBER |
// PERF_NUMBER_DECIMAL | PERF_DISPLAY_NO_SUFFIX; the last two omitted flags are 0.
const COUNTER_TYPE: u32 = 0x0001_0100;

// Atomic emission totals and stable reference cells retained through native shutdown.
struct CounterStorage {
    totals: Arc<[AtomicU64]>,
    published: Box<[UnsafeCell<u64>]>,
}

// SAFETY: the reader gate excludes cooperating callbacks and emission never
// touches published cells. REGISTRY retains their storage through shutdown.
// TODO: this does not justify native reads after a START timeout. The missing
// publication/access contract described above must be resolved before release.
unsafe impl Sync for CounterStorage {}

#[derive(Default)]
// Image-local strong ownership for every reference registered with the context-free callback.
struct Registry {
    providers: Vec<Arc<CounterStorage>>,
}

// These globals cannot be replaced with per-provider state: the Windows
// callback ABI does not carry a context. Nothing here is shared across DLLs.
static REGISTRY: Mutex<Registry> = Mutex::new(Registry { providers: Vec::new() });
static COLLECTION_FAULT: AtomicU32 = AtomicU32::new(ERROR_SUCCESS);
static COLLECTION_READERS: AtomicU32 = AtomicU32::new(0);

#[cfg(test)]
pub(crate) static TEST_NATIVE: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) fn test_snapshot(provider: &Provider, schema: &CounterSchema) -> Vec<u64> {
    let _collection = tests::Collection::begin();
    tests::registered_values(provider, schema)
}

fn fault(state: &AtomicU32, code: u32) -> u32 {
    // Retain the first failure without allocating or locking in an FFI callback.
    let _previous = state.compare_exchange(ERROR_SUCCESS, code, Ordering::AcqRel, Ordering::Acquire);
    code
}

fn collect(request: u32) -> u32 {
    collect_with(&REGISTRY, &COLLECTION_FAULT, &COLLECTION_READERS, request)
}

// Coordinate cooperating START/END callbacks; see the module's publication contract.
fn collect_with(registry: &Mutex<Registry>, state: &AtomicU32, readers: &AtomicU32, request: u32) -> u32 {
    if request != PERF_COLLECT_START && request != PERF_COLLECT_END {
        return ERROR_SUCCESS;
    }
    let existing = state.load(Ordering::Acquire);
    if existing != ERROR_SUCCESS {
        return existing;
    }
    if request == PERF_COLLECT_END {
        return match readers.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count > 0 && count < SNAPSHOT_IN_PROGRESS).then(|| count - 1)
        }) {
            Ok(_) => ERROR_SUCCESS,
            Err(_) => fault(state, ERROR_INVALID_DATA),
        };
    }
    let count = match readers.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
        if count == SNAPSHOT_IN_PROGRESS || count == SNAPSHOT_IN_PROGRESS - 1 {
            None
        } else {
            Some(if count == 0 { SNAPSHOT_IN_PROGRESS } else { count + 1 })
        }
    }) {
        Ok(count) => count,
        Err(SNAPSHOT_IN_PROGRESS) => {
            // Perflib reports a failed START to the consumer. No reader was
            // admitted, so this request must not decrement another one's END.
            return ERROR_BUSY;
        }
        Err(_) => return fault(state, ERROR_INVALID_DATA),
    };
    if count != 0 {
        return ERROR_SUCCESS;
    }
    // The writer bit prevents a new collector from reading partially copied
    // cells. END never needs the registry lock, including during teardown.
    let result = snapshot(registry, state);
    readers.store(1, Ordering::Release);
    result
}

// Refresh registered values without waiting for registry ownership.
fn snapshot(registry: &Mutex<Registry>, state: &AtomicU32) -> u32 {
    let registry = match registry.try_lock_result() {
        Ok(Some(registry)) => registry,
        // A previous snapshot remains safe to read. Still admit this reader
        // so an overlapping collector cannot refresh until its matching END.
        Ok(None) => return ERROR_SUCCESS,
        Err(_) => return fault(state, ERROR_INVALID_DATA),
    };
    let existing = state.load(Ordering::Acquire);
    if existing != ERROR_SUCCESS {
        return existing;
    }
    for provider in &registry.providers {
        for (total, published) in provider.totals.iter().zip(&provider.published) {
            let value = total.load(Ordering::Relaxed);
            // SAFETY: the writer bit excludes admitted readers and REGISTRY
            // retains each cell. This does not exclude a native reader that
            // proceeds after a callback timeout; see the publication blocker above.
            unsafe { published.get().write(value) };
        }
    }
    ERROR_SUCCESS
}

unsafe extern "system" fn control_callback(request: u32, _buffer: *mut c_void, _buffer_size: u32) -> u32 {
    // Never dereference the unneeded native buffer and never unwind into Windows.
    callback_with(|| collect(request))
}

fn callback_with(collect: impl FnOnce() -> u32 + UnwindSafe) -> u32 {
    catch_unwind(collect).unwrap_or_else(|_| fault(&COLLECTION_FAULT, ERROR_UNHANDLED_EXCEPTION))
}

// Owns provider registration, instance lifetime, and ordered cleanup.
pub(crate) struct Provider {
    handle: HANDLE,
    instance: Option<NonNull<PERF_COUNTERSET_INSTANCE>>,
    storage: Arc<CounterStorage>,
    api: Arc<dyn Api>,
}

// SAFETY: handles/pointers are owned, not thread-affine, and are only used by
// startup or exclusive Drop. Shared access only reads COLLECTION_FAULT. Storage
// synchronization and lifetime are established above independently of handles.
unsafe impl Send for Provider {}
// SAFETY: sharing Provider cannot access instance memory or mutate the handles.
// Counter emission updates separate AtomicU64 values, not native instance data.
unsafe impl Sync for Provider {}

impl Provider {
    pub(crate) fn start(schema: &CounterSchema, name: &str, totals: Arc<[AtomicU64]>) -> Result<Self, Error> {
        Self::start_with(schema, name, totals, Arc::new(SystemApi).coerce::<dyn Api>(|api| api))
    }

    fn start_with(schema: &CounterSchema, name: &str, totals: Arc<[AtomicU64]>, api: Arc<dyn Api>) -> Result<Self, Error> {
        let mut template = template(schema)?;
        let storage = Arc::new(CounterStorage {
            published: (0..totals.len()).map(|_| UnsafeCell::new(0)).collect(),
            totals,
        });
        {
            let mut registry = REGISTRY.lock_result().map_err(|_poisoned| Error::from(Poisoned::new()))?;
            check_collection()?;
            let count = registry.providers.iter().map(|provider| provider.published.len()).sum::<usize>();
            if count + storage.published.len() > MAX_PUBLISHED_COUNTERS {
                return Err(Error::configuration(PublishedCapacity::new(
                    count + storage.published.len(),
                    MAX_PUBLISHED_COUNTERS,
                )));
            }
            registry.providers.push(Arc::clone(&storage));
        }

        let provider_guid = guid(schema.provider_id);
        let handle = api.start(&provider_guid).inspect_err(|_| remove_storage(&storage))?;
        let mut provider = Self {
            handle,
            instance: None,
            storage,
            api,
        };
        let template_size =
            u32::try_from(template.len() * size_of::<u64>()).map_err(|_overflow| Error::configuration(NativeTemplateSize::new()))?;
        // SAFETY: template has the native header followed by NumCounters native
        // records, eight-byte alignment, and exactly template_size initialized bytes.
        let code = unsafe { provider.api.set_info(provider.handle, &mut template, template_size) };
        status("PerfSetCounterSetInfo", code)?;

        let counter_set_guid = guid(schema.counter_set_id);
        let name = name.encode_utf16().chain(once(0)).collect::<Vec<_>>();
        // SAFETY: the provider is live and name is validated, terminated UTF-16.
        // The counter-set GUID matches the registered template.
        let instance = unsafe { provider.api.create(provider.handle, &counter_set_guid, &name) }?;
        provider.instance = Some(instance);
        for counter in &schema.counters {
            let address = provider.storage.published[counter.source_index].get().cast::<c_void>();
            // SAFETY: instance belongs to this provider, counter.id is in its
            // template, and address is stable eight-byte u64 storage. Registration
            // occurs once; the registry retains storage through cleanup.
            let code = unsafe { provider.api.set_reference(provider.handle, instance, counter.id.get(), address) };
            status("PerfSetCounterRefValue", code)?;
        }
        Ok(provider)
    }

    #[expect(
        clippy::unused_self,
        reason = "collection status applies to every live provider sharing the callback ABI"
    )]
    pub(crate) fn check(&self) -> Result<(), Error> {
        check_collection()
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        if let Some(instance) = self.instance {
            // SAFETY: Drop exclusively owns this live provider and its instance.
            // Backing storage is retained in both self and REGISTRY.
            let code = unsafe { self.api.delete(self.handle, instance) };
            if code != ERROR_SUCCESS {
                tracing::error!(code, "observed_perf_counters.instance.delete_failed");
            }
        }
        // SAFETY: self exclusively owns the provider handle. PerfStopProvider
        // removes registration and frees associated native resources; storage
        // cannot be released before this call succeeds.
        let code = unsafe { self.api.stop(self.handle) };
        if code == ERROR_SUCCESS {
            remove_storage(&self.storage);
        } else {
            // REGISTRY retains the storage permanently: a cleanup error must
            // never turn a potentially registered native reference into a UAF.
            tracing::error!(code, "observed_perf_counters.provider.stop_failed_storage_retained");
        }
    }
}

pub(crate) fn check_collection() -> Result<(), Error> {
    let code = COLLECTION_FAULT.load(Ordering::Acquire);
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(CollectionFailure::new(code).into())
    }
}

fn remove_storage(storage: &Arc<CounterStorage>) {
    remove_storage_with(&REGISTRY, storage);
}

fn remove_storage_with(registry: &Mutex<Registry>, storage: &Arc<CounterStorage>) {
    let (mut registry, poisoned) = match registry.lock_result() {
        Ok(registry) => (registry, false),
        Err(poisoned) => (poisoned.into_inner(), true),
    };
    registry.providers.retain(|candidate| !Arc::ptr_eq(candidate, storage));
    drop(registry);
    if poisoned {
        tracing::error!("observed_perf_counters.registry.poisoned");
    }
}

fn status(operation: &'static str, code: u32) -> Result<(), Error> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(NativeFailure::new(operation, code).into())
    }
}

fn guid(value: uuid::Uuid) -> GUID {
    GUID::from_u128(value.as_u128())
}

// Encode aligned Perflib header/records from the deployment schema.
fn template(schema: &CounterSchema) -> Result<Vec<u64>, Error> {
    const _: () = assert!(size_of::<PERF_COUNTERSET_INSTANCE>() == 32);
    const _: () = assert!(size_of::<PERF_COUNTERSET_INFO>().is_multiple_of(8));
    const _: () = assert!(size_of::<PERF_COUNTER_INFO>().is_multiple_of(8));
    let size = size_of::<PERF_COUNTERSET_INFO>() + schema.counters.len() * size_of::<PERF_COUNTER_INFO>();
    let count = u32::try_from(schema.counters.len()).map_err(|_overflow| Error::configuration(NativeCounterCount::new()))?;
    let mut buffer = vec![0_u64; size / size_of::<u64>()];
    let header = PERF_COUNTERSET_INFO {
        CounterSetGuid: guid(schema.counter_set_id),
        ProviderGuid: guid(schema.provider_id),
        NumCounters: count,
        InstanceType: PERF_COUNTERSET_MULTI_INSTANCES,
    };
    let base = buffer.as_mut_ptr();
    // SAFETY: buffer is sized and aligned for the native header. All fields are
    // initialized, and the header is copied into otherwise zeroed storage.
    unsafe { base.cast::<PERF_COUNTERSET_INFO>().write(header) };
    for (index, counter) in schema.counters.iter().enumerate() {
        let record = PERF_COUNTER_INFO {
            CounterId: counter.id.get(),
            Type: COUNTER_TYPE,
            Attrib: PERF_ATTRIB_BY_REFERENCE,
            Size: 8,
            DetailLevel: PERF_DETAIL_NOVICE,
            Scale: 0,
            // CTRPP emits zero placeholders. Perflib assigns instance-relative
            // offsets from these ordered eight-byte slots during registration.
            Offset: 0,
        };
        let offset = size_of::<PERF_COUNTERSET_INFO>() + index * size_of::<PERF_COUNTER_INFO>();
        // SAFETY: offset selects one of count records within the allocation.
        // The header size and record stride preserve native eight-byte alignment.
        let destination = unsafe { base.add(offset / size_of::<u64>()) };
        // SAFETY: destination is aligned native record storage, valid for write.
        unsafe { destination.cast::<PERF_COUNTER_INFO>().write(record) };
    }
    Ok(buffer)
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use observed::metadata::InstrumentKind;

    use super::*;
    use crate::schema::CounterSpec;

    pub(super) struct Collection;

    impl Collection {
        pub(super) fn begin() -> Self {
            assert_eq!(collect(PERF_COLLECT_START), ERROR_SUCCESS);
            Self
        }
    }

    impl Drop for Collection {
        fn drop(&mut self) {
            assert_eq!(collect(PERF_COLLECT_END), ERROR_SUCCESS);
        }
    }

    pub(super) fn registered_values(provider: &Provider, schema: &CounterSchema) -> Vec<u64> {
        let instance = provider.instance.unwrap().as_ptr();
        // SAFETY: provider retains this live instance until Drop.
        let header = unsafe { instance.read() };
        schema
            .counters
            .iter()
            .map(|counter| {
                assert!(counter.offset + 8 <= header.dwSize);
                let base = instance.cast::<u8>();
                // SAFETY: instance allocation covers this aligned reference slot,
                // verified against dwSize; instance layout is immutable after setup.
                let slot = unsafe { base.add(counter.offset as usize) };
                // SAFETY: the slot contains the reference registered during startup.
                let pointer = unsafe { slot.cast::<*const u64>().read_unaligned() };
                assert_eq!(pointer, provider.storage.published[counter.source_index].get().cast_const());
                // SAFETY: the address was checked against our owned u64 cell, and
                // an active Collection keeps it frozen against snapshot writers.
                unsafe { pointer.read() }
            })
            .collect()
    }

    #[test]
    fn registered_native_references_are_frozen_through_overlapping_collections() {
        let _native_tests = TEST_NATIVE.lock();
        let schema = CounterSchema::new(
            "Native snapshots",
            "worker",
            &[
                CounterSpec {
                    name: "Bytes".into(),
                    unit: "By".into(),
                    description: String::new(),
                    source_index: 0,
                    kind: InstrumentKind::Counter,
                },
                CounterSpec {
                    name: "Requests".into(),
                    unit: String::new(),
                    description: String::new(),
                    source_index: 1,
                    kind: InstrumentKind::Counter,
                },
            ],
        )
        .unwrap();
        let totals: Arc<[AtomicU64]> = vec![AtomicU64::new(4096), AtomicU64::new(7)].into();
        let provider = testing::start_mock(&schema, Arc::clone(&totals));
        let first = Collection::begin();
        let expected = schema
            .counters
            .iter()
            .map(|counter| totals[counter.source_index].load(Ordering::Relaxed))
            .collect::<Vec<_>>();
        assert_eq!(registered_values(&provider, &schema), expected);
        totals[0].store(8192, Ordering::Relaxed);
        totals[1].store(9, Ordering::Relaxed);
        let second = Collection::begin();
        assert_eq!(registered_values(&provider, &schema), expected);
        drop(first);
        assert_eq!(registered_values(&provider, &schema), expected);
        drop(second);
        let next = Collection::begin();
        let updated = schema
            .counters
            .iter()
            .map(|counter| totals[counter.source_index].load(Ordering::Relaxed))
            .collect::<Vec<_>>();
        assert_eq!(registered_values(&provider, &schema), updated);
        drop(next);
        drop(provider);
        let restarted = testing::start_mock(&schema, totals);
        drop(restarted);
    }

    #[test]
    fn native_template_matches_ctrpp_u64_reference_records() {
        let schema = CounterSchema::new(
            "Layout test",
            "worker",
            &[
                CounterSpec {
                    name: "Bytes".into(),
                    unit: "By".into(),
                    description: String::new(),
                    source_index: 0,
                    kind: InstrumentKind::Counter,
                },
                CounterSpec {
                    name: "Requests".into(),
                    unit: String::new(),
                    description: String::new(),
                    source_index: 1,
                    kind: InstrumentKind::Counter,
                },
            ],
        )
        .unwrap();
        let buffer = template(&schema).unwrap();
        assert_eq!(
            buffer.len() * 8,
            size_of::<PERF_COUNTERSET_INFO>() + 2 * size_of::<PERF_COUNTER_INFO>()
        );
        let base = buffer.as_ptr();
        // SAFETY: template produced an aligned, initialized native header.
        let header = unsafe { base.cast::<PERF_COUNTERSET_INFO>().read() };
        assert_eq!(
            (
                (
                    header.CounterSetGuid.data1,
                    header.CounterSetGuid.data2,
                    header.CounterSetGuid.data3,
                    header.CounterSetGuid.data4
                ),
                (
                    header.ProviderGuid.data1,
                    header.ProviderGuid.data2,
                    header.ProviderGuid.data3,
                    header.ProviderGuid.data4
                ),
                header.NumCounters,
                header.InstanceType,
            ),
            (
                (
                    GUID::from_u128(schema.counter_set_id.as_u128()).data1,
                    GUID::from_u128(schema.counter_set_id.as_u128()).data2,
                    GUID::from_u128(schema.counter_set_id.as_u128()).data3,
                    GUID::from_u128(schema.counter_set_id.as_u128()).data4
                ),
                (
                    GUID::from_u128(schema.provider_id.as_u128()).data1,
                    GUID::from_u128(schema.provider_id.as_u128()).data2,
                    GUID::from_u128(schema.provider_id.as_u128()).data3,
                    GUID::from_u128(schema.provider_id.as_u128()).data4
                ),
                2,
                PERF_COUNTERSET_MULTI_INSTANCES,
            ),
        );
        for (index, counter) in schema.counters.iter().enumerate() {
            let offset = size_of::<PERF_COUNTERSET_INFO>() + index * size_of::<PERF_COUNTER_INFO>();
            // SAFETY: offset identifies an initialized record in this buffer.
            let address = unsafe { base.add(offset / size_of::<u64>()) };
            // SAFETY: address is aligned and points at one complete native record.
            let record = unsafe { address.cast::<PERF_COUNTER_INFO>().read() };
            assert_eq!(
                (
                    record.CounterId,
                    record.Type,
                    record.Attrib,
                    record.Size,
                    record.Offset,
                    record.DetailLevel,
                    record.Scale
                ),
                (counter.id.get(), 0x0001_0100, PERF_ATTRIB_BY_REFERENCE, 8, 0, PERF_DETAIL_NOVICE, 0)
            );
        }
    }
}
