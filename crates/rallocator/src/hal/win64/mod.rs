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
    #[cfg(test)]
    fn GetErrorMode() -> u32;
    #[cfg(test)]
    fn SetErrorMode(mode: u32) -> u32;
}

#[cfg(test)]
pub(crate) fn disable_test_crash_reporting() {
    const SEM_NOGPFAULTERRORBOX: u32 = 0x0002;
    // SAFETY: GetErrorMode reads this process's flags without accessing pointers.
    let previous = unsafe { GetErrorMode() };
    // SAFETY: Only the fatal-path test subprocess calls this; existing flags
    // are preserved, and abort still terminates with its original exit status.
    unsafe { SetErrorMode(previous | SEM_NOGPFAULTERRORBOX) };
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
    wait_with(word, expected, |address, compare, size, milliseconds| {
        // SAFETY: Both addresses refer to the live equally sized words supplied by wait_with.
        unsafe { WaitOnAddress(address, compare, size, milliseconds) }
    });
}

fn wait_with(word: &AtomicU32, expected: u32, mut wait: impl FnMut(*const c_void, *const c_void, usize, u32) -> i32) {
    while word.load(Ordering::Acquire) == expected {
        // #[gamma::skip(stmt.delete_call, tag = "timeout", reason = "November 6 campaign 087718788bb4: deleting this wait exceeded the timeout")]
        wait(
            std::ptr::from_ref::<AtomicU32>(word).cast(),
            (&raw const expected).cast(),
            std::mem::size_of::<u32>(),
            u32::MAX,
        );
    }
}

// #[gamma::skip(fn_value.unit, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
/// Wakes a waiter by numeric address; the OS does not access target memory.
pub(crate) fn wake_one(address: *const u32) {
    // SAFETY: WakeByAddressSingle uses the address as a wait-table key, and does
    // not dereference it (the waiter may already have returned).
    unsafe { WakeByAddressSingle(address.cast()) };
}

/// Reserves inaccessible address space; no Rust allocation is performed.
pub(crate) fn reserve(size: usize, alignment: usize) -> *mut u8 {
    reserve_with(size, alignment, |size, flags, protection, requirements| {
        let mut parameter = ExtendedParameter {
            kind: 1, // MemExtendedParameterAddressRequirements.
            value: std::ptr::from_mut(requirements).cast(),
        };
        // SAFETY: ABI-compatible parameters point to live stack structures; the OS
        // validates the request and returns a fresh, exclusively owned reservation.
        unsafe {
            VirtualAlloc2(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                size,
                flags,
                protection,
                &raw mut parameter,
                1,
            )
            .cast()
        }
    })
}

fn reserve_with(size: usize, alignment: usize, allocate: impl FnOnce(usize, u32, u32, &mut AddressRequirements) -> *mut u8) -> *mut u8 {
    if size == 0 || !size.is_multiple_of(RESERVE_MIN) || !alignment.is_power_of_two() {
        return std::ptr::null_mut();
    }
    let mut requirements = AddressRequirements {
        lowest: std::ptr::null_mut(),
        highest: std::ptr::null_mut(),
        alignment: alignment.max(RESERVE_MIN),
    };
    allocate(size, 0x2000, 4, &mut requirements)
}

/// # Safety
/// The page-aligned range must lie in one live reservation owned by the caller.
/// Committing an already committed page is permitted and preserves its data.
pub(crate) unsafe fn commit(address: *mut u8, size: usize) -> bool {
    commit_with(address, size, |address, size, flags, protection| {
        // SAFETY: Caller owns the reservation and supplies page-granular bounds.
        unsafe { native_allocate(address.cast(), size, flags, protection).cast() }
    })
}

fn commit_with(address: *mut u8, size: usize, commit: impl FnOnce(*mut u8, usize, u32, u32) -> *mut u8) -> bool {
    !commit(address, size, 0x1000, 4).is_null()
}

/// # Safety
/// The page-aligned range must belong to one reservation, with no remaining
/// accesses or live objects in the range. Returns false when the OS leaves the
/// range committed; the caller may safely retain it as committed cache memory.
pub(crate) unsafe fn decommit(address: *mut u8, size: usize) -> bool {
    decommit_with(address, size, |address, size, flags| {
        // SAFETY: Caller has retired all objects and retains the reservation.
        unsafe { native_free(address.cast(), size, flags) }
    })
}

fn decommit_with(address: *mut u8, size: usize, decommit: impl FnOnce(*mut u8, usize, u32) -> i32) -> bool {
    decommit(address, size, 0x4000) != 0
}

/// # Safety
/// `address` is the base of an exclusively owned reservation with no users.
// #[gamma::skip(parameter.default_shadow, reason = "The generated replacement shadows only the unused usize _size parameter; Windows MEM_RELEASE always receives zero size through release_with and never reads _size")]
pub(crate) unsafe fn release(address: *mut u8, _size: usize) {
    release_with(address, |address, size, flags| {
        // SAFETY: Caller is surrendering the entire unused reservation.
        unsafe { native_free(address.cast(), size, flags) }
    });
}

fn release_with(address: *mut u8, release: impl FnOnce(*mut u8, usize, u32) -> i32) {
    crate::abort::require(release(address, 0, 0x8000) != 0);
}

unsafe fn native_allocate(address: *mut c_void, size: usize, flags: u32, protection: u32) -> *mut c_void {
    let allocate = VirtualAlloc;
    #[cfg(test)]
    let allocate: unsafe extern "system" fn(*mut c_void, usize, u32, u32) -> *mut c_void =
        if native_probe::active() { native_probe::allocate } else { allocate };
    // SAFETY: The caller forwards its owned reservation and validated native flags.
    unsafe { allocate(address, size, flags, protection) }
}

unsafe fn native_free(address: *mut c_void, size: usize, flags: u32) -> i32 {
    let free = VirtualFree;
    #[cfg(test)]
    let free: unsafe extern "system" fn(*mut c_void, usize, u32) -> i32 = if native_probe::active() { native_probe::free } else { free };
    // SAFETY: The caller owns and has retired the entire requested reservation range.
    unsafe { free(address, size, flags) }
}

#[cfg(test)]
mod native_probe {
    use std::cell::RefCell;

    use super::c_void;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) enum Call {
        Allocate(usize, usize, u32, u32),
        Free(usize, usize, u32),
    }

    struct State {
        succeed: bool,
        calls: Vec<Call>,
    }

    std::thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    }

    pub(super) fn active() -> bool {
        STATE.with_borrow(Option::is_some)
    }

    pub(super) unsafe extern "system" fn allocate(address: *mut c_void, size: usize, flags: u32, protection: u32) -> *mut c_void {
        STATE.with_borrow_mut(|state| {
            let state = state.as_mut().unwrap();
            state.calls.push(Call::Allocate(address.addr(), size, flags, protection));
            if state.succeed { address } else { std::ptr::null_mut() }
        })
    }

    pub(super) unsafe extern "system" fn free(address: *mut c_void, size: usize, flags: u32) -> i32 {
        STATE.with_borrow_mut(|state| {
            let state = state.as_mut().unwrap();
            state.calls.push(Call::Free(address.addr(), size, flags));
            i32::from(state.succeed)
        })
    }

    #[derive(Debug)]
    pub(super) struct Probe {
        thread: std::thread::ThreadId,
    }

    impl Probe {
        pub(super) fn start(succeed: bool) -> Self {
            STATE.with_borrow_mut(|state| {
                assert!(state.is_none());
                *state = Some(State {
                    succeed,
                    calls: Vec::new(),
                });
            });
            Self {
                thread: std::thread::current().id(),
            }
        }

        pub(super) fn calls(&self) -> Vec<Call> {
            assert_eq!(self.thread, std::thread::current().id());
            STATE.with_borrow(|state| state.as_ref().unwrap().calls.clone())
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            STATE.with_borrow_mut(|state| *state = None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamma_nov4_actual_hal_and_windows_wrappers_forward_exact_native_ranges_results_and_release_once() {
        use native_probe::{Call, Probe};

        let address = std::ptr::without_provenance_mut::<u8>(0x10000);
        for succeed in [false, true] {
            let probe = Probe::start(succeed);
            // SAFETY: The scoped native adapter models this reservation; no OS or memory access occurs.
            assert_eq!(unsafe { crate::hal::commit(address, PAGE * 2) }, succeed);
            // SAFETY: The scoped adapter owns the same modeled reservation and retired pages.
            assert_eq!(unsafe { crate::hal::decommit(address, PAGE * 2) }, succeed);
            // SAFETY: The scoped native adapter models a page-aligned caller-owned reservation.
            assert_eq!(unsafe { commit(address, PAGE) }, succeed);
            // SAFETY: The same modeled reservation has no live objects or real memory accesses.
            assert_eq!(unsafe { decommit(address, PAGE) }, succeed);
            assert_eq!(
                probe.calls(),
                [
                    Call::Allocate(0x10000, PAGE * 2, 0x1000, 4),
                    Call::Free(0x10000, PAGE * 2, 0x4000),
                    Call::Allocate(0x10000, PAGE, 0x1000, 4),
                    Call::Free(0x10000, PAGE, 0x4000),
                ]
            );
        }
        let probe = Probe::start(true);
        // SAFETY: The scoped adapter owns the modeled retired reservation; release never reaches the OS.
        unsafe { release(address, RESERVE_MIN * 2) };
        assert_eq!(probe.calls(), [Call::Free(0x10000, 0, 0x8000)]);
    }

    #[test]
    fn gamma_nov4_wait_retries_spurious_wakes_with_exact_infinite_timeout_until_the_word_changes() {
        let word = AtomicU32::new(17);
        let mut calls = 0;
        wait_with(&word, 17, |address, compare, size, timeout| {
            assert_eq!(address, std::ptr::from_ref(&word).cast());
            assert_eq!((size, timeout), (size_of::<u32>(), u32::MAX));
            // SAFETY: wait_with borrows its initialized expected word for the duration of this callback.
            assert_eq!(unsafe { compare.cast::<u32>().read() }, 17);
            calls += 1;
            if calls == 3 {
                word.store(19, Ordering::Release);
            }
            1
        });
        assert_eq!(calls, 3);
        assert_eq!(word.load(Ordering::Relaxed), 19);
        wait_with(&word, 17, |_, _, _, _| panic!("a changed word cannot start waiting"));
    }

    #[test]
    fn gamma_round2_windows_memory_plans_preserve_exact_native_arguments() {
        for (size, alignment) in [(0, RESERVE_MIN), (1, RESERVE_MIN), (RESERVE_MIN, 3)] {
            let result = reserve_with(size, alignment, |_, _, _, _| {
                panic!("invalid input must not reach the native allocator")
            });
            assert!(result.is_null());
        }
        let mut byte = 0_u8;
        let address = &raw mut byte;
        let mut calls = Vec::new();
        let reserved = reserve_with(RESERVE_MIN * 2, 1, |size, flags, protection, requirements| {
            assert!(requirements.lowest.is_null());
            assert!(requirements.highest.is_null());
            assert_eq!(requirements.alignment, RESERVE_MIN);
            calls.push((size, flags, protection));
            address
        });
        assert_eq!(reserved, address);
        assert_eq!(calls, [(RESERVE_MIN * 2, 0x2000, 4)]);
        assert!(commit_with(address, RESERVE_MIN, |pointer, size, flags, protection| {
            assert_eq!(pointer, address);
            assert_eq!((size, flags, protection), (RESERVE_MIN, 0x1000, 4));
            pointer
        }));
        assert!(!commit_with(address, RESERVE_MIN, |_, _, _, _| std::ptr::null_mut()));
        assert!(decommit_with(address, RESERVE_MIN, |pointer, size, flags| {
            assert_eq!(pointer, address);
            assert_eq!((size, flags), (RESERVE_MIN, 0x4000));
            1
        }));
        assert!(!decommit_with(address, RESERVE_MIN, |_, _, _| 0));
        let mut releases = 0;
        release_with(address, |pointer, size, flags| {
            assert_eq!(pointer, address);
            assert_eq!((size, flags), (0, 0x8000));
            releases += 1;
            1
        });
        assert_eq!(releases, 1);
    }

    #[test]
    fn platform_rejects_invalid_reservation_requests() {
        for (size, alignment) in [(0, RESERVE_MIN), (1, RESERVE_MIN), (RESERVE_MIN, 0), (RESERVE_MIN, 3)] {
            assert!(reserve(size, alignment).is_null());
            assert!(crate::hal::reserve(size, alignment).is_null());
        }
    }
}
