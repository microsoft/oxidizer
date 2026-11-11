// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
// Derived from mimalloc (39448a96), Copyright (c) Microsoft Research, Daan Leijen.
// See ../LICENSE-MIMALLOC.

use super::{Policy, Reservation};

pub(super) fn reserve(size: usize, alignment: usize, committed: bool) -> Option<Reservation> {
    let policy = super::policy();
    let mapped_size = size
        .checked_add(alignment - policy.page_size)
        .filter(|&n| isize::try_from(n).is_ok())?;
    #[cfg(target_os = "linux")]
    let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | if policy.overcommit { libc::MAP_NORESERVE } else { 0 };
    #[cfg(target_os = "macos")]
    let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS;
    let protection = if committed {
        libc::PROT_READ | libc::PROT_WRITE
    } else {
        libc::PROT_NONE
    };
    // macOS uses the reference allocator's default anonymous-memory tag.
    #[cfg(target_os = "macos")]
    let descriptor = 100 << 24;
    #[cfg(target_os = "linux")]
    let descriptor = -1;
    // SAFETY: A fresh anonymous mapping has no objects or existing owners.
    let mapping = unsafe { libc::mmap(std::ptr::null_mut(), mapped_size, protection, flags, descriptor, 0) };
    if mapping == libc::MAP_FAILED {
        return None;
    }
    let Some(aligned) = mapping.addr().checked_add(alignment - 1).map(|p| p & !(alignment - 1)) else {
        // SAFETY: This function still owns the entire fresh mapping.
        unsafe { release(mapping.cast(), mapped_size) };
        return None;
    };
    let prefix = aligned - mapping.addr();
    let suffix = mapped_size - prefix - size;
    // SAFETY: The aligned address is within the mapping and retains provenance.
    let address = unsafe { mapping.cast::<u8>().add(prefix) };
    if prefix != 0 {
        // SAFETY: The page-aligned prefix is disjoint from the retained range.
        unsafe { release(mapping.cast(), prefix) };
    }
    if suffix != 0 {
        // SAFETY: The retained range ends before this page-aligned suffix.
        let end = unsafe { address.add(size) };
        // SAFETY: No objects or accesses exist in the unused suffix.
        unsafe { release(end, suffix) };
    }
    #[cfg(target_os = "linux")]
    if committed && policy.purge_slices > 1 && size.is_multiple_of(policy.large_page_size) && alignment >= policy.large_page_size {
        // SAFETY: This optional advice covers exactly the owned mapping. Its
        // rejection affects only huge-page promotion, not mapping validity.
        unsafe { libc::madvise(address.cast(), size, libc::MADV_HUGEPAGE) };
    }
    Some(Reservation {
        address,
        size,
        base: address,
        reserved_size: size,
    })
}

pub(super) unsafe fn commit(address: *mut u8, size: usize) -> bool {
    // SAFETY: The HAL supplies an owned, page-aligned reservation range.
    unsafe { libc::mprotect(address.cast(), size, libc::PROT_READ | libc::PROT_WRITE) == 0 }
}

#[cfg(target_os = "macos")]
pub(super) unsafe fn reuse(address: *mut u8, size: usize) {
    // SAFETY: The caller owns this committed range. FREE_REUSE restores RSS
    // accounting only. Rejected advice does not invalidate accessible pages.
    unsafe { libc::madvise(address.cast(), size, libc::MADV_FREE_REUSE) };
}

#[cfg(target_os = "linux")]
pub(super) unsafe fn reuse(_address: *mut u8, _size: usize) {}

pub(super) unsafe fn purge(address: *mut u8, size: usize) -> bool {
    #[cfg(target_os = "macos")]
    // SAFETY: The caller retired all accesses. Rejected reusable-page advice
    // leaves this mapping valid, so the same owned range can use DONTNEED.
    if unsafe { libc::madvise(address.cast(), size, libc::MADV_FREE_REUSABLE) } == 0 {
        return false;
    }
    // SAFETY: The caller retired this page-aligned range. Purging is optional
    // backing reclamation. Failure leaves the existing readable/writable
    // mapping intact, with its dirty bits still set.
    unsafe { libc::madvise(address.cast(), size, libc::MADV_DONTNEED) };
    // prim/unix/prim.c deliberately retains protection in non-hardened release
    // builds. Both successful and failed advice need no mprotect on reuse.
    false
}

pub(super) unsafe fn release(address: *mut u8, size: usize) {
    // SAFETY: The HAL supplies the exact, unused, page-granular mapping extent.
    let released = unsafe { libc::munmap(address.cast(), size) };
    // A whole owned extent has valid alignment and needs no VMA split.
    // Failure means an invalid extent or a violated OS ownership invariant.
    crate::backend::abort::require(released == 0);
}

pub(super) const fn is_in_threadpool() -> bool {
    false
}

pub(super) fn policy() -> Policy {
    // SAFETY: sysconf reads the OS page-size constant and takes no pointers.
    let page_size = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(4096);
    #[cfg(target_os = "linux")]
    {
        let mut overcommit = [0; 32];
        let count = read_kernel_file(c"/proc/sys/vm/overcommit_memory", &mut overcommit);
        let mut thp = [0; 64];
        let thp_count = read_kernel_file(c"/sys/kernel/mm/transparent_hugepage/enabled", &mut thp);
        let thp = super::linux_policy::has_transparent_huge_pages(&thp[..thp_count]);
        let overcommit = super::linux_policy::has_overcommit(&overcommit[..count]);
        // Upstream eager_commit=2 detects overcommit; allow_thp=2 retains 2-MiB
        // groups on THP-enabled hosts to avoid splitting their huge pages.
        Policy {
            eager_commit: overcommit,
            purge_slices: if thp { 32 } else { 1 },
            page_size,
            large_page_size: 2 * 1024 * 1024,
            overcommit,
        }
    }
    #[cfg(target_os = "macos")]
    Policy {
        eager_commit: true,
        purge_slices: 1,
        page_size,
        large_page_size: 2 * 1024 * 1024,
    }
}

#[cfg(target_os = "linux")]
fn read_kernel_file(path: &std::ffi::CStr, buffer: &mut [u8]) -> usize {
    // Use the source allocator's direct syscall path. Interposed libc file
    // functions can allocate while the global allocator is still initializing.
    // SAFETY: The path is NUL-terminated and the openat arguments have the
    // kernel ABI types. The syscall does not retain the path pointer.
    let fd = unsafe { libc::syscall(libc::SYS_openat, libc::AT_FDCWD, path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC, 0) };
    if fd < 0 {
        return 0;
    }
    // SAFETY: The writable buffer is valid for its full length.
    let count = unsafe { libc::syscall(libc::SYS_read, fd, buffer.as_mut_ptr(), buffer.len()) };
    // SAFETY: This call uniquely owns the descriptor returned by open.
    unsafe { libc::syscall(libc::SYS_close, fd) };
    usize::try_from(count).unwrap_or(0)
}
