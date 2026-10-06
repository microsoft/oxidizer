// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An owner-return allocator implemented in Rust.
//!
//! Supported: **x86-64 and `AArch64` Linux with 4-KiB kernel pages**, and
//! **x86-64 and ARM64 Windows 10 version 1809 or later, MSVC**.
//! Linux hosts with larger kernel pages reject allocation requests rather than
//! discarding neighboring live objects. Both backends use 4-KiB base pages. Windows uses `VirtualAlloc2`
//! reservations and commit/decommit; Linux uses aligned anonymous `mmap`
//! reservations, `mprotect` for access and `madvise(MADV_DONTNEED)` to discard
//! unused backing pages without revoking access. Neither uses another allocator.
//! OS operations and CPU instructions are isolated under the internal HAL.
//! It is intended for a single statically
//! linked allocator instance; allocations must not cross independently linked
//! instances in DLLs.
//!
//! Small allocations use 16-byte-minimum, two-bit mantissa size classes through
//! 64 KiB. Large allocations use power-of-two ranges. Owner-local allocation
//! lists and slab-return queues are unsynchronized; remote frees are grouped
//! into same-slab rings and sent through batched owner queues. Persistent
//! endpoints are reused after thread exit, so outstanding allocations may
//! outlive their allocating thread.
//! Allocation ownership may be transferred solely for deallocation through a
//! relaxed atomic pointer handoff: prepared allocation lists use release
//! fences, and pointer-consuming allocator operations use acquire fences.
//! These internal fences do not publish application payload writes. A receiving
//! thread that reads or modifies allocation contents still requires the normal
//! application-level synchronization that makes those contents visible.
//!
//! The core is deterministic and non-hardened: no randomized reuse, freelist
//! encryption, optional security mitigations, or built-in allocator counters.
//! The separate Seismograph bridge provides opt-in allocation lifecycle events.
//! Virtual address reservations and the sparse pagemap are process-lifetime;
//! unused committed object memory is returned through the backend caches.
//!
//! The allocation policy follows snmalloc's non-hardened `StandardLocalState`
//! core at revision `511e91a`. This is not a binary-compatible translation:
//! intrusive structures use Rust-specific representations and TLS teardown
//! uses reusable endpoint leases. Normal flushing and owner release retain the
//! incoming queue's final user message, as native snmalloc does: a later real
//! message makes that tail reclaimable. Its slab or large range remains live.
//! Teardown always returns the endpoint to its pool, but flushes only for
//! teardown counts below 128 or at powers of two, including the first normal
//! teardown. Other late-TLS returns retain local and outgoing caches until a
//! later flush. Allocator-internal explicit flushing is not throttled.
//!
//! Owner storage preserves the measured snmalloc `511e91a` MSVC 14.51 x64,
//! C++20, non-hardened envelope: a 9,216-byte logical prefix in 16-KiB backing.
//! This is a configuration-specific policy constant, not a universal native
//! type size. The smaller Rust owner and its realloc lookup sidecar occupy the
//! logical prefix; the remaining gap is permanently withheld. The 7,168-byte
//! suffix supplies 112 initial 64-byte metadata cells. Backing size and remote
//! routing derive from the logical prefix, not Rust's type size. Pool reuse
//! neither reinitializes nor redonates this storage, and the containing backing
//! is never returned.
//!
//! Local backend ranges grow geometrically up to 2 MiB; global refill growth
//! is capped at 16 MiB, unless a request itself is larger. Global cached ranges
//! attempt to discard backing pages; failed OS discard leaves them committed.
//! Local cached ranges remain committed. The 16-KiB-granularity pagemap reserves 256 GiB of
//! virtual address space but commits only the pages it uses. Individual range
//! requests above 64 TiB fail. No fixed-heap or dynamic memory-pressure policy
//! is provided. Allocation failure returns null; failure to obtain metadata
//! needed to complete a deallocation terminates the process.
//!
//! ```
//! rallocator::rallocator!();
//! let value = Box::new([42u8; 1024]);
//! assert_eq!(value[0], 42);
//! ```
//!
//! # Allocation recording
//!
//! ```no_run
//! use seismograph::recorder::{Configuration, RecordingPolicy};
//! rallocator::rallocator!();
//! seismograph::recorder(Configuration {
//!     allocations: RecordingPolicy::all(true),
//!     ..Default::default()
//! });
//! let value = Box::new([42_u8; 1024]);
//! drop(value);
//! seismograph::recorder(Configuration::default());
//! let snapshot = seismograph::snapshot(Default::default()).unwrap();
//! # let _ = snapshot;
//! ```
//!
//! The existing Seismograph recorder owns event policies, bounded buffers,
//! timestamps, stack capture, sampling and suppression. This allocator emits
//! the unchanged allocation/deallocation payload inside the lazy `record()`
//! closure, using only the address and the caller's layout. There is no lifetime
//! registry, allocator-specific thread identity or per-allocation lifetime
//! bookkeeping while recording is stopped. The address is the sampling and correlation key, not a globally
//! unique allocation identity. The container supplies the actual recorder
//! thread; allocator-specific thread and heap IDs are zero (unavailable).
//! The general heap never sets the event contract's bump-only
//! `freed_after_heap_release` flag.
//!
//! Every free while recording is enabled emits an event, including allocations
//! made while recording was stopped or suppressed. Address reuse, recording
//! gaps, concurrent operation completion, sampling, recorder TLS teardown and
//! bounded-buffer overwrites prevent definitive lifetime reconstruction. The
//! plugin pairs retained events in order and assigns view-local identities;
//! unmatched records are not a live-memory census. Sampling uses the address,
//! so repeated reuse of the same address receives the same sampling decision.
//! Recorder-internal allocations and snapshot source storage remain suppressed.
//!
//! Recording does not change native realloc behavior: same-class resizing keeps
//! the pointer, and different-class resizing uses the original native path.
//! A successful size change emits a free with the old layout and an allocation
//! with the new layout, including in-place resizing. An unchanged size or failed
//! replacement emits neither; failure leaves the original pointer and bytes intact.
//!
//! # Native state observations
//!
//! The allocator source uses `seismograph_rallocator` schema 3. It inventories
//! persistent owner endpoints, including owners created before recording.
//! Inventory linkage and lease generations change only on owner lifecycle
//! paths; ordinary recording-off allocation/free paths do no observation work.
//! No public allocator inspection methods or internal structures are exposed.
//!
//! With allocation recording enabled, owners can publish bounded native state
//! after accepted events, outside the native core's mutable borrow. Enable or
//! disable this through
//! [`seismograph_rallocator::native::set_publication_enabled`]. Snapshot polling
//! requests subsequent observation rounds; an application can also call
//! [`seismograph_rallocator::native::request_observation`]. No background sampler
//! or per-operation clock read is required.
//!
//! Published state includes small classes, native outstanding large ranges,
//! local range/metadata caches and remote-return state. The collector inspects
//! returned owners under the pool lock, but never reads another thread's leased
//! core. Quiet leased owners retain older publications or explicitly unknown
//! state; unavailable denotes telemetry storage failure. Every observation
//! identifies its recording session, round, lease generation and capture time.
//! Walks and inventory collection are bounded and explicitly report truncation.
//! Failed telemetry storage is unavailable, not fabricated zero state.
//!
//! Native outstanding ranges are not application-live allocations; incoming
//! queue addresses do not provide a queue-depth census. Physical residency is
//! unknown, including when OS discard fails. Global backend state is collected
//! separately under its existing lock. v1 snapshot compatibility is not provided.
//!
//! # Migration from v1
//!
//! `rallocator!()` remains the installation entrypoint. [`Rallocator`] is
//! a stateless unit struct without generic configuration. `GlobalRallocator`,
//! `config::{Config, Tunables, SizeClassLayout, Standard, StandardSizeClasses}`,
//! configured macro arguments, `caller-symbolization`, `loom` and
//! `tuning-telemetry` are removed. v1 medium-cache/purge, slab-scan and bitmap
//! tunables have no v4 counterparts. `allocation_hints` remains an independent
//! crate, but v4 ignores prospective general/bump/thread-heap hints. There is
//! no second active v1 allocator or compatibility backend. v1-specific tests,
//! examples and benchmark targets are retired; benchmark source files are
//! untouched and are not build targets. Miri is not a v4 backend.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(not(all(
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "linux", all(target_os = "windows", target_env = "msvc"))
)))]
compile_error!("rallocator supports x86_64 and aarch64 Linux and Windows MSVC");

mod backend;
mod buddy;
mod classes;
mod combining;
mod core;
mod hal;
mod observation;
mod pagemap;
mod recording;
mod remote;
mod thread;

use std::alloc::{GlobalAlloc, Layout};

/// A stateless handle to the process's allocator.
#[derive(Debug)]
pub struct Rallocator;

// SAFETY: The core returns disjoint, suitably aligned live allocations and
// routes their destruction to their unique owner; failure is reported as null.
unsafe impl GlobalAlloc for Rallocator {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        recording::allocate(layout, false)
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: GlobalAlloc requires a live allocation from this instance.
        unsafe { recording::deallocate(ptr, layout) };
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        recording::allocate(layout, true)
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: GlobalAlloc guarantees the live allocation and valid new size.
        unsafe { recording::reallocate(ptr, layout, new_size) }
    }
}

#[inline]
unsafe fn reallocate_core(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    let Some(request) = classes::Request::with_size(layout, new_size) else {
        return std::ptr::null_mut();
    };
    // override/rust.cc retains a pointer exactly when the aligned old and
    // new layouts select the same size class, not for arbitrary shrinking.
    if classes::Request::new(layout) == Some(request) {
        return ptr;
    }
    // SAFETY: The caller guarantees ptr is live for layout; the copy bound
    // fits both the old allocation and requested replacement.
    unsafe { thread::reallocate(ptr, request, layout.size().min(new_size)) }
}

/// Defines this crate's allocator as the executable's global allocator.
#[macro_export]
macro_rules! rallocator {
    () => {
        #[global_allocator]
        static GLOBAL: $crate::Rallocator = $crate::Rallocator;
    };
}
