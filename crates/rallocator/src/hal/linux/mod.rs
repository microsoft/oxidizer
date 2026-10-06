// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Linux virtual memory and allocation-free process-private `futex` waits.

use std::sync::atomic::{AtomicU32, Ordering};

pub(crate) const PAGE: usize = 4096;
// Preserve the v4 backend's reservation/refill policy across operating systems.
pub(crate) const RESERVE_MIN: usize = 65536;

pub(crate) fn reserve(size: usize, alignment: usize) -> *mut u8 {
    // SAFETY: sysconf queries a process-wide constant without pointer arguments.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    reserve_with_page_size(size, alignment, page_size)
}

fn reserve_with_page_size(size: usize, alignment: usize, page_size: libc::c_long) -> *mut u8 {
    // Linux AArch64 also permits 16/64-KiB kernel pages. Rounding a discard to
    // those sizes could destroy neighboring live objects in our 16-KiB chunks.
    // Reject such hosts before creating any allocator-owned reservation.
    if usize::try_from(page_size) != Ok(PAGE) {
        return std::ptr::null_mut();
    }

    let alignment = alignment.max(PAGE);
    let Some(mapped_size) = size.checked_add(alignment - PAGE) else {
        return std::ptr::null_mut();
    };
    // SAFETY: Anonymous PROT_NONE mapping reserves fresh address space without
    // a file, Rust allocation, or backing-page commitment. MAP_NORESERVE keeps
    // the process-lifetime sparse pagemap out of the kernel's commit accounting.
    let mapping = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            mapped_size,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    if mapping == libc::MAP_FAILED {
        return std::ptr::null_mut();
    }
    let base = mapping.addr();
    let prefix = base.checked_add(alignment - 1).map(|rounded| (rounded & !(alignment - 1)) - base);
    // SAFETY: This call owns the fresh mapping; a computed prefix is page-granular
    // and leaves the requested size within the mapping.
    unsafe { retain_mapping(mapping.cast(), mapped_size, size, prefix) }
}

/// # Safety
/// The complete page-granular mapping is exclusively owned. A supplied prefix
/// is page-granular and leaves `size` bytes inside that mapping.
unsafe fn retain_mapping(mapping: *mut u8, mapped_size: usize, size: usize, prefix: Option<usize>) -> *mut u8 {
    let Some(prefix) = prefix else {
        // SAFETY: The entire fresh mapping still belongs exclusively to this call.
        unsafe { release(mapping, mapped_size) };
        return std::ptr::null_mut();
    };
    let suffix = mapped_size - prefix - size;
    // SAFETY: prefix is within the fresh mapping; this aligned subrange is
    // retained while its disjoint, page-granular prefix and suffix are unmapped.
    let aligned = unsafe { mapping.add(prefix) };
    if prefix != 0 {
        // SAFETY: The unused prefix contains no objects or accesses.
        unsafe { release(mapping, prefix) };
    }
    if suffix != 0 {
        // SAFETY: size ends within the original mapping, before its unused suffix.
        let end = unsafe { aligned.add(size) };
        // SAFETY: The unused suffix is page-granular and exclusively owned.
        unsafe { release(end, suffix) };
    }
    aligned
}

/// # Safety
/// The page-granular range belongs to a live caller-owned reservation.
pub(crate) unsafe fn commit(address: *mut u8, size: usize) -> bool {
    // SAFETY: mprotect makes the owned anonymous pages accessible without
    // replacing already committed contents or changing reservation ownership.
    unsafe { libc::mprotect(address.cast(), size, libc::PROT_READ | libc::PROT_WRITE) == 0 }
}

/// # Safety
/// The caller has retired all objects and accesses in this page-granular range.
pub(crate) unsafe fn decommit(address: *mut u8, size: usize) -> bool {
    // SAFETY: DONTNEED discards private anonymous backing pages, which read as
    // zero on their next use. Keeping the mapping writable avoids an additional
    // mprotect and partial-protection rollback on failure; ownership is unchanged.
    unsafe { libc::madvise(address.cast(), size, libc::MADV_DONTNEED) == 0 }
}

/// # Safety
/// The complete page-granular mapping is exclusively owned, with no remaining users.
pub(crate) unsafe fn release(address: *mut u8, size: usize) {
    // SAFETY: The caller supplies the exact unused mapping extent.
    crate::abort::require(unsafe { libc::munmap(address.cast(), size) } == 0);
}

pub(crate) fn wait(word: &AtomicU32, expected: u32) {
    while word.load(Ordering::Acquire) == expected {
        // SAFETY: AtomicU32 provides the aligned four-byte futex word, which
        // stays live until this wait returns. Null timeout means no deadline.
        let result = unsafe {
            libc::syscall(
                libc::SYS_futex,
                word.as_ptr(),
                libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG,
                expected,
                std::ptr::null::<libc::timespec>(),
            )
        };
        check_wait_result(result);
    }
}

/// Wakes one waiter without reading the possibly departed stack node in Rust.
pub(crate) fn wake_one(address: *const u32) {
    // SAFETY: FUTEX_WAKE uses the aligned numeric address as a private wait key,
    // not a Rust reference. A waiter may depart after its release notification.
    let result = unsafe { libc::syscall(libc::SYS_futex, address, libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG, 1_i32) };
    crate::abort::require(result != -1 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EFAULT));
}

fn check_wait_result(result: libc::c_long) {
    if result == -1 {
        // A changed word or signal is not a failure: recheck the predicate.
        check_wait_error(std::io::Error::last_os_error().raw_os_error());
    }
}

fn check_wait_error(error: Option<i32>) {
    crate::abort::require(matches!(error, Some(libc::EAGAIN | libc::EINTR)));
}

#[cfg(test)]
mod tests {
    use super::{PAGE, RESERVE_MIN, reserve_with_page_size};
    use crate::hal;

    #[test]
    fn changed_futex_word_is_a_retryable_kernel_error() {
        let word = std::sync::atomic::AtomicU32::new(1);
        // SAFETY: The aligned atomic stays live through this call. Its value
        // differs from the expected value, so the kernel must not block.
        let result = unsafe {
            libc::syscall(
                libc::SYS_futex,
                word.as_ptr(),
                libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG,
                0_u32,
                std::ptr::null::<libc::timespec>(),
            )
        };
        assert_eq!(result, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EAGAIN));
        super::check_wait_result(result);
        super::check_wait_result(0);
        super::wait(&word, 0);
    }

    #[test]
    fn rejected_alignment_releases_the_entire_owned_mapping() {
        // SAFETY: Anonymous mapping with no file or backing-page commitment.
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                PAGE,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(mapping, libc::MAP_FAILED);
        let mut residency = 0_u8;
        // SAFETY: mincore only queries the mapping and writes one residency byte.
        assert_eq!(unsafe { libc::mincore(mapping, PAGE, &raw mut residency) }, 0);
        // SAFETY: This test owns the complete mapping; None rejects retention.
        assert!(unsafe { super::retain_mapping(mapping.cast(), PAGE, PAGE, None) }.is_null());
        // SAFETY: mincore accepts numeric virtual addresses, including unmapped ones,
        // and writes only into the live residency byte if the query succeeds.
        assert_eq!(unsafe { libc::mincore(mapping, PAGE, &raw mut residency) }, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ENOMEM));
    }

    #[test]
    fn unexpected_wait_error_aborts_instead_of_retrying_forever() {
        crate::abort::assert_aborts(
            "hal::linux::tests::unexpected_wait_error_aborts_instead_of_retrying_forever",
            || {
                // SAFETY: The kernel rejects the null futex address; no Rust
                // reference or memory access is formed from that address.
                let result = unsafe {
                    libc::syscall(
                        libc::SYS_futex,
                        std::ptr::null::<u32>(),
                        libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG,
                        0_u32,
                        std::ptr::null::<libc::timespec>(),
                    )
                };
                assert_eq!(result, -1);
                assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EFAULT));
                super::check_wait_result(result);
            },
        );
        super::check_wait_error(Some(libc::EAGAIN));
        super::check_wait_error(Some(libc::EINTR));
    }

    #[test]
    fn unsupported_kernel_pages_fail_before_reserving_memory() {
        for page_size in [-1, 0, 16_384, 65_536] {
            assert!(reserve_with_page_size(RESERVE_MIN, RESERVE_MIN, page_size).is_null());
        }
        let ptr = reserve_with_page_size(RESERVE_MIN, RESERVE_MIN, 4096);
        assert!(!ptr.is_null());
        // SAFETY: The test owns the entire unused reservation.
        unsafe { hal::release(ptr, RESERVE_MIN) };
        assert!(reserve_with_page_size(usize::MAX & !(PAGE - 1), RESERVE_MIN, 4096).is_null());
    }

    #[test]
    fn aligned_reservations_cover_small_and_large_alignment_requests() {
        for alignment in [1, PAGE, RESERVE_MIN, 1 << 20, 1 << 24] {
            let ptr = hal::reserve(RESERVE_MIN, alignment);
            assert!(!ptr.is_null());
            assert_eq!(ptr.addr() % alignment, 0);
            // SAFETY: This test owns the complete reservation.
            assert!(unsafe { hal::commit(ptr, RESERVE_MIN) });
            // SAFETY: Every byte is committed and exclusively writable.
            unsafe { ptr.write_bytes(0x5a, RESERVE_MIN) };
            // SAFETY: The first endpoint is within the initialized reservation.
            let first = unsafe { *ptr };
            // SAFETY: The final endpoint is within the same reservation.
            let last = unsafe { ptr.add(RESERVE_MIN - 1) };
            // SAFETY: The final endpoint was initialized by write_bytes above.
            assert_eq!((first, unsafe { *last }), (0x5a, 0x5a));
            // SAFETY: No object or reference survives the complete mapping release.
            unsafe { hal::release(ptr, RESERVE_MIN) };
        }
    }

    #[test]
    fn invalid_and_overflowing_reservations_fail_without_mapping() {
        for (size, alignment) in [
            (0, PAGE),
            (PAGE, PAGE),
            (RESERVE_MIN, 3),
            (usize::MAX & !(RESERVE_MIN - 1), RESERVE_MIN),
        ] {
            assert!(hal::reserve(size, alignment).is_null());
        }
    }
}
