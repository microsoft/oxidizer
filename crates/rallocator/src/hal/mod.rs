// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Platform memory operations. All unsafe calls require caller-owned ranges;
//! this module never allocates through Rust's global allocator.

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod win64;
#[cfg(target_arch = "x86_64")]
mod x86_64;

#[cfg(test)]
use std::cell::Cell;

#[cfg(target_arch = "aarch64")]
pub(crate) use aarch64::{pause, prefetch};
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "macos")]
pub(crate) use macos::{Mutex, MutexGuard};
pub(crate) use platform::{PAGE, RESERVE_MIN, release, wait, wake_one};
#[cfg(target_os = "windows")]
use win64 as platform;
#[cfg(all(test, target_os = "windows"))]
pub(crate) use win64::disable_test_crash_reporting;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::{pause, prefetch};

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Reserve,
    Commit,
    Decommit,
}

#[cfg(test)]
type CommitProbe = fn(usize, usize) -> bool;

#[cfg(test)]
std::thread_local! {
    static FAILURE: Cell<Option<Failure>> = const { Cell::new(None) };
    static COMMIT_PROBE: Cell<Option<CommitProbe>> = const { Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_commit_probe<R>(probe: CommitProbe, run: impl FnOnce() -> R) -> R {
    struct Restore(Option<CommitProbe>);
    impl Drop for Restore {
        fn drop(&mut self) {
            COMMIT_PROBE.set(self.0);
        }
    }
    let _restore = Restore(COMMIT_PROBE.replace(Some(probe)));
    run()
}

#[cfg(test)]
pub(crate) fn fail_next(failure: Failure) {
    FAILURE.with(|slot| {
        assert_eq!(slot.replace(Some(failure)), None, "only one HAL failure may be armed at a time");
    });
}

#[cfg(test)]
fn should_fail(failure: Failure) -> bool {
    FAILURE.with(|slot| slot.get() == Some(failure) && slot.replace(None).is_some())
}

/// Reserves inaccessible, suitably aligned address space without `GlobalAlloc`.
pub(crate) fn reserve(size: usize, alignment: usize) -> *mut u8 {
    if size == 0 || !size.is_multiple_of(RESERVE_MIN) || !alignment.is_power_of_two() {
        return std::ptr::null_mut();
    }
    #[cfg(test)]
    if should_fail(Failure::Reserve) {
        return std::ptr::null_mut();
    }
    platform::reserve(size, alignment)
}

/// # Safety
/// The page-aligned range lies within a live caller-owned reservation.
/// Committing already accessible pages preserves their contents.
pub(crate) unsafe fn commit(address: *mut u8, size: usize) -> bool {
    commit_with(address, size, |address, size| {
        #[cfg(test)]
        if let Some(probe) = COMMIT_PROBE.get() {
            return probe(address.addr(), size);
        }
        // SAFETY: Forward the caller's exclusive reservation and page-granular bounds.
        unsafe { platform::commit(address, size) }
    })
}

fn commit_with(address: *mut u8, size: usize, commit: impl FnOnce(*mut u8, usize) -> bool) -> bool {
    #[cfg(test)]
    if should_fail(Failure::Commit) {
        return false;
    }
    commit(address, size)
}

/// # Safety
/// The page-aligned, caller-owned range has no live objects or remaining accesses.
/// Success requests backing-page reclamation; Linux and macOS retain writable
/// mappings. macOS reclamation is lazy and does not promise zero on recommit.
pub(crate) unsafe fn decommit(address: *mut u8, size: usize) -> bool {
    decommit_with(address, size, |address, size| {
        // SAFETY: The caller has retired all objects in its reserved range.
        unsafe { platform::decommit(address, size) }
    })
}

fn decommit_with(address: *mut u8, size: usize, decommit: impl FnOnce(*mut u8, usize) -> bool) -> bool {
    #[cfg(test)]
    if should_fail(Failure::Decommit) {
        return false;
    }
    decommit(address, size)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    #[test]
    fn gamma_round2_hal_dispatch_preserves_exact_lengths_and_native_results() {
        let mut byte = 0u8;
        let address = std::ptr::from_mut(&mut byte);
        for size in [0, PAGE, PAGE * 2] {
            for result in [false, true] {
                assert_eq!(
                    commit_with(address, size, |actual, length| {
                        assert_eq!((actual, length), (address, size));
                        result
                    }),
                    result
                );
                assert_eq!(
                    decommit_with(address, size, |actual, length| {
                        assert_eq!((actual, length), (address, size));
                        result
                    }),
                    result
                );
            }
        }
    }

    #[test]
    fn gamma_survivor_hal_alignment_idempotent_commit_and_failure_consumption() {
        for alignment in [1, RESERVE_MIN, RESERVE_MIN * 2] {
            let pointer = reserve(RESERVE_MIN * 2, alignment);
            assert!(!pointer.is_null());
            assert_eq!(pointer.addr() % alignment, 0);
            // SAFETY: The complete reservation is exclusively owned through release.
            assert!(unsafe { commit(pointer, PAGE * 2) });
            // SAFETY: Both committed pages are writable and owned, with no aliases.
            unsafe { pointer.write_bytes(37, PAGE * 2) };
            // SAFETY: Committing the same owned pages again is idempotent.
            assert!(unsafe { commit(pointer, PAGE * 2) });
            // SAFETY: Exactly two initialized committed pages remain live.
            let bytes = unsafe { std::slice::from_raw_parts(pointer, PAGE * 2) };
            assert!(bytes.iter().all(|&byte| byte == 37));
            fail_next(Failure::Commit);
            // SAFETY: Valid committed pages are provided; the in-memory failure seam intercepts the call.
            assert!(!unsafe { commit(pointer, PAGE) });
            fail_next(Failure::Decommit);
            // SAFETY: No object references survive; the in-memory failure seam leaves these pages committed.
            assert!(!unsafe { decommit(pointer, PAGE * 2) });
            // SAFETY: Failed decommit preserved both initialized committed pages.
            assert_eq!(unsafe { pointer.wrapping_add(PAGE).read() }, 37);
            // SAFETY: The test retires both pages before decommit.
            assert!(unsafe { decommit(pointer, PAGE * 2) });
            // SAFETY: The same live reservation remains owned.
            assert!(unsafe { commit(pointer, PAGE * 2) });
            #[cfg(not(target_os = "macos"))]
            {
                // SAFETY: Recommitted pages are readable; Windows and Linux reclaim their anonymous contents.
                let bytes = unsafe { std::slice::from_raw_parts(pointer, PAGE * 2) };
                assert!(bytes.iter().all(|&byte| byte == 0));
            }
            // SAFETY: All references have ended and the test returns its entire reservation once.
            unsafe { release(pointer, RESERVE_MIN * 2) };
        }
        for (size, alignment) in [(0, 1), (1, 1), (RESERVE_MIN, 0), (RESERVE_MIN, 3)] {
            assert!(reserve(size, alignment).is_null());
        }
    }

    #[test]
    fn cpu_hints_accept_unreadable_addresses() {
        for address in [0, 1, usize::MAX] {
            prefetch(address);
        }
        pause();
    }

    #[test]
    fn invalid_reservations_do_not_reach_the_platform() {
        for (size, alignment) in [(0, RESERVE_MIN), (RESERVE_MIN - 1, RESERVE_MIN), (RESERVE_MIN, 0), (RESERVE_MIN, 3)] {
            fail_next(Failure::Reserve);
            assert!(reserve(size, alignment).is_null());
            assert_eq!(FAILURE.with(Cell::get), Some(Failure::Reserve));
            assert!(reserve(RESERVE_MIN, RESERVE_MIN).is_null());
            assert_eq!(FAILURE.with(Cell::get), None);
        }
    }

    #[test]
    fn reserve_commit_decommit_recommit() {
        let size = 2 * RESERVE_MIN;
        let ptr = reserve(size, size);
        assert!(!ptr.is_null());
        assert_eq!(ptr.addr() % size, 0);
        // SAFETY: The test exclusively owns the reservation through its release.
        assert!(unsafe { commit(ptr, PAGE) });
        // SAFETY: Fresh anonymous memory is initialized to zero.
        assert!(unsafe { std::slice::from_raw_parts(ptr, PAGE) }.iter().all(|&b| b == 0));
        // SAFETY: The committed page is exclusively writable.
        unsafe { ptr.write_bytes(0x5a, PAGE) };
        // SAFETY: Idempotent commit preserves this owned page.
        assert!(unsafe { commit(ptr, PAGE) });
        // SAFETY: The first byte remains committed and initialized.
        assert_eq!(unsafe { *ptr }, 0x5a, "idempotent commit must preserve contents");
        // SAFETY: No reference into the page survives.
        assert!(unsafe { decommit(ptr, PAGE) });
        // SAFETY: The reservation still belongs to the test.
        assert!(unsafe { commit(ptr, PAGE) });
        #[cfg(not(target_os = "macos"))]
        // SAFETY: The entire recommitted page is readable.
        assert!(unsafe { std::slice::from_raw_parts(ptr, PAGE) }.iter().all(|&b| b == 0));
        // SAFETY: Recommitted memory is writable even when reclamation is lazy.
        unsafe { ptr.write_bytes(0x3c, PAGE) };
        // SAFETY: The complete page was explicitly initialized above.
        assert!(unsafe { std::slice::from_raw_parts(ptr, PAGE) }.iter().all(|&b| b == 0x3c));
        // SAFETY: The test has finished accessing its complete reservation.
        unsafe { release(ptr, size) };
    }

    #[test]
    fn injected_failures_are_single_use() {
        fail_next(Failure::Reserve);
        assert!(reserve(RESERVE_MIN, RESERVE_MIN).is_null());
        let ptr = reserve(RESERVE_MIN, RESERVE_MIN);
        assert!(!ptr.is_null());
        fail_next(Failure::Commit);
        // SAFETY: The test owns this page-aligned reservation.
        assert!(!unsafe { commit(ptr, PAGE) });
        // SAFETY: The injected failure was consumed and the reservation remains valid.
        assert!(unsafe { commit(ptr, PAGE) });
        fail_next(Failure::Decommit);
        // SAFETY: Failed decommit leaves the owned page committed.
        assert!(!unsafe { decommit(ptr, PAGE) });
        // SAFETY: A subsequent real decommit can retire the untouched page.
        assert!(unsafe { decommit(ptr, PAGE) });
        // SAFETY: No access remains into the reservation.
        unsafe { release(ptr, RESERVE_MIN) };
    }

    #[test]
    fn decommit_does_not_discard_neighboring_live_pages() {
        let ptr = reserve(RESERVE_MIN, RESERVE_MIN);
        assert!(!ptr.is_null());
        // SAFETY: The complete reservation belongs to the test.
        assert!(unsafe { commit(ptr, 3 * PAGE) });
        // SAFETY: All three committed pages are exclusively writable.
        unsafe { ptr.write_bytes(0x5a, 3 * PAGE) };
        // SAFETY: PAGE is within the owned reservation.
        let middle = unsafe { ptr.add(PAGE) };
        // SAFETY: No access to the middle page is retained during decommit.
        assert!(unsafe { decommit(middle, PAGE) });
        // SAFETY: Committing the whole range must preserve the two live neighbors.
        assert!(unsafe { commit(ptr, 3 * PAGE) });
        #[cfg(target_os = "macos")]
        // SAFETY: Darwin may retain retired contents; initialize only the middle
        // page so the two neighboring live pages still test commit preservation.
        unsafe {
            middle.write_bytes(0, PAGE);
        }
        // SAFETY: All three pages are accessible and initialized.
        let contents = unsafe { std::slice::from_raw_parts(ptr, 3 * PAGE) };
        assert!(contents[..PAGE].iter().all(|&b| b == 0x5a));
        assert!(contents[PAGE..2 * PAGE].iter().all(|&b| b == 0));
        assert!(contents[2 * PAGE..].iter().all(|&b| b == 0x5a));
        // SAFETY: No slice is used after releasing the complete reservation.
        unsafe { release(ptr, RESERVE_MIN) };
    }

    #[test]
    fn wait_rechecks_the_word_and_wakes_without_lost_notifications() {
        let word = AtomicU32::new(0);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                word.store(1, Ordering::Release);
                wake_one(std::ptr::from_ref(&word).cast());
            });
            wait(&word, 0);
        });
        assert_eq!(word.load(Ordering::Acquire), 1);
        // A notification that preceded wait must not send this caller to sleep.
        wait(&word, 0);
    }
}
