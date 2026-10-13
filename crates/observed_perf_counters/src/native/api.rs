// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::c_void;
use std::ptr::NonNull;

use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::System::Performance::{
    PERF_COUNTERSET_INSTANCE, PerfCreateInstance, PerfDeleteInstance, PerfSetCounterRefValue, PerfSetCounterSetInfo, PerfStartProvider,
    PerfStopProvider,
};
use windows_sys::core::GUID;

use super::{control_callback, status};
use crate::Error;
use crate::error::ErrorKind;

// The provider owns the API implementation and its handle/instance lifetimes.
// Injection makes native failures reproducible without corrupting OS handles.
pub(super) trait Api: Send + Sync {
    fn start(&self, provider: &GUID) -> Result<HANDLE, Error>;
    unsafe fn set_info(&self, handle: HANDLE, template: &mut [u64], bytes: u32) -> u32;
    unsafe fn create(&self, handle: HANDLE, counter_set: &GUID, name: &[u16]) -> Result<NonNull<PERF_COUNTERSET_INSTANCE>, Error>;
    unsafe fn set_reference(&self, handle: HANDLE, instance: NonNull<PERF_COUNTERSET_INSTANCE>, id: u32, address: *mut c_void) -> u32;
    unsafe fn delete(&self, handle: HANDLE, instance: NonNull<PERF_COUNTERSET_INSTANCE>) -> u32;
    unsafe fn stop(&self, handle: HANDLE) -> u32;
}

pub(super) struct SystemApi;

impl Api for SystemApi {
    fn start(&self, provider: &GUID) -> Result<HANDLE, Error> {
        let mut handle = std::ptr::null_mut();
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
        let instance = unsafe { PerfCreateInstance(handle, counter_set, name.as_ptr(), std::process::id()) };
        NonNull::new(instance).ok_or_else(|| {
            // SAFETY: this immediately follows a failed PerfCreateInstance on
            // this thread, without another Windows call.
            let code = unsafe { GetLastError() };
            ErrorKind::Windows {
                operation: "PerfCreateInstance",
                code,
            }
            .into()
        })
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
