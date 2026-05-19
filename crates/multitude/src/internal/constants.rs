// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared constants for the chunk and smart-pointer machinery.
//!
//! The `64 KiB` alignment is what makes header recovery by masking work.

/// Alignment (and minimum size) of every chunk allocation in bytes.
///
/// Smart pointers recover their chunk header by masking the value
/// pointer with `!(CHUNK_ALIGN - 1)`, which assumes every allocation
/// produced by the arena lives within the 64 KiB tile that starts at
/// the chunk's header.
pub const CHUNK_ALIGN: usize = 65_536;

/// Maximum supported alignment for any value allocated through a
/// smart pointer. Values aligned at `CHUNK_ALIGN` or higher would let
/// the masking trick collide with a neighboring chunk's header.
pub const MAX_SMART_PTR_ALIGN: usize = CHUNK_ALIGN / 2;

/// Default minimum capacity for a fresh chunk's payload.
pub const DEFAULT_MIN_PAYLOAD: usize = 512;

/// Smallest cacheable chunk **total allocation** size, in bytes
/// (header + payload). The smallest class's *payload* is
/// `MIN_CHUNK_BYTES - header_size::<A>()`, which depends on `A`.
pub const MIN_CHUNK_BYTES: usize = 512;

/// Largest cacheable chunk **total allocation** size, in bytes
/// (header + payload). Anything strictly larger is "oversized" and
/// bypasses the cache entirely; oversized chunks are sized to fit
/// exactly the requested payload (plus header, plus drop-list
/// alignment rounding). The largest cached class's payload is
/// `MAX_CHUNK_BYTES - header_size::<A>()`, which depends on `A`.
pub const MAX_CHUNK_BYTES: usize = 65_536;

/// Number of cacheable size classes (powers of two from
/// [`MIN_CHUNK_BYTES`] up to [`MAX_CHUNK_BYTES`] inclusive).
pub const NUM_CHUNK_CLASSES: u8 = 8;

/// Largest "normal" allocation request. Requests strictly larger than
/// this go to the oversized one-shot chunk path and are never cached.
pub const MAX_NORMAL_ALLOC: usize = 16 * 1024;

/// Minimum permitted value of the `max_normal_alloc` builder knob.
pub const MIN_MAX_NORMAL_ALLOC: usize = 4 * 1024;

/// Inflated initial refcount for `SharedChunk`s while they are
/// "current" on an arena.
///
/// While a chunk is the arena's current shared chunk, every
/// `alloc_arc` call only bumps a non-atomic counter on the arena
/// (`arcs_issued`); the chunk's atomic refcount is held at this
/// large value so cross-thread `Arc::drop`s can never drive it to
/// zero in the meantime. On chunk swap-out the arena reconciles in
/// one atomic op.
pub const LARGE: usize = (isize::MAX as usize) / 2;

/// Returns `true` if a chunk allocation starting at `start_addr` and
/// spanning `total` bytes lies entirely within the user-space portion
/// of the address space (i.e. its end address fits in `isize`).
///
/// This is the runtime precondition for the `assert_unchecked` hints
/// the hot-path bump-cursor arithmetic uses to prove that
/// `data_addr + bumped + entry_size` fits in `isize`.
///
/// # Mutation testing
///
/// The five comparison-operator mutations on this expression fall into
/// two groups:
///   * `>` → `==` and `||` → `&&` are killable: a pathological
///     allocator returning a chunk in the upper half of the address
///     space exposes the difference. The test
///     `local_chunk_allocate_rejects_high_address_from_pathological_allocator`
///     covers them.
///   * `>` → `>=`, `<` → `==`, `<` → `<=` are equivalent: the
///     distinguishing inputs (`end_addr == isize::MAX` exactly, or
///     `end_addr == start_addr` which requires `total == 0`) cannot
///     be produced by any real allocator, and the chunk-allocation
///     callers reject `total_bytes < header_bytes` (a strictly
///     positive lower bound) before reaching this helper, so `total
///     == 0` is unreachable on the production path. Skipping mutation
///     testing for this helper is the cleanest way to record that
///     observation; both arms of the check are still exercised by the
///     regression test above and by every successful chunk allocation
///     in the test suite.
#[inline]
#[must_use]
#[cfg_attr(test, mutants::skip)]
pub(crate) fn chunk_end_addr_fits_in_isize(start_addr: usize, total: usize) -> bool {
    let end_addr = start_addr.wrapping_add(total);
    isize::try_from(end_addr).is_ok() && end_addr >= start_addr
}

/// (header + payload).
///
/// The class's usable *payload* is `class_to_bytes(class) -
/// header_size::<A>()`, which depends on the chunk type and `A`.
///
/// `class` must be `< NUM_CHUNK_CLASSES`.
#[inline]
#[must_use]
pub const fn class_to_bytes(class: u8) -> usize {
    debug_assert!(class < NUM_CHUNK_CLASSES, "class out of range");
    MIN_CHUNK_BYTES << class
}

/// Smallest size class whose **total allocation** is at least `bytes`.
///
/// Saturates at `NUM_CHUNK_CLASSES - 1` for `bytes > MAX_CHUNK_BYTES`;
/// callers that care about oversized requests must test against
/// [`MAX_NORMAL_ALLOC`] separately.
///
/// Callers that have a *payload* requirement (rather than a total)
/// must add `header_size::<A>()` before calling: the routing
/// decision must include the header so the picked class's payload
/// actually fits the request.
#[inline]
#[must_use]
#[cfg_attr(test, mutants::skip)] // Boundary arithmetic mutations can still satisfy the contract.
pub const fn min_class_for_bytes(bytes: usize) -> u8 {
    if bytes <= MIN_CHUNK_BYTES {
        return 0;
    }
    if bytes >= MAX_CHUNK_BYTES {
        return NUM_CHUNK_CLASSES - 1;
    }
    // Smallest `c` with `(MIN_CHUNK_BYTES << c) >= bytes`.
    let ratio = bytes.div_ceil(MIN_CHUNK_BYTES);
    // Equivalently: smallest `c` with `(1 << c) >= ratio`.
    let mut c: u8 = 0;
    let mut v: usize = 1;
    while v < ratio {
        v <<= 1;
        c += 1;
    }
    c
}

/// Aborts the process on chunk-refcount overflow.
///
/// Mirrors the behavior of `std::sync::Arc::clone`: a refcount that
/// wraps to zero would let live pointers race with a free, so the
/// only sound response is to terminate the process. Marked
/// `#[cold]` and `#[inline(never)]` so the call site stays small on
/// the hot path.
#[cold]
#[inline(never)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)] // Abort paths terminate the test process before coverage can observe them.
pub fn refcount_overflow_abort() -> ! {
    // Prefer `process::abort`: unwinding through user destructors is no longer sound.
    #[cfg(all(not(loom), feature = "std"))]
    {
        std::process::abort();
    }
    #[cfg(all(not(loom), not(feature = "std")))]
    {
        // On stable no_std, double-panic is the closest abort equivalent.
        struct ForceAbort;
        impl Drop for ForceAbort {
            fn drop(&mut self) {
                #[expect(clippy::panic, reason = "fatal-error path; no recovery path through user destructors is safe")]
                {
                    panic!("multitude: chunk refcount overflow (abort)");
                }
            }
        }
        let _force = ForceAbort;
        #[expect(clippy::panic, reason = "fatal-error path; no recovery path through user destructors is safe")]
        {
            panic!("multitude: chunk refcount overflow");
        }
    }
    #[cfg(loom)]
    {
        // loom does not model `process::abort`; panic marks this path unreachable.
        #[expect(clippy::panic, reason = "loom-only test path")]
        {
            panic!("multitude: chunk refcount overflow")
        }
    }
}
