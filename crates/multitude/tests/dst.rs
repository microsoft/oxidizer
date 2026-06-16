// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    dead_code,
    unused_imports,
    clippy::unnecessary_safety_comment,
    reason = "residue of Rc-test removal: orphaned helpers/imports kept to preserve surrounding test bodies verbatim"
)]

//! Consolidated DST tests (Arc/Rc, Box, and panic-safety paths).

#![cfg(feature = "dst")]

mod common;

// === merged from tests/dst.rs ===
mod dst {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::assertions_on_result_states, reason = "tests prefer assert!")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    use core::sync::atomic::{AtomicUsize, Ordering};

    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn alloc_dst_arc_byte_slice() {
        let arena = Arena::new();
        let len = 5_usize;
        let layout = core::alloc::Layout::array::<u8>(len).unwrap();
        // SAFETY: layout matches [u8; 5]; metadata is len; init writes len bytes.
        let r = unsafe {
            arena.alloc_dst_arc::<[u8]>(layout, len, |fat: *mut [u8]| {
                let p = fat.cast::<u8>();
                for i in 0..len {
                    p.add(i).write((i + 200) as u8);
                }
            })
        };
        assert_eq!(r.len(), 5);
        for (i, byte) in r.iter().enumerate() {
            assert_eq!(*byte, (i + 200) as u8);
        }
    }

    #[test]
    fn try_alloc_dst_arc_succeeds() {
        let arena = Arena::new();
        let layout = core::alloc::Layout::array::<u8>(3).unwrap();
        // SAFETY: layout matches [u8; 3]; init writes 3 bytes.
        let r = unsafe {
            arena
                .try_alloc_dst_arc::<[u8]>(layout, 3_usize, |fat: *mut [u8]| {
                    let p = fat.cast::<u8>();
                    p.add(0).write(1);
                    p.add(1).write(2);
                    p.add(2).write(3);
                })
                .unwrap()
        };
        assert_eq!(&*r, &[1, 2, 3]);
    }

    #[test]
    fn alloc_dst_arc_outlives_arena() {
        let r = {
            let arena = Arena::new();
            let layout = core::alloc::Layout::array::<u32>(4).unwrap();
            // SAFETY: layout matches [u32; 4]; init writes 4 u32s.
            unsafe {
                arena.alloc_dst_arc::<[u32]>(layout, 4_usize, |fat: *mut [u32]| {
                    let p = fat.cast::<u32>();
                    for i in 0..4 {
                        p.add(i).write(11 * (i as u32 + 1));
                    }
                })
            }
        };
        assert_eq!(&*r, &[11, 22, 33, 44]);
    }

    #[test]
    fn alloc_dst_arc_send_across_threads() {
        let arena = Arena::new();
        let layout = core::alloc::Layout::array::<u32>(3).unwrap();
        // SAFETY: layout matches [u32; 3]; init writes 3 u32s.
        let r = unsafe {
            arena.alloc_dst_arc::<[u32]>(layout, 3_usize, |fat: *mut [u32]| {
                let p = fat.cast::<u32>();
                p.add(0).write(7);
                p.add(1).write(8);
                p.add(2).write(9);
            })
        };
        let r2 = r.clone();
        let h = std::thread::spawn(move || r2.iter().sum::<u32>());
        assert_eq!(h.join().unwrap(), 24);
        assert_eq!(&*r, &[7, 8, 9]);
    }

    #[test]
    fn try_alloc_dst_arc_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let half_chunk = 32 * 1024_usize;
        let layout = core::alloc::Layout::from_size_align(half_chunk, half_chunk).unwrap();
        let r = unsafe {
            arena.try_alloc_dst_arc::<[u8]>(layout, 0_usize, |_| {
                unreachable!("init must not be called when allocation fails");
            })
        };
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_dst_box_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let half_chunk = 32 * 1024_usize;
        let layout = core::alloc::Layout::from_size_align(half_chunk, half_chunk).unwrap();
        let r = unsafe {
            arena.try_alloc_dst_box::<[u8]>(layout, 0_usize, |_| {
                unreachable!("init must not be called when allocation fails");
            })
        };
        assert!(r.is_err());
    }
}

// === merged from tests/dst_box.rs ===
mod dst_box {
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::cast_possible_truncation, reason = "test indices are small and bounded")]
    use core::sync::atomic::{AtomicUsize, Ordering};

    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn alloc_dst_box_byte_slice() {
        let arena = Arena::new();
        let len = 10_usize;
        let layout = core::alloc::Layout::array::<u8>(len).unwrap();
        // SAFETY: layout matches [u8; 10]; init writes len bytes.
        let b = unsafe {
            arena.alloc_dst_box::<[u8]>(layout, len, |fat: *mut [u8]| {
                let p = fat.cast::<u8>();
                for i in 0..len {
                    p.add(i).write(i as u8);
                }
            })
        };
        assert_eq!(b.len(), 10);
        for (i, byte) in b.iter().enumerate() {
            assert_eq!(*byte, i as u8);
        }
    }

    #[test]
    fn try_alloc_dst_box_succeeds() {
        let arena = Arena::new();
        let layout = core::alloc::Layout::array::<u32>(2).unwrap();
        // SAFETY: layout matches [u32; 2]; init fully initializes.
        let b = unsafe {
            arena
                .try_alloc_dst_box::<[u32]>(layout, 2_usize, |fat: *mut [u32]| {
                    let p = fat.cast::<u32>();
                    p.add(0).write(11);
                    p.add(1).write(22);
                })
                .unwrap()
        };
        assert_eq!(&*b, &[11, 22]);
    }

    #[test]
    fn alloc_dst_box_runs_drop_immediately() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        struct Tracked(String);
        impl Drop for Tracked {
            fn drop(&mut self) {
                let _ = COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        COUNT.store(0, Ordering::SeqCst);
        let arena = Arena::new();
        let layout = core::alloc::Layout::array::<Tracked>(3).unwrap();
        // SAFETY: layout matches [Tracked; 3]; init writes 3 elements.
        let b = unsafe {
            arena.alloc_dst_box::<[Tracked]>(layout, 3_usize, |fat: *mut [Tracked]| {
                let p = fat.cast::<Tracked>();
                p.add(0).write(Tracked("a".to_string()));
                p.add(1).write(Tracked("b".to_string()));
                p.add(2).write(Tracked("c".to_string()));
            })
        };
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].0, "a");
        assert_eq!(b[2].0, "c");

        let before = COUNT.load(Ordering::SeqCst);
        drop(b);
        assert_eq!(
            COUNT.load(Ordering::SeqCst),
            before + 3,
            "drop_in_place([T;3]) must drop each element"
        );
        drop(arena);
        assert_eq!(COUNT.load(Ordering::SeqCst), before + 3, "no extra drops at arena teardown");
    }

    #[test]
    fn alloc_slice_copy_box_basic() {
        let arena = Arena::new();
        let b = arena.alloc_slice_copy_box([1_u32, 2, 3]);
        assert_eq!(&*b, &[1, 2, 3]);

        // Folded from_coverage_extras_dst::alloc_slice_copy_box_succeeds keeps the alternate payload assertion.
        let b: multitude::Box<[u8]> = arena.alloc_slice_copy_box([10_u8, 20, 30]);
        assert_eq!(&*b, &[10, 20, 30]);
    }

    #[test]
    fn alloc_slice_copy_box_mutable() {
        let arena = Arena::new();
        let mut b = arena.alloc_slice_copy_box([10_u32, 20, 30]);
        b[1] = 200;
        assert_eq!(&*b, &[10, 200, 30]);
    }

    #[test]
    fn try_alloc_slice_copy_box_works() {
        let arena = Arena::new();
        let b = arena.try_alloc_slice_copy_box([1_u8, 2, 3]).unwrap();
        assert_eq!(&*b, &[1, 2, 3]);
    }

    #[test]
    fn alloc_slice_clone_box_basic() {
        let arena = Arena::new();
        let originals = [
            std::string::String::from("a"),
            std::string::String::from("b"),
            std::string::String::from("c"),
        ];
        let b = arena.alloc_slice_clone_box(&originals);
        assert_eq!(b.len(), 3);
        assert_eq!(b[0], "a");
        assert_eq!(b[2], "c");

        // Folded from_coverage_extras_dst::alloc_slice_clone_box_succeeds keeps the alternate input case.
        let src = [
            std::string::String::from("x"),
            std::string::String::from("y"),
            std::string::String::from("z"),
        ];
        let b: multitude::Box<[String]> = arena.alloc_slice_clone_box(src);
        assert_eq!(b.len(), 3);
        assert_eq!(b[2], "z");
    }

    #[test]
    fn try_alloc_slice_clone_box_works() {
        let arena = Arena::new();
        let b = arena.try_alloc_slice_clone_box([100_u32, 200]).unwrap();
        assert_eq!(&*b, &[100, 200]);
    }

    #[test]
    fn alloc_slice_fill_with_box_basic() {
        let arena = Arena::new();
        let b: multitude::Box<[u64]> = arena.alloc_slice_fill_with_box(5, |i| (i as u64) * 10);
        assert_eq!(&*b, &[0, 10, 20, 30, 40]);

        // Folded from_coverage_extras_dst::alloc_slice_fill_with_box_succeeds keeps the shorter fill case.
        let b: multitude::Box<[u32]> = arena.alloc_slice_fill_with_box(4, |i| (i + 1) as u32);
        assert_eq!(&*b, &[1, 2, 3, 4]);
    }

    #[test]
    fn try_alloc_slice_fill_with_box_works() {
        let arena = Arena::new();
        let b: multitude::Box<[u32]> = arena.try_alloc_slice_fill_with_box(3, |i| u32::try_from(i + 100).unwrap()).unwrap();
        assert_eq!(&*b, &[100, 101, 102]);
    }

    #[test]
    fn alloc_slice_fill_iter_box_basic() {
        let arena = Arena::new();
        let b: multitude::Box<[i32]> = arena.alloc_slice_fill_iter_box([7_i32, 8, 9]);
        assert_eq!(&*b, &[7, 8, 9]);

        // Folded from_coverage_extras_dst::alloc_slice_fill_iter_box_succeeds keeps the range-based iterator case.
        let b: multitude::Box<[u8]> = arena.alloc_slice_fill_iter_box(0_u8..5);
        assert_eq!(&*b, &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn try_alloc_slice_fill_iter_box_works() {
        let arena = Arena::new();
        let b: multitude::Box<[u32]> = arena.try_alloc_slice_fill_iter_box([42_u32, 43, 44]).unwrap();
        assert_eq!(&*b, &[42, 43, 44]);
    }

    #[test]
    fn alloc_slice_fill_iter_box_empty() {
        let arena = Arena::new();
        let b: multitude::Box<[u32]> = arena.alloc_slice_fill_iter_box(core::iter::empty::<u32>());
        assert!(b.is_empty());
    }

    // Drop semantics: ArenaBox<[T]>::Drop must run T::drop on each element
    // IMMEDIATELY before the chunk reclaims.

    #[test]
    fn alloc_slice_clone_box_drops_elements_immediately() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        #[derive(Clone)]
        struct Tracked;
        impl Drop for Tracked {
            fn drop(&mut self) {
                let _ = COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        COUNT.store(0, Ordering::SeqCst);
        let arena = Arena::new();
        let originals = [Tracked, Tracked, Tracked];
        let b = arena.alloc_slice_clone_box(&originals);
        assert_eq!(b.len(), 3);
        let count_before = COUNT.load(Ordering::SeqCst);
        drop(b);
        let count_after = COUNT.load(Ordering::SeqCst);
        assert_eq!(count_after - count_before, 3, "drop_in_place([T;3]) must drop each element");

        // The arena drop must NOT run drops again (entry was unlinked).
        drop(originals);
        drop(arena);
        // After arena drop: count includes the originals drop (3 more), but no
        // double-drop of the box's elements.
        assert_eq!(COUNT.load(Ordering::SeqCst), count_after + 3);
    }

    #[test]
    fn alloc_slice_fill_with_box_drops_elements_immediately() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        struct Tracked;
        impl Drop for Tracked {
            fn drop(&mut self) {
                let _ = COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        COUNT.store(0, Ordering::SeqCst);
        let arena = Arena::new();
        let b: multitude::Box<[Tracked]> = arena.alloc_slice_fill_with_box(5, |_| Tracked);
        assert_eq!(b.len(), 5);
        let before = COUNT.load(Ordering::SeqCst);
        drop(b);
        assert_eq!(COUNT.load(Ordering::SeqCst), before + 5);
        drop(arena); // No double-drop.
        assert_eq!(COUNT.load(Ordering::SeqCst), before + 5);
    }

    #[test]
    fn alloc_slice_copy_box_no_drop_for_copy_types() {
        let arena = Arena::new();
        let b = arena.alloc_slice_copy_box([1_u8, 2, 3, 4, 5]);
        assert_eq!(b.len(), 5);
        drop(b);
        let b2 = arena.alloc_slice_copy_box([9_u8, 8, 7]);
        assert_eq!(&*b2, &[9, 8, 7]);
    }

    #[test]
    fn alloc_slice_fill_with_box_zero_len_works() {
        let arena = Arena::new();
        let b: multitude::Box<[u32]> = arena.alloc_slice_fill_with_box(0, |_| panic!("never called"));
        assert!(b.is_empty());
    }

    #[test]
    fn alloc_slice_fill_with_box_zst_with_drop() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        struct ZstDrop;
        impl Drop for ZstDrop {
            fn drop(&mut self) {
                let _ = COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        COUNT.store(0, Ordering::SeqCst);
        let arena = Arena::new();
        let b: multitude::Box<[ZstDrop]> = arena.alloc_slice_fill_with_box(7, |_| ZstDrop);
        drop(b);
        assert_eq!(COUNT.load(Ordering::SeqCst), 7);
    }

    #[test]
    fn alloc_slice_fill_with_box_panic_drops_initialized_prefix() {
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
        struct DropCounter;
        impl Drop for DropCounter {
            fn drop(&mut self) {
                let _ = DROP_COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        DROP_COUNT.store(0, Ordering::SeqCst);
        let arena = Arena::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Panic at index 3, after producing 3 DropCounters.
            let _b: multitude::Box<[DropCounter]> = arena.alloc_slice_fill_with_box(10, |i| {
                assert!(i != 3, "intentional");
                DropCounter
            });
        }));
        assert!(result.is_err());
        assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 3);
        let b: multitude::Box<[u32]> = arena.alloc_slice_fill_with_box(2, |i| u32::try_from(i).unwrap());
        assert_eq!(&*b, &[0, 1]);
    }

    #[test]
    #[should_panic(expected = "caller violated ExactSizeIterator contract")]
    fn alloc_slice_fill_iter_box_panics_on_short_iter() {
        struct Liar(usize);
        impl Iterator for Liar {
            type Item = u32;
            fn next(&mut self) -> Option<u32> {
                None
            }
        }
        impl ExactSizeIterator for Liar {
            fn len(&self) -> usize {
                self.0
            }
        }
        let arena = Arena::new();
        let _b: multitude::Box<[u32]> = arena.alloc_slice_fill_iter_box(Liar(2));
    }

    #[test]
    fn alloc_slice_box_high_alignment_drop_locates_entry_correctly() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        #[repr(align(32))]
        struct A32(#[expect(dead_code, reason = "field present only to give the type a non-zero size")] u8);
        impl Drop for A32 {
            fn drop(&mut self) {
                let _ = COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        COUNT.store(0, Ordering::SeqCst);
        let arena = Arena::new();
        let _decoy: &mut u8 = arena.alloc(0_u8);
        let b: multitude::Box<[A32]> = arena.alloc_slice_fill_with_box(4, |_| A32(0));
        assert_eq!(b.len(), 4);
        let before = COUNT.load(Ordering::SeqCst);
        drop(b);
        assert_eq!(COUNT.load(Ordering::SeqCst), before + 4);
    }

    /// Regression: a slice DST with `len > u16::MAX` and `T: Drop` must be
    /// rejected at allocation time (returns `AllocError`) so that a future
    /// `Box::<[T]>::into_rc()` call cannot find itself with no drop entry
    /// to retarget. Matches the non-DST slice-alloc paths which use the
    /// same `entry_size != 0 && len > u16::MAX` guard.
    #[test]
    fn try_alloc_dst_box_rejects_drop_slice_with_overflowing_len() {
        struct DropCounter(std::sync::Arc<AtomicUsize>);
        impl Drop for DropCounter {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let arena = Arena::new();
        let n: usize = (u16::MAX as usize) + 1;
        // Layout::array fits since u16::MAX+1 elements at small size are well under isize::MAX.
        let Ok(layout) = core::alloc::Layout::array::<DropCounter>(n) else {
            // Allocator wouldn't even build the layout; the test isn't meaningful.
            return;
        };

        // SAFETY: init would write all `n` elements; we never reach that point
        // because the allocation is rejected up front by the new guard.
        let result = unsafe {
            arena.try_alloc_dst_box::<[DropCounter]>(layout, n, |_fat: *mut [DropCounter]| {
                unreachable!("alloc must be rejected before init runs");
            })
        };
        assert!(result.is_err(), "DST slice with len > u16::MAX and T: Drop must be rejected");
    }
}

// === merged from tests/dst_panic_safety.rs ===
mod dst_panic_safety {
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[test]
    fn dst_arc_init_panic_then_evict_is_sound() {
        let arena = Arena::new();
        let _a1 = arena.alloc_arc(String::from("a"));
        let _a2 = arena.alloc_arc(String::from("b"));

        let layout = core::alloc::Layout::array::<u32>(4).unwrap();
        let result = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: panic before any write — the Arc is never observed.
            unsafe {
                arena.alloc_dst_arc::<[u32]>(layout, 4_usize, |_fat: *mut [u32]| {
                    panic!("planned panic in DST init");
                });
            }
        }));
        assert!(result.is_err());

        // 256 drop-typed strings still retire multiple chunks, so eviction
        // replays the same drop-list state this test cares about.
        for _ in 0..256 {
            let _ = arena.alloc_arc(String::from("xxxxxxxxxx"));
        }
        drop(arena);
    }
}

// === relocated from coverage_extras.rs (dst-gated tests) ===
mod from_coverage_extras_dst {
    #![allow(clippy::items_after_statements, reason = "relocated tests put inner types near use")]
    #![allow(clippy::clone_on_ref_ptr, reason = "relocated tests use .clone() on Arc/Rc")]
    #![allow(dead_code, reason = "relocated helpers retain fields for layout")]
    #![allow(
        unfulfilled_lint_expectations,
        reason = "relocated #[expect] may be fulfilled at file or feature level"
    )]
    #![allow(
        clippy::undocumented_unsafe_blocks,
        reason = "relocated test bodies preserve original safety reasoning"
    )]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "relocated tests group related unsafe ops")]
    #![allow(clippy::cast_possible_truncation, reason = "relocated tests use bounded values")]
    #![allow(clippy::cast_sign_loss, reason = "relocated tests use non-negative values")]
    #![allow(clippy::empty_drop, reason = "relocated tests use empty Drop impls to mark dropability")]
    #![allow(clippy::assertions_on_result_states, reason = "relocated tests deliberately assert error returns")]
    #![allow(clippy::empty_line_after_doc_comments, reason = "relocated test doc-comments")]
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::panic::catch_unwind;

    use crate::common::FailingAllocator as _FA;
    fn fail_arena() -> Arena<_FA> {
        Arena::new_in(_FA::new(0))
    }
    fn expect_panic<F: FnOnce() + std::panic::UnwindSafe>(f: F) {
        let r = catch_unwind(f);
        assert!(r.is_err(), "expected panic");
    }

    use std::panic::AssertUnwindSafe;

    use multitude::{Arena, ArenaBuilder, Box};
    #[expect(dead_code, reason = "helper used by some relocated tests")]
    struct OneByteDrop(u8);
    impl Drop for OneByteDrop {
        fn drop(&mut self) {}
    }
    #[expect(unused_imports, reason = "relocated tests may reference common helpers")]
    use crate::common::{self, FailingAllocator, SendFailingAllocator};

    #[test]
    fn alloc_box_oversized_drop_type_uses_has_drop_layout() {
        // Same path as above, but via the box family (which also routes
        // through `oversized_layout(_, true)` for Drop-needing types).
        static DROPPED: AtomicUsize = AtomicUsize::new(0);
        struct BigDrop {
            _bytes: [u8; 4096],
        }
        impl Drop for BigDrop {
            fn drop(&mut self) {
                let _ = DROPPED.fetch_add(1, Ordering::SeqCst);
            }
        }
        DROPPED.store(0, Ordering::SeqCst);

        let arena: Arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        {
            let _b: Box<BigDrop> = arena.alloc_box(BigDrop { _bytes: [0; 4096] });
        }
        assert_eq!(DROPPED.load(Ordering::SeqCst), 1);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_copy_box_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_slice_copy_box([0_u8; 4]);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_clone_box_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_slice_clone_box([1_u32, 2]);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_fill_with_box_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_slice_fill_with_box::<u32, _>(4, |i| i as u32);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_fill_iter_box_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_slice_fill_iter_box([1_u32, 2, 3]);
    }

    #[test]
    fn panic_alloc_uninit_slice_box() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_uninit_slice_box::<u32>(4);
        });
    }

    #[test]
    fn panic_alloc_zeroed_slice_box() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_zeroed_slice_box::<u32>(4);
        });
    }

    #[test]
    fn try_alloc_uninit_slice_box_err() {
        let a = fail_arena();
        a.try_alloc_uninit_slice_box::<u32>(4).unwrap_err();
    }

    #[test]
    fn try_alloc_zeroed_slice_box_err() {
        let a = fail_arena();
        a.try_alloc_zeroed_slice_box::<u32>(4).unwrap_err();
    }

    #[test]
    fn try_alloc_dst_arc_accepts_sized_metadata() {
        let arena = Arena::new();
        let layout = core::alloc::Layout::new::<u32>();
        // SAFETY: init writes a valid `u32` through the supplied pointer.
        let result = unsafe { arena.try_alloc_dst_arc::<u32>(layout, (), |p| p.write(42_u32)) };
        let arc = result.expect("sized DST allocation through try_alloc_dst_arc should succeed");
        assert_eq!(*arc, 42);
    }

    // ---- arena.rs: refill failure paths inside DST reservation loops. ----
    //
    // These hit the `refill_local(...)?` / `refill_shared(...)?` Err arms in
    // `allocate_shared_layout` (3112), `try_reserve_dst_local_with_entry` (3227)
    // and `try_reserve_dst_shared_with_entry` (3311).
    #[test]
    fn try_alloc_dst_arc_refill_failure_propagates() {
        // Small chunk + failing allocator: first chunk acquire succeeds, the
        // refill needed for a second allocation that doesn't fit fails.
        let alloc = common::SendFailingAllocator::new(1);
        let arena = Arena::builder_in(alloc).max_normal_alloc(4096).try_build().unwrap();
        let layout = core::alloc::Layout::array::<u8>(2048).unwrap();
        let mut errs = 0;
        for _ in 0..16 {
            // SAFETY: init only invoked if allocation succeeds.
            let r = unsafe { arena.try_alloc_dst_arc::<[u8]>(layout, 2048, |_| {}) };
            if r.is_err() {
                errs += 1;
            }
        }
        assert!(errs >= 1, "expected at least one refill failure");
    }

    // Drop-needing DST shared path: refill failure inside `try_reserve_dst_shared_with_entry`.
    #[test]
    fn try_alloc_dst_arc_drop_refill_failure_propagates() {
        let alloc = common::SendFailingAllocator::new(1);
        let arena = Arena::builder_in(alloc).max_normal_alloc(4096).try_build().unwrap();
        // String isn't Send via Arc, but `Vec<u8>` is.
        let layout = core::alloc::Layout::array::<Vec<u8>>(64).unwrap();
        let mut errs = 0;
        for _ in 0..16 {
            // SAFETY: init only invoked if allocation succeeds.
            let r = unsafe {
                arena.try_alloc_dst_arc::<[Vec<u8>]>(layout, 64, |p| {
                    for i in 0..64 {
                        core::ptr::write(p.cast::<Vec<u8>>().add(i), Vec::new());
                    }
                })
            };
            if r.is_err() {
                errs += 1;
            }
        }
        assert!(errs >= 1, "expected at least one refill failure");
    }
}

// === relocated from mutants_extras.rs (dst-gated tests) ===
mod from_mutants_extras_dst {
    #![allow(clippy::items_after_statements, reason = "relocated tests put inner types near use")]
    #![allow(clippy::clone_on_ref_ptr, reason = "relocated tests use .clone() on Arc/Rc")]
    #![allow(dead_code, reason = "relocated helpers retain fields for layout")]
    #![allow(
        unfulfilled_lint_expectations,
        reason = "relocated #[expect] may be fulfilled at file or feature level"
    )]
    #![allow(
        clippy::undocumented_unsafe_blocks,
        reason = "relocated test bodies preserve original safety reasoning"
    )]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "relocated tests group related unsafe ops")]
    #![allow(clippy::cast_possible_truncation, reason = "relocated tests use bounded values")]
    #![allow(clippy::cast_sign_loss, reason = "relocated tests use non-negative values")]
    #![allow(clippy::empty_drop, reason = "relocated tests use empty Drop impls to mark dropability")]
    #![allow(clippy::assertions_on_result_states, reason = "relocated tests deliberately assert error returns")]
    #![allow(clippy::empty_line_after_doc_comments, reason = "relocated test doc-comments")]

    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arena, Box as ArenaBox};

    #[expect(dead_code, reason = "helper used by some relocated tests")]
    struct OneByteDrop(u8);
    impl Drop for OneByteDrop {
        fn drop(&mut self) {}
    }
    #[expect(unused_imports, reason = "relocated tests may reference common helpers")]
    use crate::common::{self, FailingAllocator, SendFailingAllocator};

    /// Kills `arena.rs:3197:55 - -> /` in `allocate_shared_layout`.
    /// Line 3197 computes `aligned_offset = aligned_addr - data_addr`.
    /// Mutated `/`: when `aligned_addr == data_addr` (the common case for
    /// pre-aligned bump cursors), `-` yields 0 but `/` yields 1, so the
    /// returned pointer becomes misaligned by 1 byte. This test exercises
    /// the path via the public DST-Arc API for a `[u128]` (align=16, non-
    /// Drop) and asserts the returned payload is properly aligned.
    #[test]
    fn allocate_shared_layout_high_align_offset_zero_preserved() {
        use core::alloc::Layout;
        let arena = multitude::Arena::new();
        // Repeat many times so we hit both fresh-chunk (offset 0) and
        // mid-chunk paths; misalignment of even one would fail an assert.
        for _ in 0..64 {
            let layout = Layout::array::<u128>(32).unwrap();
            let metadata: usize = 32;
            #[expect(
                clippy::multiple_unsafe_ops_per_block,
                reason = "single logical unsafe operation: the alloc_dst_arc DST initialization"
            )]
            // SAFETY: `init` writes 32 valid `u128`s through the fat pointer;
            // layout matches `[u128; 32]`; metadata is the slice length.
            let arc: multitude::Arc<[u128]> = unsafe {
                arena.alloc_dst_arc::<[u128]>(layout, metadata, |p: *mut [u128]| {
                    let base = p.cast::<u128>();
                    for i in 0_u128..32 {
                        core::ptr::write(base.add(i as usize), i);
                    }
                })
            };
            let raw_addr = arc.as_ptr().cast::<u128>() as usize;
            assert_eq!(raw_addr % 16, 0, "payload must be aligned to u128");
            assert_eq!(arc.len(), 32);
            for i in 0_u128..32 {
                assert_eq!(arc[i as usize], i);
            }
        }
    }

    #[test]
    fn into_box_empty_drop_type_takes_copy_path() {
        struct D;
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let arena = Arena::new();
        let v: ArenaVec<'_, D> = arena.alloc_vec_with_capacity(4);
        assert_eq!(v.len(), 0);
        let b: ArenaBox<[D], _> = v.into_boxed_slice();
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn into_box_with_full_capacity_for_drop_type_no_reclaim() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, OneByteDrop> = arena.alloc_vec_with_capacity(4);
        for i in 0..4 {
            v.push(OneByteDrop(i));
        }
        assert_eq!(v.len(), v.capacity());
        let b: ArenaBox<[OneByteDrop], _> = v.into_boxed_slice();
        assert_eq!(b.len(), 4);
    }

    #[test]
    fn into_box_with_full_capacity_for_non_drop_type_no_reclaim() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(4);
        for i in 0..4 {
            v.push(i);
        }
        assert_eq!(v.len(), v.capacity());
        let b: ArenaBox<[u32], _> = v.into_boxed_slice();
        assert_eq!(b.len(), 4);
    }

    #[test]
    fn into_box_advances_consumed_index() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicU32, Ordering};

        struct D {
            idx: u32,
            seen: StdArc<AtomicU32>,
        }
        impl Drop for D {
            fn drop(&mut self) {
                self.seen.fetch_or(1_u32 << (self.idx % 32), Ordering::Relaxed);
            }
        }

        let seen = StdArc::new(AtomicU32::new(0));
        {
            let arena = Arena::new();
            // 16-byte D; default max_normal_alloc = 16 KiB. with_capacity(1100)
            // requests 17.6 KiB > max_normal_alloc → buffer goes to oversized
            // chunk, install fails, into_box falls back to the copy path.
            let mut v: ArenaVec<'_, D> = arena.alloc_vec_with_capacity(1100);
            for i in 0..1100_u32 {
                v.push(D {
                    idx: i,
                    seen: seen.clone(),
                });
            }
            let b: ArenaBox<[D], _> = v.into_boxed_slice();
            for (i, d) in b.iter().enumerate() {
                assert_eq!(d.idx, i as u32, "element {i} should have idx {i}");
            }
            drop(b);
        }
        assert_eq!(seen.load(Ordering::Relaxed), u32::MAX);
    }

    #[test]
    fn dst_arc_rejects_excessive_alignment_via_layout() {
        let arena = Arena::new();
        let layout = core::alloc::Layout::from_size_align(64, 32 * 1024).unwrap();
        // SAFETY: validation runs before any user-visible state mutation.
        let result = unsafe { arena.try_alloc_dst_arc::<[u8]>(layout, 64, |_| {}) };
        result.unwrap_err();
    }
}
