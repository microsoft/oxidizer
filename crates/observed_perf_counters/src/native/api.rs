// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::c_void;
use std::process::id;
use std::ptr::{NonNull, null_mut};

use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::System::Performance::{
    PERF_COUNTERSET_INSTANCE, PerfCreateInstance, PerfDeleteInstance, PerfSetCounterRefValue, PerfSetCounterSetInfo, PerfStartProvider,
    PerfStopProvider,
};
use windows_sys::core::GUID;

use crate::Error;
use crate::error::NativeFailure;
use crate::native::{control_callback, status};

/// Native resource ownership, with injection for deterministic lifecycle failures.
///
/// # Safety
///
/// Successful startup and instance creation transfer live, exclusively owned
/// resources to the caller. Resources must support use and teardown from another
/// thread. Registered addresses may be accessed only while retained by the caller;
/// successful `stop` must end all access, including pending native reads/callbacks.
/// Implementations must not unwind across native callbacks. These obligations do
/// not establish synchronization between publication writes and consumer reads.
pub(super) unsafe trait Api: Send + Sync {
    fn start(&self, provider: &GUID) -> Result<HANDLE, Error>;
    unsafe fn set_info(&self, handle: HANDLE, template: &mut [u64], bytes: u32) -> u32;
    unsafe fn create(&self, handle: HANDLE, counter_set: &GUID, name: &[u16]) -> Result<NonNull<PERF_COUNTERSET_INSTANCE>, Error>;
    unsafe fn set_reference(&self, handle: HANDLE, instance: NonNull<PERF_COUNTERSET_INSTANCE>, id: u32, address: *mut c_void) -> u32;
    unsafe fn delete(&self, handle: HANDLE, instance: NonNull<PERF_COUNTERSET_INSTANCE>) -> u32;
    unsafe fn stop(&self, handle: HANDLE) -> u32;
}

/// Perflib entry points used by production provider ownership.
pub(super) struct SystemApi;

// SAFETY: successful Perflib creation returns caller-owned, non-thread-affine
// resources. Stop unregisters the provider; caller retains all reference storage
// until it succeeds. No borrowed input is retained except registered addresses.
unsafe impl Api for SystemApi {
    fn start(&self, provider: &GUID) -> Result<HANDLE, Error> {
        let mut handle = null_mut();
        // SAFETY: provider and output handle are valid. The static callback
        // outlives the provider and cannot unwind into Windows.
        let code = unsafe { PerfStartProvider(provider, Some(control_callback), &raw mut handle) };
        status("PerfStartProvider", code)?;
        Ok(handle)
    }

    unsafe fn set_info(&self, handle: HANDLE, template: &mut [u64], bytes: u32) -> u32 {
        // SAFETY: caller supplies a live provider and the aligned, initialized
        // template whose native byte length is bytes.
        unsafe { PerfSetCounterSetInfo(handle, template.as_mut_ptr().cast(), bytes) }
    }

    unsafe fn create(&self, handle: HANDLE, counter_set: &GUID, name: &[u16]) -> Result<NonNull<PERF_COUNTERSET_INSTANCE>, Error> {
        // SAFETY: caller supplies the live provider, counter-set GUID
        // and a terminated UTF-16 instance name.
        let instance = unsafe { PerfCreateInstance(handle, counter_set, name.as_ptr(), id()) };
        // SAFETY: read immediately on this thread, before any other Windows call.
        // The value is relevant only when instance is null.
        let code = unsafe { GetLastError() };
        created_instance(instance, code)
    }

    unsafe fn set_reference(&self, handle: HANDLE, instance: NonNull<PERF_COUNTERSET_INSTANCE>, id: u32, address: *mut c_void) -> u32 {
        // SAFETY: caller retains the instance and reference cell until stop
        // succeeds; id belongs to this provider's registered template.
        unsafe { PerfSetCounterRefValue(handle, instance.as_ptr(), id, address) }
    }

    unsafe fn delete(&self, handle: HANDLE, instance: NonNull<PERF_COUNTERSET_INSTANCE>) -> u32 {
        // SAFETY: caller exclusively owns the live instance and provider.
        unsafe { PerfDeleteInstance(handle, instance.as_ptr()) }
    }

    unsafe fn stop(&self, handle: HANDLE) -> u32 {
        // SAFETY: caller exclusively owns a live provider handle and retains
        // every registered reference cell until this call succeeds.
        unsafe { PerfStopProvider(handle) }
    }
}

// Preserve the status captured at the FFI boundary when instance creation failed.
pub(super) fn created_instance(instance: *mut PERF_COUNTERSET_INSTANCE, code: u32) -> Result<NonNull<PERF_COUNTERSET_INSTANCE>, Error> {
    NonNull::new(instance).ok_or_else(|| NativeFailure::new("PerfCreateInstance", code).into())
}
