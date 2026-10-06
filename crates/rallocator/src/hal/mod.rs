// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Platform memory operations. All unsafe calls require caller-owned ranges;
//! this module never allocates through Rust's global allocator.

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_os = "linux")]
mod linux;
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
pub(crate) use platform::{PAGE, RESERVE_MIN, release, wait, wake_one};
#[cfg(target_os = "windows")]
use win64 as platform;
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
std::thread_local! {
    static FAILURE: Cell<Option<Failure>> = const { Cell::new(None) };
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
    #[cfg(test)]
    if should_fail(Failure::Commit) {
        return false;
    }
    // SAFETY: Forward the caller's exclusive reservation and page-granular bounds.
    unsafe { platform::commit(address, size) }
}

/// # Safety
/// The page-aligned, caller-owned range has no live objects or remaining accesses.
/// Success discards its backing pages; Linux retains writable virtual mappings.
pub(crate) unsafe fn decommit(address: *mut u8, size: usize) -> bool {
    #[cfg(test)]
    if should_fail(Failure::Decommit) {
        return false;
    }
    // SAFETY: The caller has retired all objects in its reserved range.
    unsafe { platform::decommit(address, size) }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    #[test]
    fn cpu_hints_accept_unreadable_addresses() {
        for address in [0, 1, usize::MAX] {
            prefetch(address);
        }
        pause();
    }

    #[test]
    fn reserve_commit_decommit_recommit_zero() {
        let size = 2 * RESERVE_MIN;
        let ptr = reserve(size, size);
        assert!(!ptr.is_null());
        assert_eq!(ptr.addr() % size, 0);
        // SAFETY: The test exclusively owns the reservation through its release.
        assert!(unsafe { commit(ptr, PAGE) });
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
        // SAFETY: The entire recommitted page is readable.
        assert!(unsafe { std::slice::from_raw_parts(ptr, PAGE) }.iter().all(|&b| b == 0));
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
