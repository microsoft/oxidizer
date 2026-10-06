// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

pub(crate) const PAGE: usize = 4096;
pub(crate) const RESERVE_MIN: usize = 65536;

#[repr(C)]
struct AddressRequirements {
    lowest: *mut c_void,
    highest: *mut c_void,
    alignment: usize,
}

#[repr(C)]
struct ExtendedParameter {
    kind: u64,
    value: *mut c_void,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> i32;
}

#[link(name = "mincore")]
unsafe extern "system" {
    fn VirtualAlloc2(
        process: *mut c_void,
        address: *mut c_void,
        size: usize,
        kind: u32,
        protect: u32,
        parameters: *mut ExtendedParameter,
        count: u32,
    ) -> *mut c_void;
}

#[link(name = "synchronization")]
unsafe extern "system" {
    fn WaitOnAddress(address: *const c_void, compare: *const c_void, size: usize, milliseconds: u32) -> i32;
    fn WakeByAddressSingle(address: *const c_void);
}

pub(crate) fn wait(word: &AtomicU32, expected: u32) {
    while word.load(Ordering::Acquire) == expected {
        // SAFETY: Live atomic word and comparison word have identical size.
        // WaitOnAddress only reads the atomic's representation atomically.
        unsafe {
            WaitOnAddress(
                std::ptr::from_ref::<AtomicU32>(word).cast(),
                (&raw const expected).cast(),
                std::mem::size_of::<u32>(),
                u32::MAX,
            )
        };
    }
}

/// Wakes a waiter by numeric address; the OS does not access target memory.
pub(crate) fn wake_one(address: *const u32) {
    // SAFETY: WakeByAddressSingle uses the address as a wait-table key, and does
    // not dereference it (the waiter may already have returned).
    unsafe { WakeByAddressSingle(address.cast()) };
}

/// Reserves inaccessible address space; no Rust allocation is performed.
pub(crate) fn reserve(size: usize, alignment: usize) -> *mut u8 {
    if size == 0 || !size.is_multiple_of(RESERVE_MIN) || !alignment.is_power_of_two() {
        return std::ptr::null_mut();
    }
    let mut requirements = AddressRequirements {
        lowest: std::ptr::null_mut(),
        highest: std::ptr::null_mut(),
        alignment: alignment.max(RESERVE_MIN),
    };
    let mut parameter = ExtendedParameter {
        kind: 1, // MemExtendedParameterAddressRequirements.
        value: (&raw mut requirements).cast(),
    };
    // SAFETY: ABI-compatible parameters point to live stack structures; the OS
    // validates the request and returns a fresh, exclusively owned reservation.
    unsafe { VirtualAlloc2(std::ptr::null_mut(), std::ptr::null_mut(), size, 0x2000, 4, &raw mut parameter, 1).cast() }
}

/// # Safety
/// The page-aligned range must lie in one live reservation owned by the caller.
/// Committing an already committed page is permitted and preserves its data.
pub(crate) unsafe fn commit(address: *mut u8, size: usize) -> bool {
    // SAFETY: Caller owns the reservation and supplies page-granular bounds.
    unsafe { !VirtualAlloc(address.cast(), size, 0x1000, 4).is_null() }
}

/// # Safety
/// The page-aligned range must belong to one reservation, with no remaining
/// accesses or live objects in the range. Returns false when the OS leaves the
/// range committed; the caller may safely retain it as committed cache memory.
pub(crate) unsafe fn decommit(address: *mut u8, size: usize) -> bool {
    // SAFETY: Caller has retired all objects and retains the reservation.
    unsafe { VirtualFree(address.cast(), size, 0x4000) != 0 }
}

/// # Safety
/// `address` is the base of an exclusively owned reservation with no users.
pub(crate) unsafe fn release(address: *mut u8, _size: usize) {
    // SAFETY: Caller is surrendering the entire unused reservation.
    if unsafe { VirtualFree(address.cast(), 0, 0x8000) } == 0 {
        std::process::abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_rejects_invalid_reservation_requests() {
        for (size, alignment) in [(0, RESERVE_MIN), (1, RESERVE_MIN), (RESERVE_MIN, 0), (RESERVE_MIN, 3)] {
            assert!(reserve(size, alignment).is_null());
            assert!(crate::hal::reserve(size, alignment).is_null());
        }
    }
}
