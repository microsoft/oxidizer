// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Darwin memory reservations, lazy page reclamation and public address waits.

use std::sync::atomic::{AtomicU32, Ordering};

mod mutex;
pub(crate) use mutex::{Mutex, MutexGuard};

#[cfg(target_arch = "aarch64")]
pub(crate) const PAGE: usize = 16384;
#[cfg(target_arch = "x86_64")]
pub(crate) const PAGE: usize = 4096;
pub(crate) const RESERVE_MIN: usize = 65536;

const VM_FLAGS_ANYWHERE: libc::c_int = 1;
const VM_INHERIT_COPY: u32 = 1;
#[cfg(test)]
const OS_CLOCK_MACH_ABSOLUTE_TIME: u32 = 32;
#[cfg(test)]
const WAIT_TIMEOUT_NANOS: u64 = 30_000_000_000;
// Limit transient reuse failures to four syscalls, yielding between retries.
const REUSE_MAX_ATTEMPTS: usize = 4;

#[link(name = "System")]
unsafe extern "C" {
    static mut mach_task_self_: u32;
    fn mach_vm_map(
        task: u32,
        address: *mut u64,
        size: u64,
        mask: u64,
        flags: libc::c_int,
        object: u32,
        offset: u64,
        copy: libc::c_int,
        protection: libc::c_int,
        maximum_protection: libc::c_int,
        inheritance: u32,
    ) -> libc::c_int;
    #[cfg(test)]
    fn mach_vm_protect(task: u32, address: u64, size: u64, set_maximum: libc::c_int, protection: libc::c_int) -> libc::c_int;
    fn os_sync_wait_on_address(address: *mut libc::c_void, value: u64, size: usize, flags: u32) -> libc::c_int;
    #[cfg(test)]
    fn os_sync_wait_on_address_with_timeout(
        address: *mut libc::c_void,
        value: u64,
        size: usize,
        flags: u32,
        clock: u32,
        timeout: u64,
    ) -> libc::c_int;
    fn os_sync_wake_by_address_any(address: *mut libc::c_void, size: usize, flags: u32) -> libc::c_int;
}

pub(crate) fn reserve(size: usize, alignment: usize) -> *mut u8 {
    // SAFETY: sysconf reads a process-wide constant without pointer arguments.
    reserve_with_page_size(size, alignment, unsafe { libc::sysconf(libc::_SC_PAGESIZE) })
}

fn reserve_with_page_size(size: usize, alignment: usize, page_size: libc::c_long) -> *mut u8 {
    if usize::try_from(page_size) != Ok(PAGE) || size == 0 || !size.is_multiple_of(RESERVE_MIN) || !alignment.is_power_of_two() {
        return std::ptr::null_mut();
    }
    let mut address = 0_u64;
    // SAFETY: libSystem initializes this task port before program startup.
    let task = unsafe { mach_task_self_ };
    // SAFETY: The live output word and alignment mask request a fresh, inaccessible
    // anonymous reservation, with no Rust allocator or partial unmapping.
    let result = unsafe {
        mach_vm_map(
            task,
            &raw mut address,
            size as u64,
            (alignment.max(PAGE) - 1) as u64,
            VM_FLAGS_ANYWHERE,
            0,
            0,
            1,
            libc::PROT_NONE,
            libc::PROT_READ | libc::PROT_WRITE,
            VM_INHERIT_COPY,
        )
    };
    if result == 0 { address as *mut u8 } else { std::ptr::null_mut() }
}

/// # Safety
/// The page-granular range belongs to a live caller-owned reservation.
pub(crate) unsafe fn commit(address: *mut u8, size: usize) -> bool {
    // SAFETY: Changing access preserves existing contents, including live
    // neighbors in a shared pagemap page.
    if unsafe { libc::mprotect(address.cast(), size, libc::PROT_READ | libc::PROT_WRITE) } != 0 {
        return false;
    }
    // SAFETY: The range is now writable; clearing its reusable state prevents
    // reclamation before the caller can publish new objects in it.
    unsafe { advise(address, size, libc::MADV_FREE_REUSE) }
}

/// # Safety
/// All objects and accesses in this page-granular range have been retired.
pub(crate) unsafe fn decommit(address: *mut u8, size: usize) -> bool {
    // SAFETY: Only completely retired pages are marked reusable. Their mapping
    // remains writable, but retired contents are no longer guaranteed.
    unsafe { advise(address, size, libc::MADV_FREE_REUSABLE) }
}

/// # Safety
/// The writable, page-granular range is caller-owned; REUSABLE additionally
/// requires that all objects and accesses in the range have been retired.
unsafe fn advise(address: *mut u8, size: usize, advice: libc::c_int) -> bool {
    advise_with(advice, || {
        // SAFETY: The caller owns the complete range and meets the advice's
        // access and object-lifetime requirements.
        if unsafe { libc::madvise(address.cast(), size, advice) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().raw_os_error())
        }
    })
}

fn advise_with(advice: libc::c_int, mut call: impl FnMut() -> Result<(), Option<i32>>) -> bool {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match call() {
            Ok(()) => return true,
            Err(Some(libc::EAGAIN)) if advice == libc::MADV_FREE_REUSE && attempts < REUSE_MAX_ATTEMPTS => {
                std::thread::yield_now();
            }
            Err(_) => return false,
        }
    }
}

/// # Safety
/// The complete page-granular reservation has no remaining objects or users.
pub(crate) unsafe fn release(address: *mut u8, size: usize) {
    // SAFETY: The caller surrenders this exact, exclusively owned mapping.
    crate::abort::require(unsafe { libc::munmap(address.cast(), size) } == 0);
}

pub(crate) fn wait(word: &AtomicU32, expected: u32) {
    while word.load(Ordering::Acquire) == expected {
        #[cfg(test)]
        // SAFETY: The aligned atomic remains live; the kernel atomically
        // compares its representation and joins the wait queue. The timeout
        // bounds broken wake protocols without changing production waits.
        let result = unsafe {
            os_sync_wait_on_address_with_timeout(
                word.as_ptr().cast(),
                u64::from(expected),
                std::mem::size_of::<u32>(),
                0,
                OS_CLOCK_MACH_ABSOLUTE_TIME,
                WAIT_TIMEOUT_NANOS,
            )
        };
        #[cfg(not(test))]
        // SAFETY: The live, aligned atomic provides the four-byte wait word.
        let result = unsafe { os_sync_wait_on_address(word.as_ptr().cast(), u64::from(expected), std::mem::size_of::<u32>(), 0) };
        check_wait_result(result);
    }
}

fn check_wait_result(result: libc::c_int) {
    if result == -1 {
        check_wait_error(std::io::Error::last_os_error().raw_os_error());
    }
}

/// Wakes by numeric address without accessing a possibly departed stack node.
pub(crate) fn wake_one(address: *const u32) {
    crate::abort::require(!address.is_null() && address.is_aligned());
    // SAFETY: The wake API uses the numeric address as its wait-table key,
    // without dereferencing or forming a Rust reference to the departed word.
    let result = unsafe { os_sync_wake_by_address_any(address.cast_mut().cast(), std::mem::size_of::<u32>(), 0) };
    if result == -1 {
        check_wake_error(std::io::Error::last_os_error().raw_os_error());
    }
}

fn check_wait_error(error: Option<i32>) {
    // The API documents interrupted waits and transient kernel allocation or
    // copy-in failures as retryable, after rechecking the live atomic word.
    crate::abort::require(matches!(error, Some(libc::EINTR | libc::ENOMEM | libc::EFAULT)));
}

fn check_wake_error(error: Option<i32>) {
    // The original waiter may have departed and its address may now belong to
    // a different ulock opcode. ENOENT and EDOM both denote a stale wake key.
    crate::abort::require(matches!(error, Some(libc::ENOENT | libc::EDOM)));
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn unsupported_pages_and_invalid_reservations_fail_before_mapping() {
        for page_size in [-1, 0, libc::c_long::try_from(PAGE * 2).unwrap()] {
            assert!(reserve_with_page_size(RESERVE_MIN, RESERVE_MIN, page_size).is_null());
        }
        for (size, alignment) in [(0, PAGE), (PAGE, PAGE), (RESERVE_MIN, 3)] {
            assert!(reserve(size, alignment).is_null());
        }
    }

    #[test]
    fn mach_reservations_preserve_requested_alignment() {
        for alignment in [1, PAGE, RESERVE_MIN, 1 << 20, 1 << 24] {
            let pointer = reserve(RESERVE_MIN, alignment);
            if pointer.is_null() {
                std::alloc::handle_alloc_error(std::alloc::Layout::from_size_align(RESERVE_MIN, alignment).unwrap());
            }
            assert_eq!(pointer.addr() % alignment, 0);
            // SAFETY: The test owns the entire fresh reservation.
            assert!(unsafe { commit(pointer, RESERVE_MIN) });
            // SAFETY: The complete range is exclusively writable.
            unsafe { pointer.write_bytes(0x5a, RESERVE_MIN) };
            // SAFETY: The first endpoint belongs to the initialized range.
            let first = unsafe { *pointer };
            // SAFETY: The final endpoint lies within the same owned reservation.
            let last = unsafe { pointer.add(RESERVE_MIN - 1) };
            // SAFETY: The final endpoint was initialized by write_bytes above.
            assert_eq!((first, unsafe { *last }), (0x5a, 0x5a));
            // SAFETY: No access remains into the owned reservation.
            unsafe { release(pointer, RESERVE_MIN) };
        }
    }

    #[test]
    fn rejected_commit_preserves_the_owned_reservation() {
        let pointer = reserve(RESERVE_MIN, RESERVE_MIN);
        assert!(!pointer.is_null());
        // SAFETY: libSystem maintains this process's task port.
        let task = unsafe { mach_task_self_ };
        // SAFETY: The test owns the complete, inaccessible reservation. Lowering
        // its maximum protection to NONE deliberately prevents a later commit.
        let result = unsafe { mach_vm_protect(task, pointer.addr() as u64, RESERVE_MIN as u64, 1, libc::PROT_NONE) };
        assert_eq!(result, 0);
        // SAFETY: The page-granular range still belongs to this live reservation.
        assert!(!unsafe { commit(pointer, PAGE) });
        // SAFETY: The rejected commit leaves the complete reservation owned.
        unsafe { release(pointer, RESERVE_MIN) };
    }

    #[test]
    fn rejected_advice_leaves_owned_pages_accessible() {
        let pointer = reserve(RESERVE_MIN, RESERVE_MIN);
        if pointer.is_null() {
            std::alloc::handle_alloc_error(std::alloc::Layout::from_size_align(RESERVE_MIN, RESERVE_MIN).unwrap());
        }
        // SAFETY: The test owns this complete, page-aligned reservation.
        assert!(unsafe { commit(pointer, PAGE) });
        // SAFETY: The committed page is exclusively writable.
        unsafe { pointer.write_bytes(0x5a, PAGE) };
        // SAFETY: The owned page is writable, contains no live objects, and the
        // invalid advice is rejected by the kernel without changing its mapping.
        assert!(!unsafe { advise(pointer, PAGE, -1) });
        // SAFETY: The rejected advice preserves this initialized page.
        assert_eq!(unsafe { *pointer }, 0x5a);
        // SAFETY: No access remains into the complete owned reservation.
        unsafe { release(pointer, RESERVE_MIN) };
    }

    #[test]
    fn timed_out_test_wait_aborts_through_the_kernel_result_dispatch() {
        crate::abort::assert_aborts(
            "hal::macos::tests::timed_out_test_wait_aborts_through_the_kernel_result_dispatch",
            || {
                let word = AtomicU32::new(0);
                // SAFETY: The live, aligned word matches the expected value.
                // A one-nanosecond timeout bounds this intentional failed wait.
                let result = unsafe {
                    os_sync_wait_on_address_with_timeout(
                        word.as_ptr().cast(),
                        0,
                        std::mem::size_of::<u32>(),
                        0,
                        OS_CLOCK_MACH_ABSOLUTE_TIME,
                        1,
                    )
                };
                assert_eq!(result, -1);
                assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ETIMEDOUT));
                check_wait_result(result);
            },
        );
    }

    #[test]
    fn changed_words_do_not_block_and_departed_waiters_are_tolerated() {
        let word = AtomicU32::new(1);
        // SAFETY: The aligned word remains live and differs from the expected
        // value, so the atomic compare-and-wait must return without blocking.
        assert!(unsafe { os_sync_wait_on_address(word.as_ptr().cast(), 0, 4, 0) } >= 0);
        wait(&word, 0);
        wake_one(word.as_ptr());
        check_wake_error(Some(libc::ENOENT));
        check_wake_error(Some(libc::EDOM));
        for error in [libc::EINTR, libc::ENOMEM, libc::EFAULT] {
            check_wait_error(Some(error));
        }
    }

    #[test]
    fn stale_wake_tolerates_an_address_reused_by_an_eight_byte_waiter() {
        use std::sync::atomic::AtomicU64;
        use std::time::{Duration, Instant};

        let word = AtomicU64::new(0);
        let error = std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                while word.load(Ordering::Acquire) == 0 {
                    // SAFETY: The aligned eight-byte atomic remains live until
                    // this thread joins. The timeout bounds a failed protocol.
                    let result = unsafe {
                        os_sync_wait_on_address_with_timeout(
                            word.as_ptr().cast(),
                            0,
                            std::mem::size_of::<u64>(),
                            0,
                            OS_CLOCK_MACH_ABSOLUTE_TIME,
                            WAIT_TIMEOUT_NANOS,
                        )
                    };
                    check_wait_result(result);
                }
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            let error = loop {
                // SAFETY: Probe the numeric wait key with the allocator's
                // four-byte opcode; wake does not access the atomic contents.
                let result = unsafe { os_sync_wake_by_address_any(word.as_ptr().cast(), std::mem::size_of::<u32>(), 0) };
                let error = if result == -1 {
                    std::io::Error::last_os_error().raw_os_error()
                } else {
                    None
                };
                if error != Some(libc::ENOENT) || Instant::now() >= deadline {
                    break error;
                }
                std::thread::yield_now();
            };
            if error == Some(libc::EDOM) {
                wake_one(word.as_ptr().cast());
            }
            word.store(1, Ordering::Release);
            // SAFETY: The live atomic's own eight-byte opcode releases its
            // waiter even if the mismatched probe did not observe EDOM.
            let result = unsafe { os_sync_wake_by_address_any(word.as_ptr().cast(), std::mem::size_of::<u64>(), 0) };
            if result == -1 {
                check_wake_error(std::io::Error::last_os_error().raw_os_error());
            }
            waiter.join().unwrap();
            error
        });
        assert_eq!(error, Some(libc::EDOM));
    }

    #[test]
    fn reclaim_does_not_retry_kernel_pressure() {
        let mut calls = 0;
        assert!(!advise_with(libc::MADV_FREE_REUSABLE, || {
            calls += 1;
            Err(Some(libc::EAGAIN))
        }));
        assert_eq!(calls, 1);
    }

    #[test]
    fn reuse_retries_are_bounded_and_can_recover() {
        let mut calls = 0;
        assert!(advise_with(libc::MADV_FREE_REUSE, || {
            calls += 1;
            if calls < REUSE_MAX_ATTEMPTS {
                Err(Some(libc::EAGAIN))
            } else {
                Ok(())
            }
        }));
        assert_eq!(calls, REUSE_MAX_ATTEMPTS);
        calls = 0;
        assert!(!advise_with(libc::MADV_FREE_REUSE, || {
            calls += 1;
            Err(Some(libc::EAGAIN))
        }));
        assert_eq!(calls, REUSE_MAX_ATTEMPTS);
    }

    #[test]
    fn unexpected_advice_errors_do_not_retry() {
        let mut calls = 0;
        assert!(!advise_with(libc::MADV_FREE_REUSE, || {
            calls += 1;
            Err(Some(libc::EINVAL))
        }));
        assert_eq!(calls, 1);
    }

    #[test]
    fn unexpected_wait_errors_abort_instead_of_retrying_forever() {
        crate::abort::assert_aborts(
            "hal::macos::tests::unexpected_wait_errors_abort_instead_of_retrying_forever",
            || {
                check_wait_error(Some(libc::EINVAL));
            },
        );
    }

    #[test]
    fn unexpected_wake_errors_abort_instead_of_losing_notifications() {
        crate::abort::assert_aborts(
            "hal::macos::tests::unexpected_wake_errors_abort_instead_of_losing_notifications",
            || check_wake_error(Some(libc::EINVAL)),
        );
    }
}
