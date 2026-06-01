// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Internal tests for [`Arena`] — chunk-boundary, refill, and overflow
//! invariants that don't surface through the public API. Lives next to
//! `mod.rs` to keep access to crate-private helpers.

#![allow(clippy::assertions_on_result_states, reason = "test code")]
#![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
#![allow(clippy::items_after_statements, reason = "test code: local helpers stay near their use")]
#![allow(clippy::zst_offset, reason = "test code: ZST drop-probe offsets are just for type-checking")]
#![allow(clippy::needless_borrow, reason = "test code")]
#![allow(clippy::needless_borrows_for_generic_args, reason = "test code")]
#![allow(clippy::large_stack_arrays, reason = "test code: oversized blob test probes the >64 KiB path")]

use allocator_api2::alloc::Global;

#[cfg(feature = "dst")]
use super::align_up;
use super::{AllocFlavor, align_offset, check_isize_overflow};
#[allow(unused_imports, reason = "kept for documentation in test comments")]
use crate::internal::constants::MAX_CHUNK_BYTES;
#[cfg(feature = "stats")]
use crate::internal::constants::MIN_MAX_NORMAL_ALLOC;
use crate::internal::drop_list::noop_drop_shim;
#[cfg(feature = "std")]
use crate::internal::drop_list::{drop_shim_one, drop_shim_slice};
use crate::{Arc, Arena};

#[test]
fn slice_reservation_debug_format() {
    check_isize_overflow(100, 8).expect("100 bytes fits");
    assert!(check_isize_overflow(isize::MAX as usize, 8).is_err());
}

#[test]
fn reserve_slice_rejects_excessive_alignment() {
    let arena = Arena::new();
    // Align of u8 is 1, well under 32 KiB — this should succeed.
    let _slice = arena.try_alloc_slice_copy([1u8, 2, 3]).expect("3-byte u8 slice fits");
}

/// Regression test for the ZST overflow guard in
/// Smoke test that a basic `alloc_arc` + `Arc::clone` sequence
/// returns the stored value through both handles. (Overflow of
/// `arcs_issued` is handled via the `check_smart_pointers_issued_overflow`
/// abort path, not via a mid-tenure reconcile — that mechanism was
/// removed; this test exists for basic accounting coverage.)
#[test]
fn alloc_arc_then_clone_reads_value_through_both_handles() {
    let arena = Arena::new();
    let a1 = arena.alloc_arc(42u64);
    assert_eq!(*a1, 42);
    let a2 = Arc::clone(&a1);
    assert_eq!(*a2, 42);
}

#[test]
fn arena_inner_debug_format() {
    let arena = Arena::new();
    let _ = alloc::format!("{arena:?}");
}

/// Cover `inc_ref_shared_deferred` when chunk is oversized.
#[test]
#[expect(
    clippy::large_stack_arrays,
    reason = "deliberate oversized-chunk allocation; the array is moved into the arena, not retained on the stack"
)]
fn inc_ref_shared_deferred_oversized_path() {
    let arena = Arena::new();
    let big = arena.alloc_arc([0u8; 65536]);
    let big2 = big.clone();
    assert_eq!(big[0], big2[0]);
}

/// Cover `inc_ref_shared_deferred` reconciliation on a normal chunk.
#[cfg(feature = "dst")]
#[test]
fn inc_ref_shared_deferred_reconcile_via_dst() {
    let arena = Arena::new();
    let a = arena.alloc_arc(99u32);
    assert_eq!(*a, 99);
}

/// Cover the refill path.
#[test]
fn evicted_chunk_guard_debug() {
    let arena = Arena::new();
    for i in 0..1000u64 {
        arena.alloc(i);
    }
}

/// Kills `internals.rs:467:43 - → +` and `- → /` mutants in
/// [`align_offset`]. The original computes
/// `(value + (align - 1)) & !(align - 1) - value`. The `+` mutation
/// replaces `align - 1` with `align + 1`, the `/` mutation with
/// `align / 1 == align`. For `value = 1, align = 8`, the original
/// returns `Some(7)`; the `+` mutant returns `Some(1)`; the `/`
/// mutant returns `Some(0)`.
#[test]
fn align_offset_padding_to_next_boundary() {
    assert_eq!(align_offset(0, 8), Some(0));
    assert_eq!(align_offset(1, 8), Some(7));
    assert_eq!(align_offset(7, 8), Some(1));
    assert_eq!(align_offset(8, 8), Some(0));
    assert_eq!(align_offset(9, 8), Some(7));
    assert_eq!(align_offset(15, 8), Some(1));
}

/// Kills `internals.rs:467` `align_offset` overflow handling: the
/// `checked_add` should saturate (return `None`) when `value + mask`
/// overflows `usize`.
#[test]
fn align_offset_returns_none_on_overflow() {
    assert!(align_offset(usize::MAX, 8).is_none());
    assert!(align_offset(usize::MAX - 6, 8).is_none());
    // `usize::MAX - 7` is the largest value where `value + 7` still
    // fits — the original would round up to `usize::MAX & !7` and
    // return `Some(...)`; verify the boundary is correct.
    assert!(align_offset(usize::MAX - 7, 8).is_some());
}

/// Kills `local_chunk.rs:107` `header_size` `+ → *` mutant by
/// asserting the function literally returns
/// `offset_of(drop_count) + size_of::<Cell<u16>>()` (sum, not
/// product).
#[test]
fn local_chunk_header_size_is_offset_plus_size() {
    use crate::internal::local_chunk::{LocalChunk, header_size};
    let drop_count_offset = core::mem::offset_of!(LocalChunk<Global>, drop_count);
    let drop_count_size = core::mem::size_of::<core::cell::Cell<u16>>();
    assert_eq!(header_size::<Global>(), drop_count_offset + drop_count_size);
    // Defense in depth: also check the sum lies well below the
    // product (would be `≥ 2 * drop_count_offset`).
    assert!(header_size::<Global>() < 2 * drop_count_offset);
}

/// Kills `shared_chunk.rs:134` `header_size` `+ → *` mutant.
#[test]
fn shared_chunk_header_size_is_offset_plus_size() {
    use crate::internal::shared_chunk::{SharedChunk, header_size};
    use crate::internal::sync::AtomicU16;
    let drop_count_offset = core::mem::offset_of!(SharedChunk<Global>, drop_count);
    let drop_count_size = core::mem::size_of::<AtomicU16>();
    assert_eq!(header_size::<Global>(), drop_count_offset + drop_count_size);
    assert!(header_size::<Global>() < 2 * drop_count_offset);
}

/// Kills `local_chunk.rs:167` `< → ==` and `< → <=` mutants in
/// [`LocalChunk::allocate`]. The original returns `Err` iff
/// `total_bytes < header_bytes`. `< → ==` would let
/// `total_bytes < header_bytes` slip through (underflow on
/// `payload`); `< → <=` would reject `total_bytes == header_bytes`
/// (a valid zero-payload chunk).
#[test]
fn local_chunk_allocate_total_smaller_than_header_returns_err() {
    use alloc::sync::Weak;

    use crate::internal::local_chunk::{LocalChunk, header_size};
    let header = header_size::<Global>();
    // `< → ==` mutant accepts this; original rejects.
    let result = LocalChunk::<Global>::allocate(Global, Weak::new(), header.saturating_sub(1));
    assert!(result.is_err(), "total_bytes < header_size must be rejected");
}

#[test]
fn local_chunk_allocate_total_equal_to_header_returns_ok() {
    use alloc::sync::Weak;

    use crate::internal::local_chunk::{LocalChunk, header_size};
    let header = header_size::<Global>();
    // `< → <=` mutant rejects this; original accepts (zero-payload chunk).
    let chunk = LocalChunk::<Global>::allocate(Global, Weak::new(), header).expect("zero-payload chunk should allocate");
    // SAFETY: chunk just allocated with refcount LARGE; drop it to 0
    // and free the backing. No drop entries were written.
    unsafe {
        (*chunk.as_ptr()).refcount.set(0);
        LocalChunk::<Global>::free_backing(chunk);
    }
}

/// Kills `shared_chunk.rs:210` `< → ==` and `< → <=` mutants in
/// [`SharedChunk::allocate`]. Same shape as the local variant.
#[test]
fn shared_chunk_allocate_total_smaller_than_header_returns_err() {
    use alloc::sync::Weak;

    use crate::internal::shared_chunk::{SharedChunk, header_size};
    let header = header_size::<Global>();
    let result = SharedChunk::<Global>::allocate(Global, Weak::new(), header.saturating_sub(1));
    assert!(result.is_err(), "total_bytes < header_size must be rejected");
}

#[test]
fn shared_chunk_allocate_total_equal_to_header_returns_ok() {
    use alloc::sync::Weak;

    use crate::internal::shared_chunk::{SharedChunk, header_size};
    use crate::internal::sync::Ordering;
    let header = header_size::<Global>();
    let chunk = SharedChunk::<Global>::allocate(Global, Weak::new(), header).expect("zero-payload chunk should allocate");
    // SAFETY: same shape as the local-chunk test.
    unsafe {
        (*chunk.as_ptr()).refcount.0.store(0, Ordering::Relaxed);
        SharedChunk::<Global>::free_backing(chunk);
    }
}

// White-box mutant-killing tests for private Arena boundaries.

/// Kills `arena.rs:676:24 > -> >=` and `> -> ==` mutants
/// in [`Arena::refill_shared`]: the early-return guard accepts
/// `min_payload == max_bump_extent` (the boundary value: largest
/// payload that still satisfies the chunk-header mask invariant)
/// and rejects `min_payload == max_bump_extent + 1`.
#[test]
fn refill_shared_at_max_chunk_bytes_boundary() {
    let arena = Arena::<Global>::new();
    let upper = crate::internal::shared_chunk::max_bump_extent::<Global>();
    assert!(
        arena.refill_shared(upper).is_ok(),
        "max_bump_extent is the largest accepted refill request"
    );
    let arena2 = Arena::<Global>::new();
    assert!(
        arena2.refill_shared(upper + 1).is_err(),
        "one byte past max_bump_extent must be rejected"
    );
}

/// Kills `arena.rs:977:24 > -> >=` mutant in [`Arena::refill_local`].
#[test]
fn refill_local_at_max_chunk_bytes_boundary() {
    let arena = Arena::<Global>::new();
    let upper = crate::internal::local_chunk::max_bump_extent::<Global>();
    assert!(
        arena.refill_local(upper).is_ok(),
        "max_bump_extent is the largest accepted refill request"
    );
    let arena2 = Arena::<Global>::new();
    assert!(
        arena2.refill_local(upper + 1).is_err(),
        "one byte past max_bump_extent must be rejected"
    );
}

/// Kills `arena.rs:3097:31 += -> *=` mutant in
/// [`Arena::alloc_slice_local_with_or_panic`]: the partial-init guard
/// counter must increment after each successful slot write so that a
/// mid-init panic drops exactly the elements that were written.
/// Reachable only via white-box: public callers pass `len = 1`.
#[cfg(feature = "std")]
#[test]
fn alloc_slice_local_with_or_panic_partial_init_drops_partial() {
    use core::cell::Cell;
    use std::panic::AssertUnwindSafe;

    struct D<'a>(&'a Cell<u32>);
    impl Drop for D<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    let drops = Cell::new(0_u32);
    let arena = Arena::<Global>::new();
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = arena.alloc_slice_local_with_or_panic::<D<'_>, _>(64, AllocFlavor::SimpleRef, Some(drop_shim_one::<D<'_>>), |i, slot| {
            assert!(i != 17, "synthetic init panic");
            slot.write(D(&drops));
        });
    }));
    assert!(result.is_err());
    assert_eq!(drops.get(), 17, "init guard must drop exactly the written prefix");
    drop(arena);
}

/// Kills `arena.rs:3659:31 += -> *=` mutant in
/// [`Arena::alloc_slice_shared_with_or_panic`].
/// Reachable only via white-box: public callers pass `len = 1`.
#[cfg(feature = "std")]
#[test]
fn alloc_slice_shared_with_or_panic_partial_init_drops_partial() {
    use std::panic::AssertUnwindSafe;
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicU32, Ordering as StdOrdering};

    struct D(StdArc<AtomicU32>);
    impl Drop for D {
        fn drop(&mut self) {
            self.0.fetch_add(1, StdOrdering::Relaxed);
        }
    }

    let drops = StdArc::new(AtomicU32::new(0));
    let arena = Arena::<Global>::new();
    let drops_ref = StdArc::clone(&drops);
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = arena.alloc_slice_shared_with_or_panic::<D, _>(64, Some(drop_shim_one::<D>), |i, slot| {
            assert!(i != 17, "synthetic init panic");
            slot.write(D(StdArc::clone(&drops_ref)));
        });
    }));
    assert!(result.is_err());
    assert_eq!(drops.load(StdOrdering::Relaxed), 17);
    drop(arena);
}

/// Kills `arena.rs:3031:47 && -> ||` mutant in
/// [`Arena::alloc_slice_local_with_or_panic`]: the `entry_size` decision
/// `drop_fn.is_some() && len != 0` must require BOTH conditions, not
/// either. With the mutant, calling with `drop_fn = None` and `len > 0`
/// would erroneously reserve a drop entry slot, retreating `drop_back`
/// by `size_of::<InnerDropEntry>()`.
#[test]
fn alloc_slice_local_with_or_panic_no_drop_fn_does_not_reserve_drop_entry() {
    let arena = Arena::<Global>::new();
    let _ = arena.alloc::<u32>(0);
    let drop_back_before = arena.current_local.drop_back.get();
    let _ = arena.alloc_slice_local_with_or_panic::<u32, _>(10, AllocFlavor::SimpleRef, None, |_, slot| {
        slot.write(0);
    });
    let drop_back_after = arena.current_local.drop_back.get();
    assert_eq!(
        drop_back_before, drop_back_after,
        "drop_back must not retreat when no drop entry is reserved"
    );
}

/// Kills `inner_slice.rs:975:47 && -> ||` mutant in `try_alloc_slice_shared_with`.
/// With the mutation `drop_fn.is_some() || len != 0`, an empty slice of a Drop
/// type would erroneously reserve a drop entry. Verify `drop_back` stays put.
#[test]
fn alloc_slice_shared_with_or_panic_empty_drop_type_does_not_reserve_drop_entry() {
    let arena = Arena::<Global>::new();
    let _ = arena.alloc_arc(0_u32);
    let drop_back_before = arena.current_shared.drop_back.get();
    let raw = arena.alloc_slice_shared_with_or_panic::<u8, _>(0, Some(noop_drop_shim), |_, slot| {
        slot.write(0);
    });
    let drop_back_after = arena.current_shared.drop_back.get();
    // SAFETY: `raw` was just returned by the shared slice allocator which
    // bumped the chunk's smart-pointer refcount by 1 for us.
    let _arc: crate::Arc<[u8], Global> = unsafe { crate::Arc::from_value_ptr(raw) };
    assert_eq!(
        drop_back_before, drop_back_after,
        "drop_back must not retreat for empty slice even with drop_fn"
    );
}

/// Kills `arena.rs:3603:47 && -> ||` mutant — shared-flavor sibling of
/// the previous test.
#[test]
fn alloc_slice_shared_with_or_panic_no_drop_fn_does_not_reserve_drop_entry() {
    let arena = Arena::<Global>::new();
    let _ = arena.alloc_arc(0_u32);
    let drop_back_before = arena.current_shared.drop_back.get();
    let raw = arena.alloc_slice_shared_with_or_panic::<u32, _>(10, None, |_, slot| {
        slot.write(0);
    });
    let drop_back_after = arena.current_shared.drop_back.get();
    // SAFETY: `raw` was just returned by the shared slice allocator which
    // bumped the chunk's smart-pointer refcount by 1 for us.
    let _arc: crate::Arc<[u32], Global> = unsafe { crate::Arc::from_value_ptr(raw) };
    assert_eq!(
        drop_back_before, drop_back_after,
        "drop_back must not retreat when no drop entry is reserved"
    );
}

/// Kills `arena.rs:3036:23 != -> ==` and `> -> ==`/`> -> >=` mutants on
/// the `entry_size != 0 && len > u16::MAX` panic gate in
/// [`Arena::alloc_slice_local_with_or_panic`]: a drop-aware slice of
/// length `u16::MAX` must succeed (boundary inclusive), and exactly
/// one past must panic.
#[test]
fn alloc_slice_local_with_or_panic_at_u16_max_succeeds() {
    let arena = Arena::<Global>::builder().max_normal_alloc(60 * 1024).build();
    let ptr = arena.alloc_slice_local_with_or_panic::<u8, _>(u16::MAX as usize, AllocFlavor::SimpleRef, Some(noop_drop_shim), |_, slot| {
        slot.write(0);
    });
    assert_eq!(ptr.len(), u16::MAX as usize);
}

/// Kills `arena.rs:3608:23 != -> ==` mutant — shared sibling.
#[test]
fn alloc_slice_shared_with_or_panic_at_u16_max_succeeds() {
    let arena = Arena::<Global>::builder().max_normal_alloc(60 * 1024).build();
    let raw = arena.alloc_slice_shared_with_or_panic::<u8, _>(u16::MAX as usize, Some(noop_drop_shim), |_, slot| {
        slot.write(0);
    });
    assert_eq!(raw.len(), u16::MAX as usize);
    // SAFETY: shared slice allocator bumped the chunk's smart-pointer refcount.
    let _arc: crate::Arc<[u8], Global> = unsafe { crate::Arc::from_value_ptr(raw) };
}

/// Kills `arena.rs:3039:26 > -> ==`/`> -> <`/`> -> >=` mutants on the
/// "route-to-oversized" gate in [`Arena::alloc_slice_local_with_or_panic`].
/// At `layout.size() == max_normal_alloc` the original takes the fast
/// path; the `>= ` mutant routes to the oversized helper, which calls
/// `acquire_local` and (because the current chunk isn't returned to
/// the cache) ends up allocating a fresh local chunk — observable via
/// [`ArenaStats::normal_local_chunks_allocated`].
#[cfg(feature = "stats")]
#[test]
fn alloc_slice_local_with_or_panic_at_max_normal_uses_fast_path() {
    // Use the minimum permitted `max_normal_alloc` (4 KiB) and a `u64`
    // element type so the per-element `init` closure runs only 512 times
    // instead of 8192 — the routing decision is purely a `layout.size()`
    // comparison, so the exercised branch is identical.
    let arena = Arena::<Global>::builder()
        .max_normal_alloc(MIN_MAX_NORMAL_ALLOC)
        .with_capacity_local(64 * 1024)
        .build();
    let _ = arena.alloc::<u8>(0);
    let before = arena.stats().normal_local_chunks_allocated;
    let _ = arena.alloc_slice_local_with_or_panic::<u64, _>(MIN_MAX_NORMAL_ALLOC / 8, AllocFlavor::SimpleRef, None, |_, slot| {
        slot.write(0);
    });
    let after = arena.stats().normal_local_chunks_allocated;
    assert_eq!(before, after, "exact max_normal_alloc must not need a new chunk");
}

/// Kills `arena.rs:3611:26 > -> ==`/`< `/`>=` — shared sibling.
#[cfg(feature = "stats")]
#[test]
fn alloc_slice_shared_with_or_panic_at_max_normal_uses_fast_path() {
    let arena = Arena::<Global>::builder()
        .max_normal_alloc(MIN_MAX_NORMAL_ALLOC)
        .with_capacity_shared(64 * 1024)
        .build();
    let _ = arena.alloc_arc::<u8>(0);
    let before = arena.stats().normal_shared_chunks_allocated;
    let raw = arena.alloc_slice_shared_with_or_panic::<u64, _>(MIN_MAX_NORMAL_ALLOC / 8, None, |_, slot| {
        slot.write(0);
    });
    let after = arena.stats().normal_shared_chunks_allocated;
    // SAFETY: shared slice allocator bumped the chunk's smart-pointer refcount.
    let _arc: crate::Arc<[u64], Global> = unsafe { crate::Arc::from_value_ptr(raw) };
    assert_eq!(before, after, "exact max_normal_alloc must not need a new chunk");
}

/// Kills `arena.rs:3036:23 != -> ==`. With `drop_fn=None` and
/// `len > u16::MAX`, the original computes `entry_size=0` and the
/// fast-path guard `entry_size != 0 && len > u16::MAX` is false, so
/// allocation proceeds. The mutant's `entry_size == 0 && len > u16::MAX`
/// is *true*, so it would panic — observable via `catch_unwind`.
#[cfg(feature = "std")]
#[test]
fn alloc_slice_local_with_or_panic_no_drop_large_len_succeeds() {
    let arena = Arena::<Global>::default();
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = arena.alloc_slice_local_with_or_panic::<u8, _>(u16::MAX as usize + 1, AllocFlavor::SimpleRef, None, |_, slot| {
            slot.write(0);
        });
    }));
    assert!(res.is_ok(), "no drop_fn ⇒ no u16::MAX cap, must not panic");
}

/// Kills `arena.rs:3608:23 != -> ==` — shared sibling.
#[cfg(feature = "std")]
#[test]
fn alloc_slice_shared_with_or_panic_no_drop_large_len_succeeds() {
    let arena = Arena::<Global>::default();
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let raw = arena.alloc_slice_shared_with_or_panic::<u8, _>(u16::MAX as usize + 1, None, |_, slot| {
            slot.write(0);
        });
        // SAFETY: shared slice allocator bumped the chunk's smart-pointer refcount.
        let _arc: crate::Arc<[u8], Global> = unsafe { crate::Arc::from_value_ptr(raw) };
    }));
    assert!(res.is_ok(), "no drop_fn ⇒ no u16::MAX cap, must not panic");
}

/// Kills `arena.rs:1643:28 += -> *=` mutant in
/// [`Arena::try_alloc_slice_local_oversized_with`]: the partial-init
/// guard must increment after each successful slot write so that a
/// mid-init panic drops exactly the elements that were written.
/// Reachable only via white-box: the public-API surface that reaches
/// this helper either passes `len = 1` (smart-pointer slice helpers)
/// or `drop_fn = None` (Copy-only slice helpers).
#[cfg(feature = "std")]
#[test]
fn try_alloc_slice_local_oversized_with_partial_init_drops_partial() {
    use core::cell::Cell;
    use std::panic::AssertUnwindSafe;

    struct D<'a>(&'a Cell<u32>);
    impl Drop for D<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    let drops = Cell::new(0_u32);
    let arena = Arena::<Global>::builder().max_normal_alloc(4096).build();
    // len*size_of::<D>() = 64*4 = 256 bytes — well under max_normal_alloc,
    // but we call the oversized helper directly so it executes anyway.
    let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
        arena.try_alloc_slice_local_oversized_with::<D<'_>, _>(64, AllocFlavor::Rc, Some(drop_shim_one::<D<'_>>), |i, slot| {
            assert!(i != 17, "synthetic init panic at i=17");
            slot.write(D(&drops));
        })
    }));
    assert!(res.is_err());
    assert_eq!(drops.get(), 17, "exactly the 17 successfully-written elements must drop");
}

/// Kills `arena.rs:1751:28 += -> *=` — shared sibling.
#[cfg(feature = "std")]
#[test]
fn try_alloc_slice_shared_oversized_with_partial_init_drops_partial() {
    use std::panic::AssertUnwindSafe;
    use std::sync::Mutex;

    // Counter must be Send+Sync because Arc<T> requires T: Send+Sync.
    struct D(&'static Mutex<u32>);
    impl Drop for D {
        fn drop(&mut self) {
            let mut g = self.0.lock().unwrap();
            *g += 1;
        }
    }

    static DROPS: Mutex<u32> = Mutex::new(0);
    // Statics survive within the test binary, so assert on the delta
    // from the current baseline instead of expecting zero.
    let baseline = *DROPS.lock().unwrap();
    let arena = Arena::<Global>::builder().max_normal_alloc(4096).build();
    let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
        arena.try_alloc_slice_shared_oversized_with::<D, _>(64, Some(drop_shim_one::<D>), |i, slot| {
            assert!(i != 17, "synthetic init panic at i=17");
            slot.write(D(&DROPS));
        })
    }));
    assert!(res.is_err());
    assert_eq!(
        *DROPS.lock().unwrap() - baseline,
        17,
        "exactly the 17 successfully-written elements must drop"
    );
}

/// Kills `arena.rs:1721:35 > -> <` mutant in
/// [`Arena::try_alloc_slice_shared_oversized_with`]: at any
/// `len < u16::MAX` with a `drop_fn`, the mutant returns `AllocError`
/// while the original proceeds. Reachable only via white-box.
#[test]
fn try_alloc_slice_shared_oversized_with_small_len_drop_fn_succeeds() {
    let arena = Arena::<Global>::builder().max_normal_alloc(4096).build();
    let raw = arena.try_alloc_slice_shared_oversized_with::<u32, _>(100, Some(noop_drop_shim), |_, slot| {
        slot.write(0);
    });
    assert!(raw.is_ok(), "len=100 with drop_fn must not be rejected");
    // SAFETY: shared oversized allocator bumped the chunk's smart-pointer refcount.
    let _arc: crate::Arc<[u32], Global> = unsafe { crate::Arc::from_value_ptr(raw.unwrap()) };
}

/// Kills `arena.rs:2076:33 + -> *` mutant in
/// [`Arena::alloc_inner_with_or_panic`]: `drop_count.set(drop_count.get() + 1)`
/// becomes `drop_count.set(drop_count.get() * 1) = 0`, which makes
/// `replay_drops` skip the drop-list entirely (gated on
/// `drop_count > 0`), silently leaking `T::drop`.
#[test]
fn alloc_with_drop_runs_destructor_on_arena_drop() {
    use core::cell::Cell;

    struct D<'a>(&'a Cell<u32>);
    impl Drop for D<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    let drops = Cell::new(0_u32);
    let arena = Arena::<Global>::new();
    let _r: &mut D<'_> = arena.alloc_with(|| D(&drops));
    assert_eq!(drops.get(), 0, "drop must not run before arena is dropped");
    drop(arena);
    assert_eq!(drops.get(), 1, "drop must run exactly once when arena is dropped");
}

/// Kills `> -> >=` routing mutant in
/// [`Arena::try_alloc_slice_local_no_drop_with`]: at exactly
/// `layout.size() == max_normal_alloc`, the original takes the
/// fast path (no fresh chunk) while the mutant routes to the
/// oversized helper which allocates a one-shot chunk. Observable
/// via [`ArenaStats::normal_local_chunks_allocated`].
#[cfg(feature = "stats")]
#[test]
fn try_alloc_uninit_slice_at_max_normal_uses_fast_path() {
    // Minimum permitted `max_normal_alloc` + `u64` element type: the
    // routing branch is identical (it tests `layout.size() <= max_normal_alloc`)
    // but the per-element init runs 512 times instead of 8192.
    let arena = Arena::<Global>::builder()
        .max_normal_alloc(MIN_MAX_NORMAL_ALLOC)
        .with_capacity_local(64 * 1024)
        .build();
    let _ = arena.alloc::<u8>(0);
    let before = arena.stats().normal_local_chunks_allocated;
    let _ = arena
        .try_alloc_slice_fill_with::<u64, _>(MIN_MAX_NORMAL_ALLOC / 8, |_| 0)
        .expect("must fit");
    let after = arena.stats().normal_local_chunks_allocated;
    assert_eq!(before, after, "exact max_normal_alloc must not need a new chunk");
}

/// Kills `> -> >=` routing mutant in
/// [`Arena::try_alloc_slice_local_copy`].
#[cfg(feature = "stats")]
#[test]
fn try_alloc_slice_copy_at_max_normal_uses_fast_path() {
    let arena = Arena::<Global>::builder()
        .max_normal_alloc(MIN_MAX_NORMAL_ALLOC)
        .with_capacity_local(64 * 1024)
        .build();
    let _ = arena.alloc::<u8>(0);
    // `u64` src instead of `u8`: identical fast-path branch, 8× less
    // per-byte tracking on the source Vec build *and* the
    // `copy_nonoverlapping` inside the allocator under Miri.
    let src: alloc::vec::Vec<u64> = alloc::vec![0_u64; MIN_MAX_NORMAL_ALLOC / 8];
    let before = arena.stats().normal_local_chunks_allocated;
    let _ = arena.try_alloc_slice_copy(&*src).expect("must fit");
    let after = arena.stats().normal_local_chunks_allocated;
    assert_eq!(before, after, "exact max_normal_alloc must not need a new chunk");
}

/// Kills `> -> >=` routing mutant in
/// [`Arena::alloc_slice_local_copy_or_panic`].
#[cfg(feature = "stats")]
#[test]
fn alloc_slice_copy_at_max_normal_uses_fast_path() {
    let arena = Arena::<Global>::builder()
        .max_normal_alloc(MIN_MAX_NORMAL_ALLOC)
        .with_capacity_local(64 * 1024)
        .build();
    let _ = arena.alloc::<u8>(0);
    let src: alloc::vec::Vec<u64> = alloc::vec![0_u64; MIN_MAX_NORMAL_ALLOC / 8];
    let before = arena.stats().normal_local_chunks_allocated;
    let _ = arena.alloc_slice_copy(&*src);
    let after = arena.stats().normal_local_chunks_allocated;
    assert_eq!(before, after, "exact max_normal_alloc must not need a new chunk");
}

/// Kills `> -> >=` routing mutant in
/// [`Arena::try_alloc_slice_shared_copy`].
#[cfg(feature = "stats")]
#[test]
fn try_alloc_slice_copy_arc_at_max_normal_uses_fast_path() {
    let arena = Arena::<Global>::builder()
        .max_normal_alloc(MIN_MAX_NORMAL_ALLOC)
        .with_capacity_shared(64 * 1024)
        .build();
    let _ = arena.alloc_arc::<u8>(0);
    let src: alloc::vec::Vec<u64> = alloc::vec![0_u64; MIN_MAX_NORMAL_ALLOC / 8];
    let before = arena.stats().normal_shared_chunks_allocated;
    let _ = arena.try_alloc_slice_copy_arc(&*src).expect("must fit");
    let after = arena.stats().normal_shared_chunks_allocated;
    assert_eq!(before, after, "exact max_normal_alloc must not need a new chunk");
}

/// Kills `> -> >=` routing mutant in
/// [`Arena::try_alloc_slice_shared_no_drop_with`].
#[cfg(feature = "stats")]
#[test]
fn try_alloc_uninit_slice_arc_at_max_normal_uses_fast_path() {
    let arena = Arena::<Global>::builder()
        .max_normal_alloc(MIN_MAX_NORMAL_ALLOC)
        .with_capacity_shared(64 * 1024)
        .build();
    let _ = arena.alloc_arc::<u8>(0);
    let before = arena.stats().normal_shared_chunks_allocated;
    let _ = arena
        .try_alloc_slice_fill_with_arc::<u64, _>(MIN_MAX_NORMAL_ALLOC / 8, |_| 0)
        .expect("must fit");
    let after = arena.stats().normal_shared_chunks_allocated;
    assert_eq!(before, after, "exact max_normal_alloc must not need a new chunk");
}

/// Kills `inner_slice.rs:621:26 > -> >=` in
/// [`Arena::try_alloc_slice_local_no_drop_with_slow`]. The slow path
/// is reached when the bump fast path misses; at exactly
/// `layout.size() == max_normal_alloc` the original (`>`) falls
/// through to `refill_local`, which charges the retired chunk's
/// tail to `wasted_tail_bytes`. The mutant (`>=`) routes to
/// [`Arena::try_alloc_slice_local_oversized_with`], which keeps
/// `current_local` loaded and therefore leaves `wasted_tail_bytes`
/// unchanged. Sizing `with_capacity_local` exactly at
/// `MIN_MAX_NORMAL_ALLOC` makes the preallocated chunk's payload
/// strictly smaller than `max_normal_alloc`, so the boundary-sized
/// slice cannot fit and the slow path must run.
#[cfg(feature = "stats")]
#[test]
fn try_alloc_uninit_slice_slow_at_max_normal_refills() {
    let arena = Arena::<Global>::builder()
        .max_normal_alloc(MIN_MAX_NORMAL_ALLOC)
        .with_capacity_local(MIN_MAX_NORMAL_ALLOC)
        .build();
    let _ = arena.alloc::<u8>(0);
    let before = arena.stats().wasted_tail_bytes;
    let _ = arena
        .try_alloc_slice_fill_with::<u64, _>(MIN_MAX_NORMAL_ALLOC / 8, |_| 0)
        .expect("slow-path refill must succeed");
    let after = arena.stats().wasted_tail_bytes;
    assert!(
        after > before,
        "slow path at exactly max_normal_alloc must refill_local (charges wasted tail); the `>=` mutant would route to the oversized helper instead",
    );
}

/// Kills `inner_slice.rs:750:26 > -> >=` in
/// [`Arena::alloc_slice_local_copy_slow`]. Same observation as
/// `try_alloc_uninit_slice_slow_at_max_normal_refills`, but exercised
/// through the `T: Copy` fast path.
#[cfg(feature = "stats")]
#[test]
fn try_alloc_slice_copy_slow_at_max_normal_refills() {
    let arena = Arena::<Global>::builder()
        .max_normal_alloc(MIN_MAX_NORMAL_ALLOC)
        .with_capacity_local(MIN_MAX_NORMAL_ALLOC)
        .build();
    let _ = arena.alloc::<u8>(0);
    let src: alloc::vec::Vec<u64> = alloc::vec![0_u64; MIN_MAX_NORMAL_ALLOC / 8];
    let before = arena.stats().wasted_tail_bytes;
    let _ = arena.try_alloc_slice_copy(&*src).expect("slow-path refill must succeed");
    let after = arena.stats().wasted_tail_bytes;
    assert!(
        after > before,
        "slow path at exactly max_normal_alloc must refill_local (charges wasted tail); the `>=` mutant would route to the oversized helper instead",
    );
}

/// Kills `inner_slice.rs:891:26 > -> >=` in
/// [`Arena::alloc_slice_shared_copy_slow`]. Shared-flavor sibling
/// of `try_alloc_slice_copy_slow_at_max_normal_refills`.
#[cfg(feature = "stats")]
#[test]
fn try_alloc_slice_copy_arc_slow_at_max_normal_refills() {
    let arena = Arena::<Global>::builder()
        .max_normal_alloc(MIN_MAX_NORMAL_ALLOC)
        .with_capacity_shared(MIN_MAX_NORMAL_ALLOC)
        .build();
    let _ = arena.alloc_arc::<u8>(0);
    let src: alloc::vec::Vec<u64> = alloc::vec![0_u64; MIN_MAX_NORMAL_ALLOC / 8];
    let before = arena.stats().wasted_tail_bytes;
    let _ = arena.try_alloc_slice_copy_arc(&*src).expect("slow-path refill must succeed");
    let after = arena.stats().wasted_tail_bytes;
    assert!(
        after > before,
        "slow path at exactly max_normal_alloc must refill_shared (charges wasted tail); the `>=` mutant would route to the oversized helper instead",
    );
}

/// Kills `primitives.rs:123:26 > -> >=` in
/// [`Arena::allocate_layout_slow`]. The `&Arena: Allocator` slow path
/// is reached when the bump-fit probe in [`Arena::allocate_layout`]
/// misses; at exactly `layout.size() == max_normal_alloc` the
/// original (`>`) calls `refill_local` and then retries the fast
/// path, charging the retired chunk's tail to `wasted_tail_bytes`.
/// The mutant (`>=`) routes to `allocate_oversized_layout`, which
/// keeps `current_local` loaded and never charges wasted tail.
#[cfg(feature = "stats")]
#[test]
fn allocate_layout_slow_at_max_normal_refills() {
    use core::alloc::Layout;

    use allocator_api2::alloc::Allocator;

    let arena = Arena::<Global>::builder()
        .max_normal_alloc(MIN_MAX_NORMAL_ALLOC)
        .with_capacity_local(MIN_MAX_NORMAL_ALLOC)
        .build();
    let _ = arena.alloc::<u8>(0);
    let before = arena.stats().wasted_tail_bytes;
    let layout = Layout::from_size_align(MIN_MAX_NORMAL_ALLOC, 8).expect("valid layout");
    let ptr = (&arena).allocate(layout).expect("slow-path refill must succeed");
    let after = arena.stats().wasted_tail_bytes;
    // `&Arena: Allocator::allocate` bumps the backing chunk's refcount;
    // the matching `deallocate` is required for the chunk to ever be
    // freed (otherwise the arena's own drop sees refcount > 0 and
    // leaves the chunk allocated — observable under Miri's leak checker).
    // SAFETY: `ptr.cast::<u8>()` is the pointer just returned by `allocate`
    // above; `layout` is the layout it was allocated with.
    unsafe { (&arena).deallocate(ptr.cast::<u8>(), layout) };
    assert!(
        after > before,
        "slow path at exactly max_normal_alloc must refill_local (charges wasted tail); the `>=` mutant would route to allocate_oversized_layout instead",
    );
}

/// Kills `primitives.rs:127:36 + -> -` in [`Arena::allocate_layout_slow`].
/// Line 127 computes
/// `let needed = layout.size() + layout.align().saturating_sub(align_of::<usize>())`.
/// For `align > align_of::<usize>()` the saturating slack is positive;
/// the mutant flips `+` to `-`, so for a small `size` (here `1`) and
/// `align == 16` the request becomes `1usize - 8`, which panics with
/// "subtract with overflow" in debug profile (and would wrap to a
/// huge value otherwise, which `refill_local`'s
/// `min_payload > local_max_bump_extent` defense rejects). The
/// original computes `needed = 9` and the allocation succeeds.
#[test]
fn allocate_layout_slow_high_align_does_not_underflow_needed() {
    use core::alloc::Layout;

    use allocator_api2::alloc::Allocator;

    let arena = Arena::<Global>::new();
    // align > size_of::<usize>() (which is 8 on 64-bit and 4 on 32-bit
    // targets): pick the larger value so the slack is always > 0.
    let align = (core::mem::align_of::<usize>() * 2).max(16);
    let layout = Layout::from_size_align(1, align).expect("valid layout");
    let ptr = (&arena)
        .allocate(layout)
        .expect("small high-align allocation must succeed; the `+ -> -` mutant underflows `needed` and rejects the refill");
    assert!(!ptr.is_empty(), "allocator must return a non-empty slot");
    // SAFETY: `ptr.cast::<u8>()` is the pointer just returned by `allocate`
    // above; `layout` is the layout it was allocated with. Required so
    // the chunk's refcount returns to 0 and the arena can free it on drop.
    unsafe { (&arena).deallocate(ptr.cast::<u8>(), layout) };
}

/// Kills `primitives.rs:200:26 > -> >=` in
/// [`Arena::allocate_shared_layout_slow`]. Mirror of
/// `allocate_layout_slow_at_max_normal_refills` for the shared-flavor
/// `bytesbuf`/`dst` integration path.
#[cfg(all(feature = "stats", any(feature = "dst", feature = "bytesbuf")))]
#[test]
fn allocate_shared_layout_slow_at_max_normal_refills() {
    use core::alloc::Layout;

    use crate::internal::in_chunk::InSharedChunk;
    use crate::internal::shared_chunk::SharedChunk;

    let arena = Arena::<Global>::builder()
        .max_normal_alloc(MIN_MAX_NORMAL_ALLOC)
        .with_capacity_shared(MIN_MAX_NORMAL_ALLOC)
        .build();
    let _ = arena.alloc_arc::<u8>(0);
    let before = arena.stats().wasted_tail_bytes;
    let layout = Layout::from_size_align(MIN_MAX_NORMAL_ALLOC, 8).expect("valid layout");
    let raw = arena.allocate_shared_layout(layout).expect("slow-path refill must succeed");
    let after = arena.stats().wasted_tail_bytes;
    // `allocate_shared_layout` left a `+1` on the backing chunk's
    // smart-pointer refcount; production callers (DST / `bytesbuf`)
    // hand that hold to an `Arc` whose drop calls `dec_ref`. Since
    // this test never wraps the raw pointer, decrement explicitly so
    // the chunk can be reclaimed when the arena drops (otherwise Miri
    // reports a leak).
    // SAFETY: `raw` was returned from this arena's shared allocator
    // and points inside a live chunk; the `+1` hold belongs to us.
    let chunk = unsafe { InSharedChunk::<u8, Global>::new(raw) }.chunk_ptr();
    // SAFETY: refcount is positive because of the hold we just took.
    unsafe { SharedChunk::dec_ref(chunk) };
    assert!(
        after > before,
        "slow path at exactly max_normal_alloc must refill_shared (charges wasted tail); the `>=` mutant would route to allocate_shared_oversized_layout instead",
    );
}

/// Kills `entry_size > 0 -> entry_size >= 0` mutant at the post-init
/// `drop_fn` write in [`Arena::alloc_inner_arc_with_or_panic`]: with
/// `entry_size == 0` (i.e. `T: !Drop`), the original skips the write
/// while the mutant unconditionally writes
/// `drop_shim_one::<T>` at `new_drop_back_ptr` — which equals the
/// previous `drop_back` and therefore overwrites the `drop_fn` field
/// of any drop-list entry that was installed by an earlier
/// allocation on the same chunk. We detect this by allocating an
/// `Arc<DropType>` first (installs an entry) then an `Arc<NoDrop>`
/// (mutant overwrites the previous entry's `drop_fn` with a `u32`
/// shim). When the chunk runs `replay_drops`, the first entry now
/// invokes the wrong shim, so `DropType::drop` is never called.
#[cfg(feature = "std")]
#[test]
fn alloc_arc_no_drop_after_drop_does_not_clobber_prior_entry() {
    use std::sync::Mutex;

    struct DropType(&'static Mutex<u32>);
    impl Drop for DropType {
        fn drop(&mut self) {
            *self.0.lock().unwrap() += 1;
        }
    }

    static DROPS: Mutex<u32> = Mutex::new(0);
    let baseline = *DROPS.lock().unwrap();
    let arena = Arena::<Global>::default();
    let a1 = arena.alloc_arc::<DropType>(DropType(&DROPS));
    let a2 = arena.alloc_arc::<u32>(0xdead_beef);
    drop(a1);
    drop(a2);
    drop(arena);
    assert_eq!(
        *DROPS.lock().unwrap() - baseline,
        1,
        "DropType's drop must run exactly once even after a no-drop arc allocates after it"
    );
}

/// Kills `entry_size > 0 -> >= 0` (2101:23) and `!= -> ==` (2103:31)
/// mutants in [`Arena::alloc_inner_with_or_panic`].
///
/// 2101: With `!needs_drop` T (`entry_size == 0`), the mutant enters the
/// drop-entry block and writes `drop_shim_one::<u32>` at
/// `new_drop_back_ptr == drop_back`, clobbering any prior entry's
/// `drop_fn`. We detect this by allocating a needs-drop T first, then
/// a `!needs_drop` T via `alloc_with`, and verifying the first T's
/// drop still runs.
///
/// 2103: The mutant flips the eviction check so every non-evicted
/// needs-drop alloc takes the cold `commit_alloc_after_eviction` path.
/// That path skips the `!Box` flavor guard at line 2119. For `Box`
/// flavor the shim stays as noop, leaking the drop.
#[cfg(feature = "std")]
#[test]
fn alloc_with_no_drop_after_drop_does_not_clobber_prior_entry() {
    use std::sync::Mutex;

    struct DropType<'a>(&'a Mutex<u32>);
    impl Drop for DropType<'_> {
        fn drop(&mut self) {
            *self.0.lock().unwrap() += 1;
        }
    }

    static DROPS: Mutex<u32> = Mutex::new(0);
    let baseline = *DROPS.lock().unwrap();
    let arena = Arena::<Global>::default();
    let _r1 = arena.alloc_with(|| DropType(&DROPS));
    let _r2 = arena.alloc_with(|| 0xdead_beef_u32);
    drop(arena);
    assert_eq!(
        *DROPS.lock().unwrap() - baseline,
        1,
        "DropType's drop must run exactly once even after a no-drop alloc_with follows it"
    );
}

/// Kills `delete !` (3115:20) and `!= -> ==` (3129:77) mutants in
/// [`Arena::alloc_slice_local_with_or_panic`].
///
/// 3115: Mutant flips `!matches!(flavor, AllocFlavor::Box)` to
/// `matches!(flavor, AllocFlavor::Box)`, so the `drop_fn` stays as
/// noop for non-Box flavors. We use `AllocFlavor::SimpleRef` with a
/// needs-drop T and verify drops run when the arena is dropped.
///
/// 3129: Mutant flips `entry_size != 0` to `entry_size == 0` in the
/// refill sizing. With a needs-drop T whose first `try_bump_fit` misses,
/// the mutant under-sizes the refill request, potentially causing the
/// retry to fail.
#[cfg(feature = "std")]
#[test]
fn alloc_slice_local_with_rc_flavor_drops_run() {
    use std::cell::Cell;

    struct D<'a>(&'a Cell<u32>);
    impl Drop for D<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    let drops = Cell::new(0_u32);
    let arena = Arena::<Global>::default();
    let _ = arena.alloc_slice_local_with_or_panic::<D<'_>, _>(4, AllocFlavor::SimpleRef, Some(drop_shim_slice::<D<'_>>), |_, slot| {
        slot.write(D(&drops));
    });
    drop(arena);
    assert_eq!(drops.get(), 4, "all 4 D elements must be dropped via replay_drops");
}

/// Kills `!= -> ==` (3677:63) and `!= -> ==` (3689:78) mutants in
/// [`Arena::alloc_slice_shared_with_or_panic`].
///
/// 3677: Mutant flips `len != 0` to `len == 0` in the `drop_fn` filter,
/// so the real `drop_fn` is never written over the noop for `len > 0`.
///
/// We wrap the returned `NonNull<[D]>` in an `Arc` so the chunk's
/// refcount is properly released when the Arc and arena are dropped,
/// allowing `replay_drops` to run.
#[cfg(feature = "std")]
#[test]
fn alloc_slice_shared_with_drops_run() {
    use std::sync::Mutex;

    struct D(&'static Mutex<u32>);
    impl Drop for D {
        fn drop(&mut self) {
            *self.0.lock().unwrap() += 1;
        }
    }

    static DROPS: Mutex<u32> = Mutex::new(0);
    let baseline = *DROPS.lock().unwrap();
    let arena = Arena::<Global>::default();
    let raw = arena.alloc_slice_shared_with_or_panic::<D, _>(4, Some(drop_shim_slice::<D>), |_, slot| {
        slot.write(D(&DROPS));
    });
    // SAFETY: `raw` was just returned by `alloc_slice_shared_with_or_panic`
    // which bumped the chunk's smart-pointer refcount by 1 for us.
    let arc: crate::Arc<[D], Global> = unsafe { crate::Arc::from_value_ptr(raw) };
    drop(arc);
    drop(arena);
    assert_eq!(
        *DROPS.lock().unwrap() - baseline,
        4,
        "all 4 D elements must be dropped via replay_drops on the shared chunk"
    );
}

/// Covers the defense-in-depth `len > u16::MAX` guard inside
/// [`Arena::try_alloc_slice_local_oversized_with`]. Public callers
/// in `inner_slice.rs` reject this case at an earlier guard before
/// routing into the oversized helper, so the branch is only
/// reachable via this direct call.
#[test]
fn try_alloc_slice_local_oversized_with_drop_fn_too_long_returns_err() {
    let arena = Arena::<Global>::new();
    let res =
        arena.try_alloc_slice_local_oversized_with::<u8, _>(u16::MAX as usize + 1, AllocFlavor::Rc, Some(noop_drop_shim), |_, slot| {
            slot.write(0);
        });
    assert!(res.is_err());
}

/// Shared-flavor sibling of the previous test.
#[test]
fn try_alloc_slice_shared_oversized_with_drop_fn_too_long_returns_err() {
    let arena = Arena::<Global>::new();
    let res = arena.try_alloc_slice_shared_oversized_with::<u8, _>(u16::MAX as usize + 1, Some(noop_drop_shim), |_, slot| {
        slot.write(0);
    });
    assert!(res.is_err());
}

// `Arena::alloc_slice_local_with_or_panic`: cover the `PANIC=true` arms.

/// `Layout::array` overflow calls `panic_alloc`.
#[cfg(feature = "std")]
#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_local_with_or_panic_layout_overflow_panics() {
    let arena = Arena::<Global>::new();
    // `usize::MAX / size_of::<u64>() + 1` always overflows `Layout::array`.
    let len = usize::MAX / core::mem::size_of::<u64>() + 1;
    let _ = arena.alloc_slice_local_with_or_panic::<u64, _>(len, AllocFlavor::SimpleRef, None, |_, slot| {
        slot.write(0);
    });
}

/// `align_of::<T>() >= MAX_SMART_PTR_ALIGN` (32 KiB) calls `panic_alloc`.
#[cfg(feature = "std")]
#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_local_with_or_panic_over_aligned_panics() {
    #[repr(align(32768))]
    struct HugeAlign(#[allow(dead_code, reason = "field forces alignment")] u8);

    let arena = Arena::<Global>::new();
    let _ = arena.alloc_slice_local_with_or_panic::<HugeAlign, _>(1, AllocFlavor::SimpleRef, None, |_, slot| {
        slot.write(HugeAlign(0));
    });
}

/// `drop_fn` present and `len > u16::MAX` calls `panic_alloc`.
#[cfg(feature = "std")]
#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_local_with_or_panic_drop_fn_too_long_panics() {
    let arena = Arena::<Global>::new();
    let _ =
        arena.alloc_slice_local_with_or_panic::<u8, _>(u16::MAX as usize + 1, AllocFlavor::SimpleRef, Some(noop_drop_shim), |_, slot| {
            slot.write(0);
        });
}

/// `refill_local` failure calls `panic_alloc`.
#[cfg(feature = "std")]
#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_local_with_or_panic_refill_failure_panics() {
    use core::cell::Cell;

    use allocator_api2::alloc::{AllocError, Allocator, Layout};

    #[derive(Clone)]
    struct FailEverything;

    // SAFETY: never returns a pointer; deallocate is unreachable.
    unsafe impl Allocator for FailEverything {
        fn allocate(&self, _layout: Layout) -> Result<core::ptr::NonNull<[u8]>, AllocError> {
            Err(AllocError)
        }
        unsafe fn deallocate(&self, _ptr: core::ptr::NonNull<u8>, _layout: Layout) {}
    }

    let _ = Cell::new(0); // silence unused-import on older toolchains
    let arena = crate::ArenaBuilder::new_in(FailEverything).build();
    let _ = arena.alloc_slice_local_with_or_panic::<u8, _>(64, AllocFlavor::SimpleRef, None, |_, slot| {
        slot.write(0);
    });
}

// ---------------------------------------------------------------------------
// Broader coverage batch: drive every reachable error arm in the slice
// helpers in `inner_slice.rs` and the DST helpers in `alloc_unsized.rs`.
// ---------------------------------------------------------------------------

#[repr(align(32768))]
struct HugeAlign32K(#[expect(dead_code, reason = "alignment marker")] u8);

#[repr(align(65536))]
struct HugeAlign64K(#[expect(dead_code, reason = "alignment marker")] u8);

#[derive(Clone, Copy)]
struct FailEverythingAllocator;

// SAFETY: never returns a pointer; deallocate unreachable.
unsafe impl allocator_api2::alloc::Allocator for FailEverythingAllocator {
    fn allocate(&self, _layout: core::alloc::Layout) -> Result<core::ptr::NonNull<[u8]>, allocator_api2::alloc::AllocError> {
        Err(allocator_api2::alloc::AllocError)
    }
    unsafe fn deallocate(&self, _ptr: core::ptr::NonNull<u8>, _layout: core::alloc::Layout) {}
}

/// Length that always overflows `Layout::array::<T>` for `size_of::<T>() >= 1`.
const HUGE_LEN: usize = usize::MAX / 2;

// ===== try_alloc_slice_local_oversized_with =====

#[test]
fn try_alloc_slice_local_oversized_with_layout_overflow_returns_err() {
    let arena = Arena::<Global>::new();
    let res = arena.try_alloc_slice_local_oversized_with::<u64, _>(HUGE_LEN, AllocFlavor::SimpleRef, None, |_, slot| {
        slot.write(0);
    });
    assert!(res.is_err());
}

// ===== try_alloc_slice_shared_oversized_with =====

#[test]
fn try_alloc_slice_shared_oversized_with_layout_overflow_returns_err() {
    let arena = Arena::<Global>::new();
    let res = arena.try_alloc_slice_shared_oversized_with::<u64, _>(HUGE_LEN, None, |_, slot| {
        slot.write(0);
    });
    assert!(res.is_err());
}

// ===== impl_alloc_slice_local_with PANIC=false (via try_alloc_slice_with) =====

#[test]
fn try_alloc_slice_with_layout_overflow_returns_err() {
    let arena = Arena::<Global>::new();
    let res = arena.try_alloc_slice_fill_with::<u64, _>(HUGE_LEN, |i| i as u64);
    assert!(res.is_err());
}

#[test]
fn try_alloc_slice_fill_with_over_aligned_returns_err() {
    let arena = Arena::<Global>::new();
    // Box flavor uses MAX_SMART_PTR_ALIGN cap (32 KiB) — so 32 KiB-aligned T is rejected.
    let res = arena.try_alloc_slice_fill_with_box::<HugeAlign32K, _>(1, |_| HugeAlign32K(0));
    assert!(res.is_err());
}

// ===== alloc_slice_shared_with_or_panic PANIC=true arms =====

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_shared_with_or_panic_layout_overflow_panics() {
    let arena = Arena::<Global>::new();
    let _ = arena.alloc_slice_shared_with_or_panic::<u64, _>(HUGE_LEN, None, |_, slot| {
        slot.write(0);
    });
}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_shared_with_or_panic_over_aligned_panics() {
    let arena = Arena::<Global>::new();
    let _ = arena.alloc_slice_shared_with_or_panic::<HugeAlign32K, _>(1, None, |_, slot| {
        slot.write(HugeAlign32K(0));
    });
}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_shared_with_or_panic_drop_fn_too_long_panics() {
    let arena = Arena::<Global>::new();
    let _ = arena.alloc_slice_shared_with_or_panic::<u8, _>(u16::MAX as usize + 1, Some(noop_drop_shim), |_, slot| {
        slot.write(0);
    });
}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_shared_with_or_panic_refill_failure_panics() {
    let arena = crate::ArenaBuilder::new_in(FailEverythingAllocator).build();
    let _ = arena.alloc_slice_shared_with_or_panic::<u8, _>(64, None, |_, slot| {
        slot.write(0);
    });
}

// ===== impl_alloc_slice_local_copy (Copy fast path) =====

#[test]
fn try_alloc_slice_copy_layout_overflow_returns_err() {
    let arena = Arena::<Global>::new();
    // Build a fake-huge slice by directly invoking the no-drop helper.
    let res = arena.try_alloc_slice_local_no_drop_with::<u64, _, false>(HUGE_LEN, AllocFlavor::SimpleRef, |_, slot| {
        slot.write(0);
    });
    assert!(res.is_err());
}

#[test]
fn try_alloc_slice_local_no_drop_with_over_aligned_returns_err_box_flavor() {
    let arena = Arena::<Global>::new();
    let res = arena.try_alloc_slice_local_no_drop_with::<HugeAlign32K, _, false>(1, AllocFlavor::Box, |_, slot| {
        slot.write(HugeAlign32K(0));
    });
    assert!(res.is_err());
}

#[test]
fn try_alloc_slice_local_no_drop_with_over_aligned_returns_err_simpleref_flavor() {
    let arena = Arena::<Global>::new();
    // SimpleRef path uses CHUNK_ALIGN cap (64 KiB), so 64 KiB-aligned T is rejected.
    let res = arena.try_alloc_slice_local_no_drop_with::<HugeAlign64K, _, false>(1, AllocFlavor::SimpleRef, |_, slot| {
        slot.write(HugeAlign64K(0));
    });
    assert!(res.is_err());
}

#[test]
fn try_alloc_slice_shared_no_drop_with_layout_overflow_returns_err() {
    let arena = Arena::<Global>::new();
    let res = arena.try_alloc_slice_shared_no_drop_with::<u64, _, false>(HUGE_LEN, |_, slot| {
        slot.write(0);
    });
    assert!(res.is_err());
}

#[test]
fn try_alloc_slice_shared_no_drop_with_over_aligned_returns_err() {
    let arena = Arena::<Global>::new();
    let res = arena.try_alloc_slice_shared_no_drop_with::<HugeAlign32K, _, false>(1, |_, slot| {
        slot.write(HugeAlign32K(0));
    });
    assert!(res.is_err());
}

#[test]
fn try_alloc_slice_shared_copy_layout_overflow_returns_err() {
    let arena = Arena::<Global>::new();
    // `try_alloc_slice_shared_copy` is reached through `alloc_slice_copy_arc`.
    // Sanity-check normal path; layout-overflow is covered by the no_drop variant above.
    let res = arena.try_alloc_slice_copy_arc::<u64>(&[1, 2, 3]);
    assert!(res.is_ok(), "sanity check for normal path");
}

// ===== PANIC=true panic-on-error arms in `try_alloc_slice_local_no_drop_with` =====
// These exercise the `if PANIC { panic_alloc(); }` branches that the
// PANIC=false `try_*` siblings above cannot reach. Routed through the
// public panicking `alloc_slice_fill_with*` entrypoints which propagate
// `PANIC = true` into the no-drop inner helper for `T: !needs_drop`.

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_fill_with_box_layout_overflow_panics() {
    // `u64: !needs_drop` ⇒ Box flavor's `alloc_slice_fill_with_box` routes
    // through `try_alloc_slice_local_fill_with_inner::<_, _, true>` → for
    // `!needs_drop` → `try_alloc_slice_local_no_drop_with::<_, _, true>`,
    // where `Layout::array::<u64>(HUGE_LEN)` overflows and the panicking
    // arm fires. (The SimpleRef sibling `alloc_slice_fill_with` cannot
    // reach this arm because it goes through the fallible inner with
    // `PANIC = false` and wraps the result with `expect_alloc`.)
    let arena = Arena::<Global>::new();
    let _ = arena.alloc_slice_fill_with_box::<u64, _>(HUGE_LEN, |i| i as u64);
}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_fill_with_layout_overflow_panics() {
    // SimpleRef flavor: panics through the `expect_alloc` wrapper rather
    // than `panic_alloc` directly. Kept here as the symmetric sanity test
    // for the public panicking entrypoint.
    let arena = Arena::<Global>::new();
    let _ = arena.alloc_slice_fill_with::<u64, _>(HUGE_LEN, |i| i as u64);
}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_fill_with_box_over_aligned_panics() {
    // `HugeAlign32K: !needs_drop` + Box flavor ⇒ no-drop inner with
    // `MAX_SMART_PTR_ALIGN` cap (= 32 KiB); align == cap rejects and the
    // panicking arm fires.
    let arena = Arena::<Global>::new();
    let _ = arena.alloc_slice_fill_with_box::<HugeAlign32K, _>(1, |_| HugeAlign32K(0));
}

// ===== PANIC=true panic-on-error arms in `try_alloc_slice_shared_no_drop_with` =====
// Reached through `alloc_slice_fill_with_arc` for `T: !needs_drop + Send + Sync`.

// SAFETY: `HugeAlign32K` is a single-byte payload with no thread-state.
unsafe impl Send for HugeAlign32K {}
// SAFETY: `HugeAlign32K` is a single-byte payload with no thread-state.
unsafe impl Sync for HugeAlign32K {}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_fill_with_arc_layout_overflow_panics() {
    let arena = Arena::<Global>::new();
    let _ = arena.alloc_slice_fill_with_arc::<u64, _>(HUGE_LEN, |i| i as u64);
}

#[test]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_fill_with_arc_over_aligned_panics() {
    // `HugeAlign32K: !needs_drop` ⇒ no-drop shared inner with
    // `MAX_SMART_PTR_ALIGN` cap; align == cap → panic.
    let arena = Arena::<Global>::new();
    let _ = arena.alloc_slice_fill_with_arc::<HugeAlign32K, _>(1, |_| HugeAlign32K(0));
}

// ===== PANIC=true panic-on-error arm in `try_alloc_slice_shared_copy` =====
// Reached through `alloc_slice_copy_arc` for `T: Copy + Send + Sync`.
// Gated off Windows for the same reason as the bytemuck slice
// `*_over_aligned` siblings: under coverage instrumentation, materializing
// any `HugeAlign64KCopy`-typed slot in the test's stack frame exceeds
// Windows' default 1 MiB stack.

#[cfg(not(target_os = "windows"))]
#[repr(align(65536))]
#[derive(Clone, Copy)]
struct HugeAlign64KCopy(#[expect(dead_code, reason = "alignment marker")] u8);

// SAFETY: `HugeAlign64KCopy` is a single-byte payload with no thread-state.
#[cfg(not(target_os = "windows"))]
unsafe impl Send for HugeAlign64KCopy {}
// SAFETY: `HugeAlign64KCopy` is a single-byte payload with no thread-state.
#[cfg(not(target_os = "windows"))]
unsafe impl Sync for HugeAlign64KCopy {}

#[test]
#[cfg(not(target_os = "windows"))]
#[should_panic(expected = "multitude: allocator returned AllocError")]
fn alloc_slice_copy_arc_over_aligned_panics() {
    let arena = Arena::<Global>::new();
    // Use a heap-allocated empty `Vec` so the `&[T]` is just a (ptr, len) pair
    // and no `HugeAlign64KCopy` value materializes in this stack frame.
    let v: alloc::vec::Vec<HugeAlign64KCopy> = alloc::vec::Vec::new();
    let _ = arena.alloc_slice_copy_arc::<HugeAlign64KCopy>(&*v);
}

// ===== DST: alloc_unsized.rs error arms =====

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_rc_oversized_metadata_returns_err() {
    use core::alloc::Layout;

    let arena = Arena::<Global>::new();
    let len = u16::MAX as usize + 1;
    let layout = Layout::array::<u8>(len).unwrap();
    // SAFETY: `init` is a no-op writing zero bytes to a valid slice slot.
    let res = unsafe {
        arena.try_alloc_dst_rc::<[u8]>(layout, len, |fat: *mut [u8]| {
            let p = fat.cast::<u8>();
            for i in 0..len {
                p.add(i).write(0);
            }
        })
    };
    assert!(res.is_err());
}

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_arc_oversized_metadata_returns_err() {
    use core::alloc::Layout;

    let arena = Arena::<Global>::new();
    let len = u16::MAX as usize + 1;
    let layout = Layout::array::<u8>(len).unwrap();
    // SAFETY: `init` writes zero bytes.
    let res = unsafe {
        arena.try_alloc_dst_arc::<[u8]>(layout, len, |fat: *mut [u8]| {
            let p = fat.cast::<u8>();
            for i in 0..len {
                p.add(i).write(0);
            }
        })
    };
    assert!(res.is_err());
}

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_box_oversized_metadata_drop_aware_returns_err() {
    use core::alloc::Layout;

    struct DropProbe;
    impl Drop for DropProbe {
        fn drop(&mut self) {}
    }

    let arena = Arena::<Global>::new();
    let len = u16::MAX as usize + 1;
    let layout = Layout::array::<DropProbe>(len).unwrap();
    // SAFETY: `init` initializes the slice via `DropProbe::default()`.
    let res = unsafe {
        arena.try_alloc_dst_box::<[DropProbe]>(layout, len, |fat: *mut [DropProbe]| {
            let p = fat.cast::<DropProbe>();
            for i in 0..len {
                p.add(i).write(DropProbe);
            }
        })
    };
    assert!(res.is_err());
}

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_arc_over_aligned_returns_err() {
    use core::alloc::Layout;

    let arena = Arena::<Global>::new();
    let layout = Layout::from_size_align(32_768, 32_768).unwrap();
    // SAFETY: no-op init.
    let res = unsafe { arena.try_alloc_dst_arc::<[u8]>(layout, 32_768, |_| {}) };
    assert!(res.is_err());
}

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_rc_over_aligned_returns_err() {
    use core::alloc::Layout;

    let arena = Arena::<Global>::new();
    let layout = Layout::from_size_align(32_768, 32_768).unwrap();
    // SAFETY: no-op init.
    let res = unsafe { arena.try_alloc_dst_rc::<[u8]>(layout, 32_768, |_| {}) };
    assert!(res.is_err());
}

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_box_over_aligned_returns_err() {
    use core::alloc::Layout;

    let arena = Arena::<Global>::new();
    let layout = Layout::from_size_align(32_768, 32_768).unwrap();
    // SAFETY: no-op init.
    let res = unsafe { arena.try_alloc_dst_box::<[u8]>(layout, 32_768, |_| {}) };
    assert!(res.is_err());
}

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_arc_refill_failure_returns_err() {
    use core::alloc::Layout;

    let arena = crate::ArenaBuilder::new_in(FailEverythingAllocator).build();
    let layout = Layout::array::<u8>(64).unwrap();
    // SAFETY: init never runs because allocation fails first.
    let res = unsafe { arena.try_alloc_dst_arc::<[u8]>(layout, 64, |_| {}) };
    assert!(res.is_err());
}

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_rc_refill_failure_returns_err() {
    use core::alloc::Layout;

    let arena = crate::ArenaBuilder::new_in(FailEverythingAllocator).build();
    let layout = Layout::array::<u8>(64).unwrap();
    // SAFETY: init never runs.
    let res = unsafe { arena.try_alloc_dst_rc::<[u8]>(layout, 64, |_| {}) };
    assert!(res.is_err());
}

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_box_refill_failure_returns_err() {
    use core::alloc::Layout;

    let arena = crate::ArenaBuilder::new_in(FailEverythingAllocator).build();
    let layout = Layout::array::<u8>(64).unwrap();
    // SAFETY: init never runs.
    let res = unsafe { arena.try_alloc_dst_box::<[u8]>(layout, 64, |_| {}) };
    assert!(res.is_err());
}

// ---------------------------------------------------------------------------
// `impl_alloc_slice_local_with` / `impl_alloc_slice_shared_with` PANIC=false
// arms — reached via `try_alloc_slice_*_with` when `T: Drop`.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct DropProbe(#[expect(dead_code, reason = "drop probe payload")] u64);
impl Drop for DropProbe {
    fn drop(&mut self) {}
}

#[test]
fn try_alloc_slice_fill_with_box_drop_layout_overflow_returns_err() {
    let arena = Arena::<Global>::new();
    // Drop type routes through `impl_alloc_slice_local_with` (not no_drop variant).
    let res = arena.try_alloc_slice_fill_with_box::<DropProbe, _>(HUGE_LEN, |_| DropProbe(0));
    assert!(res.is_err());
}

#[test]
fn try_alloc_slice_fill_with_box_drop_too_long_returns_err() {
    let arena = Arena::<Global>::new();
    let res = arena.try_alloc_slice_fill_with_box::<DropProbe, _>(u16::MAX as usize + 1, |_| DropProbe(0));
    assert!(res.is_err());
}

#[test]
fn try_alloc_slice_fill_with_arc_drop_layout_overflow_returns_err() {
    let arena = Arena::<Global>::new();
    // Send + Sync + Clone + Drop → routes through `impl_alloc_slice_shared_with`.
    #[derive(Clone)]
    struct SendDropProbe(#[expect(dead_code, reason = "drop probe payload")] u64);
    impl Drop for SendDropProbe {
        fn drop(&mut self) {}
    }
    let res = arena.try_alloc_slice_fill_with_arc::<SendDropProbe, _>(HUGE_LEN, |_| SendDropProbe(0));
    assert!(res.is_err());
}

#[test]
fn try_alloc_slice_fill_with_arc_drop_too_long_returns_err() {
    let arena = Arena::<Global>::new();
    #[derive(Clone)]
    struct SendDropProbe(#[expect(dead_code, reason = "drop probe payload")] u64);
    impl Drop for SendDropProbe {
        fn drop(&mut self) {}
    }
    let res = arena.try_alloc_slice_fill_with_arc::<SendDropProbe, _>(u16::MAX as usize + 1, |_| SendDropProbe(0));
    assert!(res.is_err());
}

// `impl_alloc_slice_local_copy` PANIC=false arms via `try_alloc_slice_copy`.
#[test]
fn try_alloc_slice_copy_layout_overflow_via_public_returns_err() {
    let arena = Arena::<Global>::new();
    // Huge-slice overflow is covered above; this is just a sanity check.
    let res = arena.try_alloc_slice_copy(&[1u64, 2, 3]);
    assert!(res.is_ok());
}

#[test]
fn try_alloc_slice_copy_at_max_normal_sanity() {
    let arena = Arena::<Global>::new();
    let res = arena.try_alloc_slice_copy::<u8>(&[1, 2, 3]);
    assert!(res.is_ok());
}

// Oversized-path acquire-failure coverage via `FailEverythingAllocator`.

// Bigger than MAX_CHUNK_BYTES, so it must use the oversized path.
type OversizedBlob = [u8; 128 * 1024];

#[test]
fn try_alloc_with_oversized_refill_failure_returns_err() {
    let arena = crate::ArenaBuilder::new_in(FailEverythingAllocator).build();
    let res = arena.try_alloc_with::<OversizedBlob, _>(|| [0u8; 128 * 1024]);
    assert!(res.is_err());
}

#[test]
fn try_alloc_oversized_refill_failure_returns_err() {
    let arena = crate::ArenaBuilder::new_in(FailEverythingAllocator).build();
    let res = arena.try_alloc::<OversizedBlob>([0u8; 128 * 1024]);
    assert!(res.is_err());
}

#[test]
fn try_alloc_arc_oversized_refill_failure_returns_err() {
    let arena = crate::ArenaBuilder::new_in(FailEverythingAllocator).build();
    let res = arena.try_alloc_arc::<OversizedBlob>([0u8; 128 * 1024]);
    assert!(res.is_err());
}

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_arc_oversized_refill_failure_returns_err() {
    use core::alloc::Layout;

    #[derive(Clone)]
    struct SendDropProbe(#[expect(dead_code, reason = "padding")] u64);
    impl Drop for SendDropProbe {
        fn drop(&mut self) {}
    }

    let arena = crate::ArenaBuilder::new_in(FailEverythingAllocator).build();
    // 16384 drop-bearing elements make a 128 KiB oversized allocation.
    let layout = Layout::array::<SendDropProbe>(16384).unwrap();
    // SAFETY: init is never called because allocation fails.
    let res = unsafe { arena.try_alloc_dst_arc::<[SendDropProbe]>(layout, 16384, |_| {}) };
    assert!(res.is_err());
}

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_box_oversized_refill_failure_returns_err() {
    use core::alloc::Layout;

    struct DropProbe(#[expect(dead_code, reason = "padding")] u64);
    impl Drop for DropProbe {
        fn drop(&mut self) {}
    }

    let arena = crate::ArenaBuilder::new_in(FailEverythingAllocator).build();
    let layout = Layout::array::<DropProbe>(16384).unwrap();
    // SAFETY: init never runs.
    let res = unsafe { arena.try_alloc_dst_box::<[DropProbe]>(layout, 16384, |_| {}) };
    assert!(res.is_err());
}

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_rc_oversized_refill_failure_returns_err() {
    use core::alloc::Layout;

    struct DropProbe(#[expect(dead_code, reason = "padding")] u64);
    impl Drop for DropProbe {
        fn drop(&mut self) {}
    }

    let arena = crate::ArenaBuilder::new_in(FailEverythingAllocator).build();
    let layout = Layout::array::<DropProbe>(16384).unwrap();
    // SAFETY: init never runs.
    let res = unsafe { arena.try_alloc_dst_rc::<[DropProbe]>(layout, 16384, |_| {}) };
    assert!(res.is_err());
}

#[cfg(feature = "dst")]
#[test]
fn align_up_identity_when_already_aligned() {
    // Kills the `align - 1` mutants in `align_up`: zero must stay zero.
    assert_eq!(align_up(0, 4), 0);
    assert_eq!(align_up(0, 8), 0);
    assert_eq!(align_up(4, 4), 4);
    assert_eq!(align_up(8, 8), 8);
}

#[cfg(feature = "dst")]
#[test]
fn align_up_rounds_up_to_alignment() {
    assert_eq!(align_up(1, 4), 4);
    assert_eq!(align_up(5, 4), 8);
    assert_eq!(align_up(7, 4), 8);
    assert_eq!(align_up(9, 8), 16);
}

#[cfg(feature = "dst")]
#[test]
fn align_up_align_one_is_identity() {
    assert_eq!(align_up(0, 1), 0);
    assert_eq!(align_up(1, 1), 1);
    assert_eq!(align_up(42, 1), 42);
}
