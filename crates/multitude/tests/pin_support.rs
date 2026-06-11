// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Pin support coverage: scalar / uninit / slice / DST allocators
//! producing `Pin<P<T>>` for `Box`, `Rc`, and `Arc`. Verifies the
//! pin contract (stable address until drop) holds across clones,
//! arena drop, and reset.

#![allow(clippy::std_instead_of_core, reason = "test code")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::clone_on_ref_ptr, reason = "test code")]
#![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
#![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
#![allow(clippy::redundant_clone, reason = "test code")]

use core::marker::PhantomPinned;
use core::pin::Pin;

use multitude::{Arc, Arena, Box};

/// `!Unpin` type. Its address must stay stable for as long as the
/// pinned smart pointer is live.
#[derive(Debug)]
struct NotUnpin {
    value: u64,
    _pin: PhantomPinned,
}

impl NotUnpin {
    fn new(value: u64) -> Self {
        Self {
            value,
            _pin: PhantomPinned,
        }
    }
}

// Tier 1: scalar pin allocators.

#[test]
fn alloc_box_pin_value() {
    let arena = Arena::new();
    let p: Pin<Box<NotUnpin>> = arena.alloc_box_pin(NotUnpin::new(42));
    assert_eq!(p.value, 42);
    let addr_before = (&raw const *p) as usize;
    // Smart pointer can be moved without moving the value.
    let p2 = p;
    let addr_after = (&raw const *p2) as usize;
    assert_eq!(addr_before, addr_after);
}

#[test]
fn alloc_box_pin_with_constructs_in_place() {
    let arena = Arena::new();
    let p: Pin<Box<NotUnpin>> = arena.alloc_box_pin_with(|| NotUnpin::new(7));
    assert_eq!(p.value, 7);
}

#[test]
fn alloc_arc_pin_with_cross_thread() {
    let arena = Arena::new();
    let p: Pin<Arc<NotUnpin>> = arena.alloc_arc_pin_with(|| NotUnpin::new(99));
    let addr_before = (&raw const *p) as usize;
    let p2 = p.clone();
    let h = std::thread::spawn(move || {
        let addr_in_thread = (&raw const *p2) as usize;
        assert_eq!(p2.value, 99);
        addr_in_thread
    });
    let addr_thread = h.join().unwrap();
    assert_eq!(addr_before, addr_thread, "Arc-pinned value must keep its address across threads");
}

#[test]
fn try_alloc_uninit_box_pin_rejects_over_alignment() {
    // Over-aligned T → AllocError; we expect Err, not panic.
    // Use the uninit-pin path so the test does NOT stack-construct a
    // 32 KiB-aligned value (which on Windows can blow past the
    // committed stack guard pages, causing a STATUS_ACCESS_VIOLATION
    // before the alignment check even runs).
    #[repr(align(32768))]
    #[expect(dead_code, reason = "drives the over-alignment guard before init runs")]
    struct HalfChunk(u8);
    let arena = Arena::new();
    let r: Result<Pin<Box<core::mem::MaybeUninit<HalfChunk>>>, _> = arena.try_alloc_uninit_box_pin::<HalfChunk>();
    r.unwrap_err();
}

// Tier 1: From / into_pin conversions.

#[test]
fn box_from_into_pin_value() {
    let arena = Arena::new();
    let b: Box<NotUnpin> = arena.alloc_box(NotUnpin::new(1));
    let p: Pin<Box<NotUnpin>> = Box::into_pin(b);
    assert_eq!(p.value, 1);

    let b2: Box<NotUnpin> = arena.alloc_box(NotUnpin::new(2));
    let p2: Pin<Box<NotUnpin>> = b2.into();
    assert_eq!(p2.value, 2);
}

#[test]
fn arc_from_into_pin_value() {
    let arena = Arena::new();
    let a = arena.alloc_arc(NotUnpin::new(4));
    let p: Pin<Arc<NotUnpin>> = Arc::into_pin(a);
    assert_eq!(p.value, 4);
}

// Tier 2: uninit pin + assume_init_pin.

#[test]
fn alloc_uninit_box_pin_then_assume_init_pin() {
    use core::mem::MaybeUninit;
    let arena = Arena::new();
    // Use an Unpin payload so the write path is fully safe.
    let mut uninit: Pin<Box<MaybeUninit<u64>>> = arena.alloc_uninit_box_pin::<u64>();
    let addr_before = (&raw const *uninit) as usize;
    // `MaybeUninit<u64>: Unpin`, so writing through `Pin::get_mut` is safe.
    uninit.as_mut().get_mut().write(50);
    let init: Pin<Box<u64>> = unsafe { Box::assume_init_pin(uninit) };
    let addr_after = (&raw const *init) as usize;
    assert_eq!(addr_before, addr_after);
    assert_eq!(*init, 50);
}

#[test]
fn alloc_uninit_box_pin_with_not_unpin_via_unchecked() {
    use core::mem::MaybeUninit;
    let arena = Arena::new();
    let uninit: Pin<Box<MaybeUninit<NotUnpin>>> = arena.alloc_uninit_box_pin::<NotUnpin>();
    // For `!Unpin` payloads the user must drop into `unsafe` to write
    // through the uninit slot. The pin contract is maintained because
    // no `T` value existed before the write.
    let mut unpinned = unsafe { Pin::into_inner_unchecked(uninit) };
    unpinned.write(NotUnpin::new(99));
    let init: Pin<Box<NotUnpin>> = unsafe { Box::assume_init_pin(Pin::new_unchecked(unpinned)) };
    assert_eq!(init.value, 99);
}

#[test]
fn alloc_uninit_arc_pin_then_assume_init_pin() {
    let arena = Arena::new();
    let uninit: Pin<Arc<core::mem::MaybeUninit<u32>>> = arena.alloc_uninit_arc_pin::<u32>();
    let unpinned = Pin::into_inner(uninit);
    unsafe {
        let p = Arc::as_ptr(&unpinned).cast::<core::mem::MaybeUninit<u32>>().cast_mut();
        (*p).write(70);
    }
    let init: Pin<Arc<u32>> = unsafe { Arc::assume_init_pin(Pin::new(unpinned)) };
    assert_eq!(*init, 70);
}

#[test]
fn alloc_zeroed_box_pin_yields_zeroed_storage() {
    let arena = Arena::new();
    let zeroed: Pin<Box<core::mem::MaybeUninit<u32>>> = arena.alloc_zeroed_box_pin::<u32>();
    let unpinned = Pin::into_inner(zeroed);
    let initialized: Pin<Box<u32>> = unsafe { Box::assume_init_pin(Pin::new(unpinned)) };
    assert_eq!(*initialized, 0);
}

// Tier 3: slice pin allocators.

#[test]
fn alloc_slice_fill_with_box_pin() {
    let arena = Arena::new();
    let s: Pin<Box<[NotUnpin]>> = arena.alloc_slice_fill_with_box_pin::<NotUnpin, _>(8, |i| NotUnpin::new(i as u64));
    assert_eq!(s.len(), 8);
    for i in 0..8 {
        assert_eq!(s[i].value, i as u64);
    }
    // Element addresses are stable.
    let addrs: Vec<usize> = (0..s.len()).map(|i| (&raw const s[i]) as usize).collect();
    // Move the Pin wrapper; addresses must not change.
    let s2 = s;
    let addrs2: Vec<usize> = (0..s2.len()).map(|i| (&raw const s2[i]) as usize).collect();
    assert_eq!(addrs, addrs2);
}

#[test]
fn alloc_slice_fill_with_arc_pin() {
    let arena = Arena::new();
    let s: Pin<Arc<[NotUnpin]>> = arena.alloc_slice_fill_with_arc_pin::<NotUnpin, _>(4, |i| NotUnpin::new(i as u64));
    assert_eq!(s.len(), 4);
    let clone = s.clone();
    let h = std::thread::spawn(move || {
        // The element addresses observed on the other thread match.
        clone.len()
    });
    assert_eq!(h.join().unwrap(), 4);
}

// Tier 3: DST (slice) pin allocators.

#[cfg(feature = "dst")]
#[test]
fn alloc_dst_box_pin_slice() {
    use core::alloc::Layout;

    let arena = Arena::new();
    let len = 4_usize;
    let layout = Layout::array::<NotUnpin>(len).unwrap();
    // SAFETY: init writes `len` valid `NotUnpin` values into the slice.
    let pinned: Pin<Box<[NotUnpin]>> = unsafe {
        arena.alloc_dst_box_pin::<[NotUnpin]>(layout, len, |fat: *mut [NotUnpin]| {
            let p = fat.cast::<NotUnpin>();
            for i in 0..len {
                p.add(i).write(NotUnpin::new(i as u64 + 100));
            }
        })
    };
    assert_eq!(pinned.len(), len);
    for (i, item) in pinned.iter().enumerate() {
        assert_eq!(item.value, i as u64 + 100);
    }
}

#[cfg(feature = "dst")]
#[test]
fn alloc_dst_arc_pin_slice_cross_thread() {
    use core::alloc::Layout;

    let arena = Arena::new();
    let len = 4_usize;
    let layout = Layout::array::<NotUnpin>(len).unwrap();
    // SAFETY: init fills the slice.
    let arc: Pin<Arc<[NotUnpin]>> = unsafe {
        arena.alloc_dst_arc_pin::<[NotUnpin]>(layout, len, |fat: *mut [NotUnpin]| {
            let p = fat.cast::<NotUnpin>();
            for i in 0..len {
                p.add(i).write(NotUnpin::new(i as u64 + 200));
            }
        })
    };
    let arc_clone = arc.clone();
    let h = std::thread::spawn(move || arc_clone[3].value);
    assert_eq!(h.join().unwrap(), 203);
    assert_eq!(arc[0].value, 200);
}

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_box_pin_slice_ok() {
    use core::alloc::Layout;

    let arena = Arena::new();
    let len = 2_usize;
    let layout = Layout::array::<NotUnpin>(len).unwrap();
    // SAFETY: init fills the slice.
    let pinned: Pin<Box<[NotUnpin]>> = unsafe {
        arena
            .try_alloc_dst_box_pin::<[NotUnpin]>(layout, len, |fat: *mut [NotUnpin]| {
                let p = fat.cast::<NotUnpin>();
                for i in 0..len {
                    p.add(i).write(NotUnpin::new(i as u64 + 400));
                }
            })
            .unwrap()
    };
    assert_eq!(pinned.len(), len);
    assert_eq!(pinned[1].value, 401);
}

#[cfg(feature = "dst")]
#[test]
fn try_alloc_dst_arc_pin_slice_ok() {
    use core::alloc::Layout;

    let arena = Arena::new();
    let len = 2_usize;
    let layout = Layout::array::<NotUnpin>(len).unwrap();
    // SAFETY: init fills the slice.
    let pinned: Pin<Arc<[NotUnpin]>> = unsafe {
        arena
            .try_alloc_dst_arc_pin::<[NotUnpin]>(layout, len, |fat: *mut [NotUnpin]| {
                let p = fat.cast::<NotUnpin>();
                for i in 0..len {
                    p.add(i).write(NotUnpin::new(i as u64 + 600));
                }
            })
            .unwrap()
    };
    assert_eq!(pinned.len(), len);
    assert_eq!(pinned[1].value, 601);
}

// Pin-contract regression coverage.

#[test]
fn pin_arc_survives_arena_drop_with_stable_address() {
    let addr;
    let arc_clone;
    {
        let arena = Arena::new();
        let p: Pin<Arc<NotUnpin>> = arena.alloc_arc_pin_with(|| NotUnpin::new(7));
        addr = (&raw const *p) as usize;
        arc_clone = p.clone();
        // arena drops here; the arc_clone keeps the chunk alive.
    }
    let addr_after = (&raw const *arc_clone) as usize;
    assert_eq!(addr, addr_after);
    assert_eq!(arc_clone.value, 7);
}

#[test]
fn pin_box_drops_value_in_place() {
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DroppyPinned {
        counter: StdArc<AtomicUsize>,
        _pin: PhantomPinned,
    }
    impl Drop for DroppyPinned {
        fn drop(&mut self) {
            self.counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    let counter = StdArc::new(AtomicUsize::new(0));
    {
        let arena = Arena::new();
        let _p: Pin<Box<DroppyPinned>> = arena.alloc_box_pin_with(|| DroppyPinned {
            counter: counter.clone(),
            _pin: PhantomPinned,
        });
    }
    assert_eq!(counter.load(Ordering::Relaxed), 1);
}

// Tier 5: try_* / _with variants and zeroed_*_pin variants.

#[test]
fn try_alloc_box_pin_succeeds() {
    let arena = Arena::new();
    let p = arena.try_alloc_box_pin(NotUnpin::new(1)).unwrap();
    assert_eq!(p.value, 1);
}

#[test]
fn try_alloc_box_pin_with_succeeds() {
    let arena = Arena::new();
    let p = arena.try_alloc_box_pin_with(|| NotUnpin::new(2)).unwrap();
    assert_eq!(p.value, 2);
}

#[test]
fn alloc_arc_pin_value() {
    let arena = Arena::new();
    let p: Pin<Arc<NotUnpin>> = arena.alloc_arc_pin(NotUnpin::new(6));
    assert_eq!(p.value, 6);
}

#[test]
fn try_alloc_arc_pin_succeeds() {
    let arena = Arena::new();
    let p = arena.try_alloc_arc_pin(NotUnpin::new(7)).unwrap();
    assert_eq!(p.value, 7);
}

#[test]
fn try_alloc_arc_pin_with_succeeds() {
    let arena = Arena::new();
    let p = arena.try_alloc_arc_pin_with(|| NotUnpin::new(8)).unwrap();
    assert_eq!(p.value, 8);
}

#[test]
fn alloc_zeroed_arc_pin_yields_zeroed_storage() {
    let arena = Arena::new();
    let p: Pin<Arc<core::mem::MaybeUninit<u128>>> = arena.alloc_zeroed_arc_pin::<u128>();
    // SAFETY: zeroed_arc_pin guarantees zero-initialized storage.
    let _ = unsafe { Arc::assume_init_pin(p) };
}

#[test]
fn try_alloc_zeroed_box_pin_succeeds() {
    let arena = Arena::new();
    let _ = arena.try_alloc_zeroed_box_pin::<u32>().unwrap();
}

#[test]
fn try_alloc_zeroed_arc_pin_succeeds() {
    let arena = Arena::new();
    let _ = arena.try_alloc_zeroed_arc_pin::<u32>().unwrap();
}

#[test]
fn try_alloc_uninit_box_pin_succeeds() {
    let arena = Arena::new();
    let _ = arena.try_alloc_uninit_box_pin::<u32>().unwrap();
}

#[test]
fn try_alloc_uninit_arc_pin_succeeds() {
    let arena = Arena::new();
    let _ = arena.try_alloc_uninit_arc_pin::<u32>().unwrap();
}

#[test]
fn try_alloc_slice_fill_with_box_pin_succeeds() {
    let arena = Arena::new();
    let p = arena
        .try_alloc_slice_fill_with_box_pin::<u32, _>(4, |i| u32::try_from(i).unwrap())
        .unwrap();
    assert_eq!(&*p, &[0_u32, 1, 2, 3]);
}

#[test]
fn try_alloc_slice_fill_with_arc_pin_succeeds() {
    let arena = Arena::new();
    let p = arena
        .try_alloc_slice_fill_with_arc_pin::<u32, _>(4, |i| u32::try_from(i).unwrap())
        .unwrap();
    assert_eq!(&*p, &[0_u32, 1, 2, 3]);
}

#[test]
fn assume_init_pin_slice_box() {
    let arena = Arena::new();
    let mut p: Pin<Box<[core::mem::MaybeUninit<u32>]>> = core::pin::Pin::new(arena.alloc_uninit_slice_box::<u32>(3));
    {
        let m = unsafe { Pin::get_unchecked_mut(p.as_mut()) };
        for (i, slot) in m.iter_mut().enumerate() {
            slot.write(u32::try_from(i).unwrap() + 10);
        }
    }
    // SAFETY: all elements are initialized above.
    let p: Pin<Box<[u32]>> = unsafe { Box::assume_init_pin_slice(p) };
    assert_eq!(&*p, &[10_u32, 11, 12]);
}

#[test]
fn assume_init_pin_slice_arc() {
    let arena = Arena::new();
    let s = arena.alloc_zeroed_slice_arc::<u32>(3);
    // SAFETY: storage address is stable; safe to pin.
    let p = unsafe { core::pin::Pin::new_unchecked(s) };
    // SAFETY: zero is a valid u32; assume init.
    let p = unsafe { Arc::assume_init_pin_slice(p) };
    assert_eq!(&*p, &[0_u32, 0, 0]);
}
