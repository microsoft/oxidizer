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
    // Page alignment bounds this padding by alignment - page_size, which the
    // over-reserved mapping contains. No rounded absolute address is needed.
    let prefix = mapping.addr().wrapping_neg() & (alignment - 1);
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
    read_kernel_file_with(
        buffer,
        || {
            // SAFETY: The path is NUL-terminated and the openat arguments have
            // kernel ABI types. The syscall does not retain the path pointer.
            unsafe { libc::syscall(libc::SYS_openat, libc::AT_FDCWD, path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC, 0) }
        },
        |fd, buffer| {
            // SAFETY: The writable buffer is valid for its full length.
            unsafe { libc::syscall(libc::SYS_read, fd, buffer.as_mut_ptr(), buffer.len()) }
        },
        |fd| {
            // SAFETY: The reader uniquely owns the descriptor returned by open.
            unsafe { libc::syscall(libc::SYS_close, fd) };
        },
    )
}

#[cfg(target_os = "linux")]
fn read_kernel_file_with(
    buffer: &mut [u8],
    open: impl FnOnce() -> libc::c_long,
    read: impl FnOnce(libc::c_long, &mut [u8]) -> libc::c_long,
    close: impl FnOnce(libc::c_long),
) -> usize {
    let fd = open();
    if fd < 0 {
        return 0;
    }
    let count = read(fd, buffer);
    close(fd);
    usize::try_from(count).unwrap_or(0)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::cell::Cell;

    use super::read_kernel_file_with;

    #[test]
    fn failed_open_preserves_the_buffer_without_reading_or_closing() {
        let mut buffer = *b"????";

        let count = read_kernel_file_with(
            &mut buffer,
            || -1,
            |_, _| panic!("a failed open must not be read"),
            |_| panic!("a failed open owns no descriptor to close"),
        );

        assert_eq!((count, buffer), (0, *b"????"));
    }

    #[test]
    fn failed_read_closes_its_descriptor_and_returns_no_bytes() {
        let mut buffer = *b"????";
        let closed = Cell::new(None);

        let count = read_kernel_file_with(
            &mut buffer,
            || 17,
            |fd, _| {
                assert_eq!(fd, 17);
                -1
            },
            |fd| closed.set(Some(fd)),
        );

        assert_eq!((count, buffer, closed.get()), (0, *b"????", Some(17)));
    }

    #[test]
    fn short_read_keeps_its_exact_byte_count_and_closes_the_descriptor() {
        let mut buffer = *b"????";
        let closed = Cell::new(None);

        let count = read_kernel_file_with(
            &mut buffer,
            || 17,
            |fd, buffer| {
                assert_eq!(fd, 17);
                buffer[..2].copy_from_slice(b"2\n");
                2
            },
            |fd| closed.set(Some(fd)),
        );

        assert_eq!((count, buffer, closed.get()), (2, *b"2\n??", Some(17)));
    }
}
