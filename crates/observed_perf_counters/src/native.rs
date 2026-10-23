// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stable atomic counter storage registered with Perflib.
//!
//! `PerfSetCounterRefValue` specifies a pointer to a four- or eight-byte integer
//! which Perflib dereferences when collecting. This backend registers only
//! eight-byte counters on 64-bit Windows. Windows guarantees that aligned
//! eight-byte variable accesses are atomic on those targets:
//! <https://learn.microsoft.com/windows/win32/sync/interlocked-variable-access>.
//!
//! Each reference points directly at an aligned `AtomicU64`. Rust updates use
//! only full-width atomic operations; native reads follow the Windows scalar
//! access contract, not ordinary non-atomic Rust pointer reads. No byte-sized
//! accesses, shared `u64` references, or payload publication use this storage.
//! This is the reference-counter pattern also used by .NET `CounterData` with
//! Interlocked operations. It is not a portable mixed-access assumption.
//!
//! A no-op callback satisfies manifests declaring callback="custom". It does
//! not copy or freeze values, so collection can proceed before, during, or
//! after a delayed callback. Individual readings are atomic, not transactional
//! across counters. The registry retains addresses until native shutdown.

use std::ffi::c_void;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::iter::once;
use std::ptr::NonNull;
use std::sync::atomic::AtomicU64;

use performables::arc::Arc;
use performables::sync::mutex::Mutex;
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::System::Performance::{
    PERF_ATTRIB_BY_REFERENCE, PERF_COUNTER_INFO, PERF_COUNTERSET_INFO, PERF_COUNTERSET_INSTANCE, PERF_COUNTERSET_MULTI_INSTANCES,
    PERF_DETAIL_NOVICE,
};
use windows_sys::core::GUID;

use crate::Error;
use crate::error::configuration::{NativeCounterCount, NativeTemplateSize, PublishedCapacity};
use crate::error::{NativeFailure, Poisoned};
use crate::schema::CounterSchema;

mod api;
use api::{Api, SystemApi};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod testing;

// Conservative resource policy: at most sixteen full categories per image.
const MAX_PUBLISHED_COUNTERS: usize = 4096;
// winperf.h's PERF_COUNTER_LARGE_RAWCOUNT is PERF_SIZE_LARGE | PERF_TYPE_NUMBER |
// PERF_NUMBER_DECIMAL | PERF_DISPLAY_NO_SUFFIX; the last two omitted flags are 0.
const COUNTER_TYPE: u32 = 0x0001_0100;

const _: () = assert!(size_of::<AtomicU64>() == 8 && align_of::<AtomicU64>() == 8);

// Each registration has its own identity even when it shares emission totals.
struct CounterStorage {
    totals: Arc<[AtomicU64]>,
}

#[derive(Default)]
// Retain every registered address until native shutdown succeeds.
struct Registry {
    providers: Vec<Arc<CounterStorage>>,
}

// Retain storage after failed native shutdown without a process-wide Rust ABI.
static REGISTRY: Mutex<Registry> = Mutex::new(Registry { providers: Vec::new() });

#[cfg(test)]
pub(crate) static TEST_NATIVE: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) fn test_readings(provider: &Provider, schema: &CounterSchema) -> Vec<u64> {
    tests::registered_values(provider, schema)
}

unsafe extern "system" fn control_callback(_request: u32, _buffer: *mut c_void, _buffer_size: u32) -> u32 {
    // Publication is continuous. Never dereference buffers or run fallible code.
    ERROR_SUCCESS
}

// Owns provider registration, instance lifetime, and ordered cleanup.
pub(crate) struct Provider {
    handle: HANDLE,
    instance: Option<NonNull<PERF_COUNTERSET_INSTANCE>>,
    storage: Arc<CounterStorage>,
    api: Arc<dyn Api>,
}

impl Debug for Provider {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("Provider")
            .field("registered_counters", &self.storage.totals.len())
            .finish_non_exhaustive()
    }
}

// SAFETY: handles/pointers are owned, not thread-affine, and are only used by
// startup or exclusive Drop. Registered data uses only full-width atomic
// accesses under the Windows contract above and is retained through shutdown.
unsafe impl Send for Provider {}
// SAFETY: sharing Provider cannot access instance memory or mutate the handles.
// Counter emission updates AtomicU64 values, not native instance data.
unsafe impl Sync for Provider {}

impl Provider {
    pub(crate) fn start(schema: &CounterSchema, name: &str, totals: Arc<[AtomicU64]>) -> Result<Self, Error> {
        Self::start_with(schema, name, totals, Arc::new(SystemApi).coerce::<dyn Api>(|api| api))
    }

    fn start_with(schema: &CounterSchema, name: &str, totals: Arc<[AtomicU64]>, api: Arc<dyn Api>) -> Result<Self, Error> {
        let mut template = template(schema)?;
        let storage = Arc::new(CounterStorage { totals });
        {
            let mut registry = REGISTRY.lock_result().map_err(|_poisoned| Error::from(Poisoned::new()))?;
            let count = registry.providers.iter().map(|provider| provider.totals.len()).sum::<usize>();
            if count + storage.totals.len() > MAX_PUBLISHED_COUNTERS {
                return Err(Error::configuration(PublishedCapacity::new(
                    count + storage.totals.len(),
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
            let address = provider.storage.totals[counter.source_index].as_ptr().cast::<c_void>();
            // SAFETY: instance belongs to this provider, counter.id is in its
            // eight-byte template, and address is a stable, aligned AtomicU64.
            // The native API's full-width atomic-read contract is documented
            // in api.rs; the registry retains storage through native cleanup.
            let code = unsafe { provider.api.set_reference(provider.handle, instance, counter.id.get(), address) };
            status("PerfSetCounterRefValue", code)?;
        }
        Ok(provider)
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
    use std::sync::atomic::Ordering;

    use super::*;

    static_assertions::assert_impl_all!(CounterStorage: Send, Sync);

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
                assert_eq!(pointer, provider.storage.totals[counter.source_index].as_ptr().cast_const());
                assert_eq!(pointer.align_offset(align_of::<AtomicU64>()), 0);
                // SAFETY: the pointer identifies our retained, aligned atomic.
                // Model the full-width native atomic read with a Rust atomic
                // load, not a conflicting non-atomic raw-pointer read.
                unsafe { AtomicU64::from_ptr(pointer.cast_mut()) }.load(Ordering::Relaxed)
            })
            .collect()
    }

    #[test]
    fn registered_native_references_point_at_live_atomic_totals() {
        let _native_tests = TEST_NATIVE.lock();
        let schema = crate::testing::counter_schema("Native snapshots", &["Bytes", "Requests"]);
        let totals: Arc<[AtomicU64]> = vec![AtomicU64::new(4096), AtomicU64::new(7)].into();
        let provider = testing::start_mock(&schema, Arc::clone(&totals));
        let expected = schema
            .counters
            .iter()
            .map(|counter| totals[counter.source_index].load(Ordering::Relaxed))
            .collect::<Vec<_>>();
        assert_eq!(registered_values(&provider, &schema), expected);
        totals[0].store(8192, Ordering::Relaxed);
        totals[1].store(9, Ordering::Relaxed);
        let updated = schema
            .counters
            .iter()
            .map(|counter| totals[counter.source_index].load(Ordering::Relaxed))
            .collect::<Vec<_>>();
        assert_eq!(registered_values(&provider, &schema), updated);
        drop(provider);
        let restarted = testing::start_mock(&schema, totals);
        drop(restarted);
    }

    #[test]
    fn native_template_matches_ctrpp_u64_reference_records() {
        let schema = crate::testing::counter_schema("Layout test", &["Bytes", "Requests"]);
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
