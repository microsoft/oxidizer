// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
// Derived from mimalloc (39448a96), Copyright (c) Microsoft Research, Daan Leijen.
// See ../LICENSE-MIMALLOC.

//! Owning virtual-memory reservations. No operation uses Rust's allocator.

#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
mod aarch64;
#[cfg(any(target_os = "linux", test))]
mod linux_policy;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;
mod virtual_memory;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
mod x86_64;

pub(super) mod bitmap;
use std::sync::OnceLock;

#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
use aarch64 as architecture;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use unix as platform;
pub(super) use virtual_memory::VirtualMemory;
#[cfg(target_os = "windows")]
use windows as platform;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
use x86_64 as architecture;

/// The reference allocator's 64-bit slice size covers supported OS page sizes.
pub(super) const SLICE: usize = 64 * 1024;

pub(super) struct Reservation {
    address: *mut u8,
    size: usize,
    base: *mut u8,
    reserved_size: usize,
}

// SAFETY: Reservations own disjoint OS address ranges, contain no thread-local
// state, and can be released on any thread. Access through the raw pointer
// remains the caller's responsibility. The raw pointer prevents automatic Sync.
unsafe impl Send for Reservation {}

pub(super) fn reserve(size: usize, alignment: usize) -> Option<Reservation> {
    reserve_impl(size, alignment, false)
}

pub(super) fn reserve_committed(size: usize, alignment: usize) -> Option<Reservation> {
    reserve_impl(size, alignment, true)
}

fn reserve_impl(size: usize, alignment: usize, committed: bool) -> Option<Reservation> {
    let (size, alignment) = reservation_layout(size, alignment, policy())?;
    platform::reserve(size, alignment, committed)
}

fn reservation_layout(size: usize, alignment: usize, policy: Policy) -> Option<(usize, usize)> {
    if size == 0 || !alignment.is_power_of_two() {
        return None;
    }
    if !policy.page_size.is_power_of_two() || policy.page_size > SLICE {
        return None;
    }
    let size = size.checked_add(SLICE - 1)? & !(SLICE - 1);
    let size = good_allocation_size(size, policy.page_size)?;
    if isize::try_from(size).is_err() {
        return None;
    }
    // os.c aligns allocations of at least eight large pages to the large-page
    // size, even when explicit large OS pages are disabled.
    let alignment = if policy.large_page_size > 0 && size / 8 >= policy.large_page_size {
        alignment.max(SLICE).max(policy.large_page_size)
    } else {
        alignment.max(SLICE)
    };
    Some((size, alignment))
}

impl Reservation {
    pub(super) const fn as_ptr(&self) -> *mut u8 {
        self.address
    }

    pub(super) const fn len(&self) -> usize {
        self.size
    }

    fn contains(&self, offset: usize, size: usize) -> bool {
        size > 0 && offset.is_multiple_of(SLICE) && size.is_multiple_of(SLICE) && offset <= self.size && size <= self.size - offset
    }

    /// Makes a range accessible without changing its contents. Straddling OS
    /// pages are included, as in the reference allocator's metadata commits.
    ///
    /// # Safety
    /// The caller must coordinate changes to this range with all its users.
    /// The caller also coordinates the OS pages that straddle this range.
    pub(super) unsafe fn commit(&self, offset: usize, size: usize) -> bool {
        let Some((offset, size)) = commit_range(self.size, offset, size, policy().page_size) else {
            return false;
        };
        // SAFETY: Bounds were checked above, and the reservation remains alive.
        let address = unsafe { self.address.add(offset) };
        // SAFETY: The caller coordinates page-state changes within its range.
        unsafe { platform::commit(address, size) }
    }

    /// Restores OS accounting for a committed range before reuse.
    ///
    /// # Safety
    /// The caller owns this range and coordinates its reuse with all users.
    pub(super) unsafe fn reuse(&self, offset: usize, size: usize) {
        if !self.contains(offset, size) {
            return;
        }
        // SAFETY: The range is within this live reservation.
        let address = unsafe { self.address.add(offset) };
        // SAFETY: The caller owns this committed range during the OS advice.
        unsafe { platform::reuse(address, size) };
    }

    /// Discards backing pages while retaining the virtual address reservation.
    ///
    /// # Safety
    /// No objects or accesses can remain in the range, including concurrent
    /// accesses. A true result requires commit before reuse. A false result
    /// leaves the range accessible, but macOS still requires reuse accounting.
    pub(super) unsafe fn purge(&self, offset: usize, size: usize) -> bool {
        if !self.contains(offset, size) {
            return false;
        }
        // SAFETY: Bounds were checked above, and the reservation remains alive.
        let address = unsafe { self.address.add(offset) };
        // SAFETY: The caller retired all accesses to this slice-aligned range.
        unsafe { platform::purge(address, size) }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        // SAFETY: This non-cloneable owner releases exactly its original mapping.
        unsafe { platform::release(self.base, self.reserved_size) };
    }
}

#[derive(Clone, Copy)]
pub(super) struct Policy {
    pub(super) eager_commit: bool,
    pub(super) purge_slices: usize,
    pub(super) page_size: usize,
    pub(super) large_page_size: usize,
    #[cfg(target_os = "linux")]
    pub(super) overcommit: bool,
}

pub(super) fn policy() -> Policy {
    static POLICY: OnceLock<Policy> = OnceLock::new();
    *POLICY.get_or_init(platform::policy)
}

pub(super) fn page_size() -> usize {
    policy().page_size
}

pub(super) fn is_in_threadpool() -> bool {
    platform::is_in_threadpool()
}

fn good_allocation_size(size: usize, page: usize) -> Option<usize> {
    // os.c bounds reservation waste with OS-page, 64-KiB, 256-KiB, 1-MiB,
    // and 4-MiB steps. Metadata prefixes participate in this rounding too.
    let alignment = if size < 512 * 1024 {
        page
    } else if size < 2 * 1024 * 1024 {
        SLICE
    } else if size < 8 * 1024 * 1024 {
        4 * SLICE
    } else if size < 32 * 1024 * 1024 {
        16 * SLICE
    } else {
        64 * SLICE
    };
    Some(size.checked_add(alignment - 1)? & !(alignment - 1))
}

fn commit_range(reserved: usize, offset: usize, size: usize, page: usize) -> Option<(usize, usize)> {
    if size == 0 || offset > reserved || size > reserved - offset {
        return None;
    }
    let start = offset & !(page - 1);
    let end = offset.checked_add(size)?.checked_add(page - 1)? & !(page - 1);
    (end <= reserved).then_some((start, end - start))
}

#[cfg(test)]
mod tests {
    use super::{Policy, SLICE, commit_range, good_allocation_size, page_size, reservation_layout, reserve, reserve_committed};

    fn policy() -> Policy {
        Policy {
            eager_commit: false,
            purge_slices: 1,
            page_size: 4096,
            large_page_size: 2 * 1024 * 1024,
            #[cfg(target_os = "linux")]
            overcommit: false,
        }
    }

    #[test]
    fn reservations_reject_empty_and_non_power_of_two_requests() {
        assert_eq!(
            (reservation_layout(0, SLICE, policy()), reservation_layout(SLICE, 3, policy())),
            (None, None)
        );
    }

    #[test]
    fn reservations_reject_unsupported_os_page_sizes() {
        let mut invalid = policy();
        invalid.page_size = 0;
        let zero = reservation_layout(SLICE, SLICE, invalid);
        invalid.page_size = 2 * SLICE;

        assert_eq!((zero, reservation_layout(SLICE, SLICE, invalid)), (None, None));
    }

    #[test]
    fn reservation_rounding_rejects_overflow_and_unaddressable_sizes() {
        assert_eq!(
            (
                reservation_layout(usize::MAX, SLICE, policy()),
                reservation_layout(isize::MAX as usize + 1, SLICE, policy()),
            ),
            (None, None)
        );
    }

    #[test]
    fn large_reservations_inherit_the_os_large_page_alignment() {
        assert_eq!(
            reservation_layout(16 * 1024 * 1024, SLICE, policy()),
            Some((16 * 1024 * 1024, 2 * 1024 * 1024))
        );
    }

    #[test]
    fn medium_reservations_round_to_one_mebibyte() {
        assert_eq!(good_allocation_size(8 * 1024 * 1024 + SLICE, 4096), Some(9 * 1024 * 1024));
    }

    #[test]
    fn sub_two_mebibyte_reservations_round_to_whole_slices() {
        assert_eq!(good_allocation_size(1024 * 1024 + 1, 4096), Some(1024 * 1024 + SLICE));
    }

    #[test]
    fn initially_committed_reservations_are_accessible_and_zeroed() {
        let memory = reserve_committed(SLICE, SLICE).unwrap();
        // SAFETY: This reservation was committed at creation.
        let first = unsafe { memory.as_ptr().read() };

        assert_eq!(first, 0);
    }

    #[test]
    fn invalid_vm_operations_leave_owned_memory_unchanged() {
        let memory = reserve_committed(SLICE, SLICE).unwrap();
        // SAFETY: The committed reservation is exclusively owned by this test.
        unsafe { memory.as_ptr().write(0xAB) };
        // SAFETY: The reservation validates the invalid request before any OS call.
        let committed = unsafe { memory.commit(SLICE, 1) };
        // SAFETY: No data users exist, and the invalid range is rejected.
        let purged = unsafe { memory.purge(SLICE, SLICE) };
        // SAFETY: The invalid range cannot reach the OS reuse operation.
        unsafe { memory.reuse(SLICE, SLICE) };
        // SAFETY: The valid first byte remains committed and owned.
        let byte = unsafe { memory.as_ptr().read() };

        assert_eq!((committed, purged, byte), (false, false, 0xAB));
    }

    #[test]
    fn metadata_commit_includes_every_straddling_os_page() {
        assert_eq!(commit_range(SLICE, 4095, 2, 4096), Some((0, 8192)));
    }

    #[test]
    fn metadata_commit_does_not_extend_beyond_its_reservation() {
        assert_eq!(commit_range(SLICE, SLICE - 1, 2, 4096), None);
    }

    #[test]
    fn metadata_commit_rejects_an_overflowing_range() {
        assert_eq!(commit_range(SLICE, 1, usize::MAX, 4096), None);
    }

    #[test]
    fn os_reservation_rounding_includes_the_metadata_prefix() {
        assert_eq!(
            (
                good_allocation_size(4 * 1024 * 1024 + SLICE, 4096),
                good_allocation_size(1024 * 1024 * 1024 + SLICE, 4096)
            ),
            (Some(4 * 1024 * 1024 + 4 * SLICE), Some(1024 * 1024 * 1024 + 64 * SLICE))
        );
    }

    #[test]
    fn native_metadata_commit_preserves_already_committed_bytes() {
        let memory = reserve(2 * SLICE, SLICE).unwrap();
        // SAFETY: The fresh mapping is exclusive, including straddling pages.
        assert!(unsafe { memory.commit(1, 1) });
        // SAFETY: The first commit includes this OS page.
        unsafe { memory.as_ptr().write(0xB7) };
        // SAFETY: The mapping remains exclusive. Extending commitment must
        // preserve bytes in the already committed first OS page.
        assert!(unsafe { memory.commit(0, page_size() + 1) });
        // SAFETY: The first byte remains in the live, committed mapping.
        let byte = unsafe { memory.as_ptr().read() };

        assert_eq!(byte, 0xB7);
    }

    #[test]
    fn native_purge_preserves_live_neighboring_slices() {
        let memory = reserve(3 * SLICE, SLICE).unwrap();
        // SAFETY: The new reservation has no users or objects.
        assert!(unsafe { memory.commit(0, memory.len()) });
        // SAFETY: The first and last slices are within the committed mapping.
        unsafe { memory.as_ptr().write(0xA3) };
        // SAFETY: This owned final byte is within the same committed mapping.
        let last = unsafe { memory.as_ptr().add(memory.len() - 1) };
        // SAFETY: No concurrent accesses exist to the owned reservation.
        unsafe { last.write(0xC5) };

        // SAFETY: The middle slice has no objects or accesses. The live
        // neighboring slices are outside the purged range.
        let recommit = unsafe { memory.purge(SLICE, SLICE) };
        #[cfg(target_os = "windows")]
        {
            assert!(recommit);
            // SAFETY: The middle slice remains owned and unaccessed.
            assert!(unsafe { memory.commit(SLICE, SLICE) });
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert!(!recommit);
        // SAFETY: The middle slice is either still committed or was recommitted.
        // Reuse is also valid on Windows, where it is a no-op.
        unsafe { memory.reuse(SLICE, SLICE) };
        // SAFETY: The first byte is outside the purge range and stays committed.
        let first = unsafe { memory.as_ptr().read() };
        // SAFETY: The last byte is also outside the purge range.
        let last = unsafe { last.read() };

        assert_eq!((first, last), (0xA3, 0xC5));
    }
}
