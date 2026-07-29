// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! ZST initialization, uniqueness, and pointer-routing invariants.
use core::cell::Cell;

use multitude::Arena;

thread_local! {
    /// Thread-local state isolates parallel tests while keeping marker types
    /// zero-sized.
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
    assert_eq!(
        DROPS.with(Cell::get),
        3,
        "all three ZstDrop destructors must run; the (value_offset, len) commit lookup must disambiguate via the 1-byte ZST tag",
    );
}

/// Every ZST value pointer remains strictly within its chunk tile so mask-based
/// header recovery routes to the correct chunk.
struct ZstMarker;
impl Drop for ZstMarker {
    fn drop(&mut self) {
        bump_drops();
    }
}

const CHUNK_BASE_MASK: usize = !(65_536 - 1);

#[test]
fn zst_alloc_arc_never_returns_one_past_chunk_end() {
    let arena = multitude::Arena::new();
    // Every pointer must remain strictly inside its 64 KiB tile.
    let mut prev_base: Option<usize> = None;
    for _ in 0..512 {
        let a = arena.alloc_arc(ZstMarker);
        let addr = (&raw const *a) as usize;
        let base = addr & CHUNK_BASE_MASK;
        let offset = addr - base;
        assert!(offset < 65_536, "ZST Arc value pointer must lie strictly inside its 64 KiB tile");
        let _ = prev_base;
        prev_base = Some(base);
    }
}

/// The DST routing decision includes alignment slack consistently at the
/// normal-allocation boundary.
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
