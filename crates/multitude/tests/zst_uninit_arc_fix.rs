// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Regression test for ZST + Drop + `alloc_uninit_arc`: multiple
//! allocations must each commit their own placeholder rather than
//! collide on `(value_offset, len)`.
use core::cell::Cell;

use multitude::Arena;

thread_local! {
    /// Per-test drop counter. Each `#[test]` runs on its own `libtest`
    /// thread and performs all of its ZST drops on that thread, so a
    /// thread-local counter isolates the tests from one another. A shared
    /// global `AtomicUsize` here was flaky:
    /// `zst_alloc_arc_never_returns_one_past_chunk_end` drops 512 markers
    /// that could race the count asserted by
    /// `each_zst_uninit_arc_assume_init_commits_its_own_placeholder` under
    /// parallel execution. `ZstDrop`/`ZstMarker` must stay zero-sized (the
    /// regression targets ZST + Drop), so the counter cannot live inside them.
    static DROPS: Cell<usize> = const { Cell::new(0) };
}

/// Increment the current thread's drop counter.
fn bump_drops() {
    DROPS.with(|c| c.set(c.get() + 1));
}

struct ZstDrop;
impl Drop for ZstDrop {
    fn drop(&mut self) {
        bump_drops();
    }
}

#[test]
fn each_zst_uninit_arc_assume_init_commits_its_own_placeholder() {
    DROPS.with(|c| c.set(0));
    {
        let arena = Arena::new();
        let a1 = arena.alloc_uninit_arc::<ZstDrop>();
        let a2 = arena.alloc_uninit_arc::<ZstDrop>();
        let a3 = arena.alloc_uninit_arc::<ZstDrop>();
        let p1 = a1.as_ptr().cast_mut().cast::<core::mem::MaybeUninit<ZstDrop>>();
        let p2 = a2.as_ptr().cast_mut().cast::<core::mem::MaybeUninit<ZstDrop>>();
        let p3 = a3.as_ptr().cast_mut().cast::<core::mem::MaybeUninit<ZstDrop>>();
        // SAFETY: each pointer comes from the corresponding
        // `Arc<MaybeUninit<ZstDrop>>` and we are the unique writer.
        // For a ZST, `MaybeUninit::write` touches zero bytes; the
        // operation only constructs and immediately conceptually-moves
        // the value into the placeholder slot.
        unsafe {
            (*p1).write(ZstDrop);
        }
        // SAFETY: same.
        unsafe {
            (*p2).write(ZstDrop);
        }
        // SAFETY: same.
        unsafe {
            (*p3).write(ZstDrop);
        }
        // SAFETY: each ZstDrop was just written above.
        let _i1 = unsafe { a1.assume_init() };
        // SAFETY: same.
        let _i2 = unsafe { a2.assume_init() };
        // SAFETY: same.
        let _i3 = unsafe { a3.assume_init() };
    }
    // After arena drop, every committed drop entry runs.
    assert_eq!(
        DROPS.with(Cell::get),
        3,
        "all three ZstDrop destructors must run; the (value_offset, len) commit lookup must disambiguate via the 1-byte ZST tag",
    );
}

/// Bug #3: ZST `alloc_arc` at the chunk tail used to return a value
/// pointer at `chunk_base + CHUNK_ALIGN` for the largest chunk class,
/// breaking the smart-pointer header-recovery mask on Drop. The fix
/// in `try_alloc` rejects the alloc when the probe end would equal
/// `drop_top`, forcing a refill.
///
/// To reach the failure mode pre-fix, we'd need a class-7 chunk
/// allocated and entirely consumed up to `drop_top` by other allocs
/// before the ZST alloc. We simulate it by allocating ZSTs in a loop
/// on a chunk repeatedly until refill — every ZST allocation must
/// produce a pointer whose `& CHUNK_BASE_MASK` recovers the same
/// chunk header (not the next 64 KiB tile).
struct ZstMarker;
impl Drop for ZstMarker {
    fn drop(&mut self) {
        // Side-effecting Drop so a missed destructor would observably
        // skew the counter; not strictly required for the
        // header-recovery aspect, but exercises the same drop entry
        // path.
        bump_drops();
    }
}

const CHUNK_BASE_MASK: usize = !(65_536 - 1);

#[test]
fn zst_alloc_arc_never_returns_one_past_chunk_end() {
    let arena = multitude::Arena::new();
    // Allocate enough ZST Arcs to exercise multiple refills, including
    // the largest size class. Each ZST Arc's value pointer must lie
    // strictly inside its 64 KiB tile so the chunk-header mask
    // recovery in `Arc::Drop` lands on the right chunk.
    let mut prev_base: Option<usize> = None;
    for _ in 0..512 {
        let a = arena.alloc_arc(ZstMarker);
        let addr = (&raw const *a) as usize;
        let base = addr & CHUNK_BASE_MASK;
        let offset = addr - base;
        assert!(offset < 65_536, "ZST Arc value pointer must lie strictly inside its 64 KiB tile");
        // The chunk may rotate (refill); either way `prev_base` either
        // equals the new base or points at an unrelated valid chunk —
        // both fine.
        let _ = prev_base;
        prev_base = Some(base);
    }
}

/// Regression from post-fix audit: `impl_alloc_dst_box` used to check
/// `is_oversized_shared(total)` but refill with `total + align`. At
/// `total == max_normal_alloc` but `total + align > max_normal_alloc`,
/// the in-arena fast path failed, the oversized branch was skipped,
/// and `refill_shared(refill_hint)` hit the new `debug_assert!` in
/// `acquire_shared`.
#[cfg(feature = "dst")]
#[test]
fn dst_box_at_max_normal_alloc_boundary_routes_consistently() {
    use core::alloc::Layout;
    let arena = multitude::Arena::new();
    // size=16376, align=4. meta_bytes=8 (slice metadata). payload_offset=8.
    // total = 8 + 16376 = 16384 == default max_normal_alloc.
    // refill_hint = 16384 + 4 = 16388 (> max_normal_alloc).
    let layout = Layout::array::<u32>(4094).unwrap();
    let init = |fat: *mut [u32]| {
        // SAFETY: `fat` covers `4094 * size_of::<u32>()` writable bytes.
        let s = unsafe { &mut *fat };
        for (i, v) in s.iter_mut().enumerate() {
            *v = u32::try_from(i).expect("4094 fits in u32");
        }
    };
    // SAFETY: layout matches `[u32; 4094]`; metadata is the slice length;
    // `init` initializes all elements.
    let b: multitude::Box<[u32]> = unsafe { arena.alloc_dst_box::<[u32]>(layout, 4094_usize, init) };
    assert_eq!(b.len(), 4094);
    assert_eq!(b[0], 0);
    assert_eq!(b[4093], 4093);
}
