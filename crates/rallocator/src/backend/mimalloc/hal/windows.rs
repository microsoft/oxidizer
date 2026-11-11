// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
// Derived from mimalloc (39448a96), Copyright (c) Microsoft Research, Daan Leijen.
// See ../LICENSE-MIMALLOC.

use std::ffi::{CStr, c_void};
use std::ptr::NonNull;
use std::sync::OnceLock;

use super::{Policy, Reservation, SLICE};

// Win32 allocation and protection flags from memoryapi.h.
const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_DECOMMIT: u32 = 0x4000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READWRITE: u32 = 4;

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

#[repr(C)]
struct SystemInfo {
    architecture: u32,
    page_size: u32,
    minimum_address: *mut c_void,
    maximum_address: *mut c_void,
    active_processor_mask: usize,
    processor_count: u32,
    processor_type: u32,
    allocation_granularity: u32,
    processor_level: u16,
    processor_revision: u16,
}

type VirtualAlloc2 = unsafe extern "system" fn(*mut c_void, *mut c_void, usize, u32, u32, *mut ExtendedParameter, u32) -> *mut c_void;

#[cfg_attr(not(test), derive(Clone, Copy))]
struct Wait<'a> {
    function: unsafe extern "system" fn(u32),
    _lifetime: std::marker::PhantomData<&'a ()>,
    #[cfg(test)]
    observer: Option<&'a mut dyn FnMut(u32)>,
}

impl Wait<'static> {
    const NATIVE: Self = Self {
        function: Sleep,
        _lifetime: std::marker::PhantomData,
        #[cfg(test)]
        observer: None,
    };
}

#[cfg(test)]
impl<'a> Wait<'a> {
    fn recording(observer: &'a mut dyn FnMut(u32)) -> Self {
        Self {
            function: tests::no_wait,
            _lifetime: std::marker::PhantomData,
            observer: Some(observer),
        }
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protection: u32) -> *mut c_void;
    fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> i32;
    fn GetLastError() -> u32;
    fn Sleep(milliseconds: u32);
    fn GetSystemInfo(info: *mut SystemInfo);
    fn GetLargePageMinimum() -> usize;
    fn GetModuleHandleA(name: *const std::ffi::c_char) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
}

fn virtual_alloc2() -> Option<VirtualAlloc2> {
    static FUNCTION: OnceLock<Option<VirtualAlloc2>> = OnceLock::new();
    *FUNCTION.get_or_init(|| {
        // SAFETY: The NUL-terminated static name identifies a core module that
        // remains loaded for the process lifetime. No reference is transferred.
        let module = unsafe { GetModuleHandleA(c"kernelbase.dll".as_ptr()) };
        resolve_virtual_alloc2(NonNull::new(module), |module, name| {
            // SAFETY: The core module remains loaded. Both names selected by
            // resolve_virtual_alloc2 have the documented VirtualAlloc2 ABI.
            unsafe { virtual_alloc2_procedure(module, name) }
        })
    })
}

fn resolve_virtual_alloc2(
    module: Option<NonNull<c_void>>,
    mut lookup: impl FnMut(NonNull<c_void>, &'static CStr) -> Option<VirtualAlloc2>,
) -> Option<VirtualAlloc2> {
    resolve_virtual_alloc2_with(module, &mut lookup)
}

fn resolve_virtual_alloc2_with(
    module: Option<NonNull<c_void>>,
    lookup: &mut dyn FnMut(NonNull<c_void>, &'static CStr) -> Option<VirtualAlloc2>,
) -> Option<VirtualAlloc2> {
    let module = module?;
    match lookup(module, c"VirtualAlloc2FromApp") {
        Some(allocate) => Some(allocate),
        None => lookup(module, c"VirtualAlloc2"),
    }
}

/// # Safety
/// The module must remain loaded. A present named export must have the
/// `VirtualAlloc2` ABI. An absent export returns `None` without conversion.
unsafe fn virtual_alloc2_procedure(module: NonNull<c_void>, name: &CStr) -> Option<VirtualAlloc2> {
    // SAFETY: The caller guarantees module life and the name is NUL-terminated.
    let function = unsafe { GetProcAddress(module.as_ptr(), name.as_ptr()) };
    if function.is_null() {
        None
    } else {
        // SAFETY: The caller guarantees the documented function signature.
        Some(unsafe { std::mem::transmute::<*mut c_void, VirtualAlloc2>(function) })
    }
}

fn reserve_once(size: usize, alignment: usize, committed: bool) -> *mut u8 {
    let flags = MEM_RESERVE | if committed { MEM_COMMIT } else { 0 };
    if alignment > SLICE
        && let Some(allocate) = virtual_alloc2()
    {
        let mut requirements = AddressRequirements {
            lowest: std::ptr::null_mut(),
            highest: std::ptr::null_mut(),
            alignment,
        };
        let mut parameter = ExtendedParameter {
            kind: 1, // MemExtendedParameterAddressRequirements.
            value: (&raw mut requirements).cast(),
        };
        // SAFETY: The ABI-compatible stack parameters remain live during the
        // call. A null process selects this process, and no address is forced.
        let address = unsafe {
            allocate(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                size,
                flags,
                PAGE_READWRITE,
                &raw mut parameter,
                1,
            )
        };
        if !address.is_null() {
            return address.cast();
        }
    }
    // SAFETY: The OS selects a fresh reservation. The caller checks alignment.
    unsafe { VirtualAlloc(std::ptr::null_mut(), size, flags, PAGE_READWRITE).cast() }
}

fn reserve_raw(size: usize, alignment: usize, committed: bool) -> *mut u8 {
    reserve_raw_with(size, alignment, committed, reserve_once, Wait::NATIVE)
}

fn reserve_raw_with(
    size: usize,
    alignment: usize,
    committed: bool,
    mut allocate: impl FnMut(usize, usize, bool) -> *mut u8,
    wait: Wait<'_>,
) -> *mut u8 {
    reserve_raw_driver(size, alignment, committed, &mut allocate, wait)
}

fn reserve_raw_driver(
    size: usize,
    alignment: usize,
    committed: bool,
    allocate: &mut dyn FnMut(usize, usize, bool) -> *mut u8,
    wait: Wait<'_>,
) -> *mut u8 {
    #[cfg(test)]
    let mut wait = wait;
    // The 400-ms OOM budget applies to reserve+commit, not _mi_prim_commit.
    let mut remaining = 400;
    for attempt in 1..=10 {
        let address = allocate(size, alignment, committed);
        if !address.is_null() {
            return address;
        }
        // SAFETY: GetLastError reads this thread's most recent Win32 error.
        let error = unsafe { GetLastError() };
        let Some(delay) = retry_delay(remaining, attempt, alignment, committed, error) else {
            break;
        };
        remaining -= delay;
        #[cfg(test)]
        if let Some(observer) = wait.observer.as_mut() {
            observer(delay);
        }
        // SAFETY: Wait only contains Sleep or the test's duration recorder.
        // Both accept every u32 duration and retain no caller memory.
        unsafe { (wait.function)(delay) };
    }
    std::ptr::null_mut()
}

fn retry_delay(remaining: u32, attempt: u32, alignment: usize, committed: bool, error: u32) -> Option<u32> {
    // WinError.h: NOT_ENOUGH_MEMORY, PAGEFILE_QUOTA, COMMITMENT_LIMIT,
    // COMMITMENT_MINIMUM. The alignment cap is from prim/windows/prim.c.
    let delay = (attempt * 40).min(remaining);
    (remaining > 0 && committed && alignment <= 8 * 1024 * 1024 && matches!(error, 8 | 1454 | 1455 | 635)).then_some(delay)
}

pub(super) fn reserve(size: usize, alignment: usize, committed: bool) -> Option<Reservation> {
    reserve_with(size, alignment, committed, raw_reservation, commit)
}

fn raw_reservation(size: usize, alignment: usize, committed: bool) -> Option<Reservation> {
    let address = reserve_raw(size, alignment, committed);
    NonNull::new(address).map(|address| Reservation {
        address: address.as_ptr(),
        size,
        base: address.as_ptr(),
        reserved_size: size,
    })
}

fn reserve_with(
    size: usize,
    alignment: usize,
    committed: bool,
    mut allocate: impl FnMut(usize, usize, bool) -> Option<Reservation>,
    commit_range: unsafe fn(*mut u8, usize) -> bool,
) -> Option<Reservation> {
    reserve_driver(size, alignment, committed, &mut allocate, commit_range)
}

fn reserve_driver(
    size: usize,
    alignment: usize,
    committed: bool,
    allocate: &mut dyn FnMut(usize, usize, bool) -> Option<Reservation>,
    commit_range: unsafe fn(*mut u8, usize) -> bool,
) -> Option<Reservation> {
    if let Some(reservation) = allocate(size, alignment, committed) {
        if reservation.as_ptr().addr().is_multiple_of(alignment) {
            return Some(reservation);
        }
        // Dropping the unpublished mapping releases its original reservation.
        drop(reservation);
    }

    // VirtualAlloc2 can be absent or reject address requirements. Retain the
    // full fallback reservation because VirtualFree cannot unmap its edges.
    let reserved_size = size.checked_add(alignment)?;
    if isize::try_from(reserved_size).is_err() {
        return None;
    }
    let mut reservation = allocate(reserved_size, 1, false)?;
    let offset = reservation.as_ptr().addr().wrapping_neg() & (alignment - 1);
    if offset > reservation.len() || size > reservation.len() - offset {
        return None;
    }
    // SAFETY: The over-reservation includes at least alignment bytes of slack.
    let address = unsafe { reservation.as_ptr().add(offset) };
    reservation.address = address;
    reservation.size = size;
    // SAFETY: The aligned interior range is owned by this new reservation.
    if committed && !unsafe { commit_range(address, size) } {
        return None;
    }
    Some(reservation)
}

pub(super) unsafe fn commit(address: *mut u8, size: usize) -> bool {
    // SAFETY: The HAL supplies a live, owned OS-page-aligned range. Windows
    // preserves existing committed bytes when the range is only partly new.
    !unsafe { VirtualAlloc(address.cast(), size, MEM_COMMIT, PAGE_READWRITE) }.is_null()
}

pub(super) unsafe fn purge(address: *mut u8, size: usize) -> bool {
    // SAFETY: The HAL caller retired all accesses to this reservation range.
    // Decommit is best-effort backing reclamation. Like _mi_prim_decommit,
    // assume recommit is required even if Windows reports an error.
    unsafe { VirtualFree(address.cast(), size, MEM_DECOMMIT) };
    true
}

pub(super) unsafe fn reuse(_address: *mut u8, _size: usize) {}

// prim/windows/prim.c reads this thread environment block field on Windows Vista and later. The
// crate's Windows 10 floor includes it on both supported 64-bit architectures.
const THREADPOOL_DATA_OFFSET: usize = 0x1778;

pub(super) fn is_in_threadpool() -> bool {
    let teb = super::architecture::thread_environment_block();
    // SAFETY: The architecture getter returns the current, live Windows `TEB`.
    // Its aligned 64-bit thread-pool field exists on every supported version.
    unsafe { has_threadpool_data(teb) }
}

/// # Safety
/// `teb` must cover an initialized, pointer-aligned thread-pool field at the
/// Windows `TEB` offset. Its owner must coordinate writes with this read.
unsafe fn has_threadpool_data(teb: *const u8) -> bool {
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "Windows thread environment block and its 0x1778 field are pointer-aligned on both supported 64-bit architectures"
    )]
    let slot = teb.wrapping_add(THREADPOOL_DATA_OFFSET).cast::<usize>();
    // SAFETY: The caller guarantees alignment, initialization, and field life.
    unsafe { slot.read() != 0 }
}

pub(super) unsafe fn release(address: *mut u8, _size: usize) {
    // SAFETY: The HAL supplies the original, exclusively owned reservation base.
    let released = unsafe { VirtualFree(address.cast(), 0, MEM_RELEASE) };
    // Releasing an owned whole reservation cannot fail without an invalid
    // handle, external unmapping, or an OS ownership invariant violation.
    crate::backend::abort::require(released != 0);
}

pub(super) fn policy() -> Policy {
    let mut info = std::mem::MaybeUninit::uninit();
    // SAFETY: GetSystemInfo initializes the complete ABI-compatible structure.
    unsafe { GetSystemInfo(info.as_mut_ptr()) };
    // SAFETY: GetSystemInfo has no failure path and initializes every field.
    let info = unsafe { info.assume_init() };
    // SAFETY: This process-wide query takes no pointers or mutable state.
    let large_page_size = unsafe { GetLargePageMinimum() };
    Policy {
        eager_commit: false,
        purge_slices: 1,
        page_size: info.page_size as usize,
        large_page_size,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::ffi::{CStr, c_void};
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{
        GetModuleHandleA, Reservation, SLICE, THREADPOOL_DATA_OFFSET, VirtualAlloc2, Wait, commit, has_threadpool_data, raw_reservation,
        reserve_once, reserve_raw_with, reserve_with, resolve_virtual_alloc2, retry_delay, virtual_alloc2_procedure,
    };

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateThreadpoolWork(
            callback: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void),
            context: *mut c_void,
            environment: *mut c_void,
        ) -> *mut c_void;
        fn SubmitThreadpoolWork(work: *mut c_void);
        fn WaitForThreadpoolWorkCallbacks(work: *mut c_void, cancel: i32);
        fn CloseThreadpoolWork(work: *mut c_void);
        fn SetLastError(error: u32);
    }

    pub(super) extern "system" fn no_wait(_milliseconds: u32) {}

    fn native_module_and_allocator() -> (NonNull<c_void>, VirtualAlloc2) {
        // SAFETY: The NUL-terminated static name identifies a core process DLL.
        let module = unsafe { GetModuleHandleA(c"kernelbase.dll".as_ptr()) };
        let module = NonNull::new(module).unwrap();
        // SAFETY: VirtualAlloc2 has this ABI and exists at the Windows 10 floor.
        let allocate = unsafe { virtual_alloc2_procedure(module, c"VirtualAlloc2") }.unwrap();
        (module, allocate)
    }

    fn misaligned_reservation(size: usize) -> Reservation {
        let mut reservation = raw_reservation(size + 2 * SLICE, SLICE, false).unwrap();
        // Force bit 16 on: a 64-KiB-aligned address is then not 128-KiB aligned.
        // The extra mapping slack keeps the complete requested range owned.
        let offset = (reservation.address.addr() | SLICE) - reservation.address.addr();
        // SAFETY: offset is zero or one slice within the over-reservation.
        reservation.address = unsafe { reservation.address.add(offset) };
        reservation.size -= offset;
        reservation
    }

    #[test]
    fn missing_modules_skip_lookup_and_missing_exports_try_both_names() {
        let (module, _) = native_module_and_allocator();
        let names = RefCell::new(Vec::new());
        let mut lookup = |_: NonNull<c_void>, name: &'static CStr| {
            names.borrow_mut().push(name);
            None
        };

        let absent_module = resolve_virtual_alloc2(None, &mut lookup).is_none();
        let calls_without_module = names.borrow().len();
        let absent_exports = resolve_virtual_alloc2(Some(module), &mut lookup).is_none();

        assert_eq!(
            (absent_module, calls_without_module, absent_exports, names.into_inner()),
            (true, 0, true, vec![c"VirtualAlloc2FromApp", c"VirtualAlloc2"])
        );
    }

    #[test]
    fn aligned_allocation_prefers_the_app_export() {
        let (module, native) = native_module_and_allocator();
        let mut names = Vec::new();

        let found = resolve_virtual_alloc2(Some(module), |_, name| {
            names.push(name);
            Some(native)
        })
        .unwrap();

        assert_eq!((std::ptr::fn_addr_eq(found, native), names), (true, vec![c"VirtualAlloc2FromApp"]));
    }

    #[test]
    fn aligned_allocation_uses_the_standard_export_when_needed() {
        let (module, native) = native_module_and_allocator();
        let mut names = Vec::new();

        let found = resolve_virtual_alloc2(Some(module), |_, name| {
            names.push(name);
            (name == c"VirtualAlloc2").then_some(native)
        })
        .unwrap();

        assert_eq!(
            (std::ptr::fn_addr_eq(found, native), names),
            (true, vec![c"VirtualAlloc2FromApp", c"VirtualAlloc2"])
        );
    }

    #[test]
    fn absent_native_exports_are_not_converted_to_function_pointers() {
        let (module, _) = native_module_and_allocator();

        // SAFETY: This immutable system DLL has no rallocator-specific export.
        // An absent name never reaches the function-pointer conversion.
        let missing = unsafe { virtual_alloc2_procedure(module, c"__rallocator_missing_virtual_alloc2__") };

        assert!(missing.is_none());
    }

    #[test]
    fn zero_sized_native_requests_fail_with_and_without_address_requirements() {
        let aligned = reserve_once(0, 2 * SLICE, false);
        let ordinary = raw_reservation(0, SLICE, false).is_none();

        assert_eq!((aligned.is_null(), ordinary), (true, true));
    }

    #[test]
    fn commitment_retry_preserves_request_parameters_and_uses_the_wait_hook() {
        let mut requests = Vec::new();
        let mut waits = Vec::new();
        let expected = std::ptr::without_provenance_mut::<u8>(SLICE);

        let result = reserve_raw_with(
            SLICE,
            SLICE,
            true,
            |size, alignment, committed| {
                requests.push((size, alignment, committed));
                if requests.len() == 1 {
                    // SAFETY: Last-error state belongs to this test thread.
                    unsafe { SetLastError(1455) };
                    std::ptr::null_mut()
                } else {
                    expected
                }
            },
            Wait::recording(&mut |delay| {
                waits.push(delay);
            }),
        );

        assert_eq!((result, requests, waits), (expected, vec![(SLICE, SLICE, true); 2], vec![40]));
    }

    #[test]
    fn commitment_retry_exhausts_its_budget_and_returns_failure() {
        let calls = Cell::new(0);
        let mut waits = Vec::new();

        let result = reserve_raw_with(
            SLICE,
            SLICE,
            true,
            |_, _, _| {
                calls.set(calls.get() + 1);
                // SAFETY: This test controls only its own thread's error state.
                unsafe { SetLastError(8) };
                std::ptr::null_mut()
            },
            Wait::recording(&mut |delay| waits.push(delay)),
        );

        assert_eq!((result.is_null(), calls.get(), waits), (true, 5, vec![40, 80, 120, 160]));
    }

    #[test]
    fn native_retry_wait_can_reach_a_successful_second_attempt() {
        let calls = Cell::new(0);
        let expected = std::ptr::without_provenance_mut::<u8>(SLICE);

        let result = reserve_raw_with(
            SLICE,
            SLICE,
            true,
            |_, _, _| {
                calls.set(calls.get() + 1);
                if calls.get() == 1 {
                    // SAFETY: Last-error state belongs to this test thread.
                    unsafe { SetLastError(1455) };
                    std::ptr::null_mut()
                } else {
                    expected
                }
            },
            Wait::NATIVE,
        );

        assert_eq!((result, calls.get()), (expected, 2));
    }

    #[test]
    fn noncommit_and_nonmemory_failures_do_not_wait() {
        let waits = RefCell::new(Vec::new());
        let mut wait = |delay| waits.borrow_mut().push(delay);
        let failures = [(false, 1455), (true, 487)].map(|(committed, error)| {
            let calls = Cell::new(0);
            let result = reserve_raw_with(
                SLICE,
                SLICE,
                committed,
                |_, _, _| {
                    calls.set(calls.get() + 1);
                    // SAFETY: Last-error state is local to this test thread.
                    unsafe { SetLastError(error) };
                    std::ptr::null_mut()
                },
                Wait::recording(&mut wait),
            );
            (result.is_null(), calls.get())
        });
        let before_retry = waits.borrow().clone();
        let retry_calls = Cell::new(0);
        let expected = std::ptr::without_provenance_mut::<u8>(SLICE);

        let recovered = reserve_raw_with(
            SLICE,
            SLICE,
            true,
            |_, _, _| {
                retry_calls.set(retry_calls.get() + 1);
                if retry_calls.get() == 1 {
                    // SAFETY: This positive control uses the same local error slot.
                    unsafe { SetLastError(1455) };
                    std::ptr::null_mut()
                } else {
                    expected
                }
            },
            Wait::recording(&mut wait),
        );

        assert_eq!(
            (failures, before_retry, recovered, waits.into_inner()),
            ([(true, 1); 2], vec![], expected, vec![40])
        );
    }

    #[test]
    fn misaligned_reservations_fall_back_without_losing_the_original_owner() {
        let mut requests = Vec::new();

        let reservation = reserve_with(
            SLICE,
            2 * SLICE,
            true,
            |size, alignment, committed| {
                requests.push((size, alignment, committed));
                Some(misaligned_reservation(size))
            },
            commit,
        )
        .unwrap();
        // SAFETY: The fallback committed the complete aligned data range.
        unsafe { reservation.as_ptr().write(0xB7) };
        // SAFETY: The first byte remains in the same exclusively owned mapping.
        let byte = unsafe { reservation.as_ptr().read() };

        assert_eq!(
            (
                reservation.as_ptr().addr() % (2 * SLICE),
                reservation.len(),
                reservation.as_ptr() != reservation.base,
                reservation.reserved_size,
                byte,
                requests,
            ),
            (
                0,
                SLICE,
                true,
                5 * SLICE,
                0xB7,
                vec![(SLICE, 2 * SLICE, true), (3 * SLICE, 1, false)]
            )
        );
    }

    #[test]
    fn a_failed_direct_reservation_can_recover_with_uncommitted_overallocation() {
        let mut requests = Vec::new();

        let reservation = reserve_with(
            SLICE,
            2 * SLICE,
            false,
            |size, alignment, committed| {
                requests.push((size, alignment, committed));
                (requests.len() > 1).then(|| misaligned_reservation(size))
            },
            commit,
        )
        .unwrap();

        assert_eq!(
            (reservation.as_ptr().addr() % (2 * SLICE), reservation.len(), requests),
            (0, SLICE, vec![(SLICE, 2 * SLICE, false), (3 * SLICE, 1, false)])
        );
    }

    #[test]
    fn a_failed_fallback_reservation_returns_no_owner() {
        let mut requests = Vec::new();

        let failed = reserve_with(
            SLICE,
            2 * SLICE,
            true,
            |size, alignment, committed| {
                requests.push((size, alignment, committed));
                None
            },
            commit,
        )
        .is_none();

        assert_eq!((failed, requests), (true, vec![(SLICE, 2 * SLICE, true), (3 * SLICE, 1, false)]));
    }

    #[test]
    fn failed_fallback_commitment_drops_the_full_uncommitted_reservation() {
        unsafe fn reject_commit(_address: *mut u8, _size: usize) -> bool {
            false
        }

        let calls = Cell::new(0);
        let failed = reserve_with(
            SLICE,
            2 * SLICE,
            true,
            |size, _, _| {
                calls.set(calls.get() + 1);
                (calls.get() > 1).then(|| misaligned_reservation(size))
            },
            reject_commit,
        )
        .is_none();

        assert_eq!((failed, calls.get()), (true, 2));
    }

    #[test]
    fn undersized_fallback_mappings_are_rejected_before_commitment() {
        let calls = Cell::new(0);
        let failed = reserve_with(
            SLICE,
            2 * SLICE,
            true,
            |_, _, _| {
                calls.set(calls.get() + 1);
                (calls.get() > 1).then(|| {
                    let mut mapping = misaligned_reservation(SLICE);
                    mapping.size = SLICE;
                    mapping
                })
            },
            commit,
        )
        .is_none();

        assert_eq!((failed, calls.get()), (true, 2));
    }

    #[test]
    fn overreservation_size_checks_precede_the_fallback_os_call() {
        let failures = [usize::MAX, isize::MAX as usize].map(|size| {
            let calls = Cell::new(0);
            let result = reserve_with(
                size,
                SLICE,
                false,
                |_, _, _| {
                    calls.set(calls.get() + 1);
                    None
                },
                commit,
            );
            (result.is_none(), calls.get())
        });

        assert_eq!(failures, [(true, 1); 2]);
    }

    #[test]
    fn commitment_retry_stays_within_the_default_budget() {
        let mut remaining = 400;
        let delays = (1..=5)
            .map(|attempt| {
                let delay = retry_delay(remaining, attempt, SLICE, true, 1455);
                remaining -= delay.unwrap_or(0);
                delay
            })
            .collect::<Vec<_>>();

        assert_eq!(delays, [Some(40), Some(80), Some(120), Some(160), None]);
    }

    #[test]
    fn uncommitted_reservations_do_not_wait_for_commitment() {
        assert_eq!(retry_delay(400, 1, SLICE, false, 1455), None);
    }

    #[test]
    fn large_alignments_do_not_wait_for_commitment() {
        assert_eq!(retry_delay(400, 1, 16 * 1024 * 1024, true, 1455), None);
    }

    #[test]
    fn invalid_address_errors_are_not_retried() {
        assert_eq!(retry_delay(400, 1, SLICE, true, 487), None);
    }

    #[test]
    fn only_the_windows_threadpool_slot_controls_detection() {
        let index = THREADPOOL_DATA_OFFSET / std::mem::size_of::<usize>();
        let mut teb = [1_usize; THREADPOOL_DATA_OFFSET / std::mem::size_of::<usize>() + 1];
        teb[index] = 0;
        // SAFETY: The aligned fake thread environment block covers the field.
        let absent = unsafe { has_threadpool_data(teb.as_ptr().cast()) };
        teb[index] = 1;
        // SAFETY: No concurrent accesses exist while the fake field changes.
        let present = unsafe { has_threadpool_data(teb.as_ptr().cast()) };

        assert_eq!((absent, present), (false, true));
    }

    #[test]
    fn detects_a_native_windows_threadpool_callback() {
        unsafe extern "system" fn callback(_instance: *mut c_void, context: *mut c_void, _work: *mut c_void) {
            // SAFETY: The test keeps its initialized AtomicBool alive until
            // WaitForThreadpoolWorkCallbacks confirms callback completion.
            let detected = unsafe { &*context.cast::<AtomicBool>() };
            detected.store(super::is_in_threadpool(), Ordering::Release);
        }

        struct Work(NonNull<c_void>);

        impl Drop for Work {
            fn drop(&mut self) {
                // SAFETY: This test exclusively owns the native work object.
                // Waiting protects the stack callback context during unwinding.
                unsafe { WaitForThreadpoolWorkCallbacks(self.0.as_ptr(), 0) };
                // SAFETY: All callbacks completed and no future work is queued.
                unsafe { CloseThreadpoolWork(self.0.as_ptr()) };
            }
        }

        let detected = AtomicBool::new(false);
        // SAFETY: The context remains live until Work drops. The callback has
        // the documented ABI and the default callback environment is valid.
        let work = unsafe { CreateThreadpoolWork(callback, std::ptr::from_ref(&detected).cast_mut().cast(), std::ptr::null_mut()) };
        let work = Work(NonNull::new(work).unwrap());

        // SAFETY: This is a live work object with a valid callback and context.
        unsafe { SubmitThreadpoolWork(work.0.as_ptr()) };
        // SAFETY: The object stays live while this waits for the queued callback.
        unsafe { WaitForThreadpoolWorkCallbacks(work.0.as_ptr(), 0) };

        assert!(detected.load(Ordering::Acquire));
    }
}
