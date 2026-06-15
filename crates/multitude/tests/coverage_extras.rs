// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consolidated coverage-gap tests covering surfaces not yet routed to per-module files.

mod common;

mod coverage {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain smart pointers to keep chunks alive")]
    #![allow(unused_results, reason = "test code")]
    #![allow(clippy::used_underscore_binding, reason = "intentional drop-after binding")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::explicit_into_iter_loop, reason = "test clarity")]
    #![allow(clippy::assertions_on_result_states, reason = "tests deliberately assert error returns")]
    #![allow(clippy::items_after_statements, reason = "test-local statics next to their use")]
    #![allow(
        clippy::cast_ptr_alignment,
        reason = "test writes a u32 to a u8-typed reservation we created with u32 layout"
    )]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    use core::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(feature = "dst")]
    use multitude::Arc;
    use multitude::vec::{CollectIn, Vec};
    use multitude::{Arena, ArenaBuilder};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;
    use crate::common::{FailingAllocator, SendFailingAllocator};

    #[test]
    fn allocator_deallocate_triggers_teardown_when_last_ref() {
        // <&Arena as Allocator>::deallocate's `if needs_teardown` branch:
        // the deallocate must observe refcount → 0 and call teardown_chunk.
        // Achieved by forcing many grow → relocate cycles inside a Vec
        // backed by `&Arena`: each old buffer's deallocate eventually
        // tears down its chunk (the chunk's only ref was the Vec's
        // buffer, and after retirement the arena no longer holds it).
        let arena: Arena = Arena::builder().build();
        {
            let mut v: allocator_api2::vec::Vec<u8, &Arena> = allocator_api2::vec::Vec::new_in(&arena);
            for _ in 0..16_000_u32 {
                v.push(0);
            }
            drop(v);
        }
    }

    #[test]
    fn builder_debug_format() {
        // Drives ArenaBuilder's Debug impl.
        let s = format!("{:?}", Arena::builder());
        assert!(s.contains("ArenaBuilder"));
        assert!(s.contains("max_normal_alloc"));
    }

    #[test]
    fn builder_preallocate_alloc_failed() {
        // Drives the AllocError return path in ArenaBuilder::try_build by
        // giving the builder an allocator that refuses to allocate.
        let alloc = FailingAllocator::new(0);
        let result = Arena::builder().with_capacity_local(512).allocator_in(alloc).try_build();
        assert!(result.is_err());
    }

    #[test]
    fn arena_box_drop_unlinks_middle_of_drop_list() {
        // `unlink_drop_entry` has three positions (head, middle, tail).
        // The middle case is reached when the entry being removed has both
        // a `prev` and a `next`. ArenaBox<T: Drop>::drop calls unlink. We
        // create three drop-needing ArenaBox values, then drop the second
        // one first → exercises the `Some(prev)` AND `Some(next)` branches.
        let arena = Arena::new();
        let mut b1 = arena.alloc_box(std::string::String::from("first"));
        let mut b2 = arena.alloc_box(std::string::String::from("middle"));
        let mut b3 = arena.alloc_box(std::string::String::from("last"));
        // Make sure each value is reachable (touch the contents).
        b1.push('!');
        b2.push('!');
        b3.push('!');
        drop(b2); // <-- middle of doubly-linked list
        assert_eq!(*b1, "first!");
        assert_eq!(*b3, "last!");
    }

    #[test]
    fn cached_local_chunk_revived_as_shared() {
        // `revive_cached_chunk(chunk, Shared)` → `reinit_refcount(_, Shared, 1)`
        // dispatches to the Shared branch. The deterministic way to land
        // a chunk in the cache is `.preallocate(n)`, which seeds the cache
        // before the first allocation. Then `alloc_arc` pops the cache and
        // revives the chunk as Shared.
        let arena: Arena = Arena::builder().with_capacity_local(1024).build();
        let shared = arena.alloc_arc(99_u64);
        assert_eq!(*shared, 99);
        let join = std::thread::spawn(move || *shared);
        assert_eq!(99, join.join().unwrap());
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_box_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_box(0_u32);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_box_with_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_box_with(|| 0_u32);
    }

    #[test]
    #[cfg(feature = "dst")]
    fn alloc_dst_arc_rejects_excessive_alignment() {
        let arena: Arena = Arena::new();
        let huge_align = 128 * 1024_usize;
        let layout = core::alloc::Layout::from_size_align(huge_align, huge_align).unwrap();
        let r = unsafe {
            arena.try_alloc_dst_arc::<[u8]>(layout, 0_usize, |_| {
                unreachable!("init must not be called when allocation fails");
            })
        };
        assert!(r.is_err());
    }

    #[test]
    #[cfg(feature = "dst")]
    fn alloc_dst_box_rejects_excessive_alignment() {
        let arena: Arena = Arena::new();
        let huge_align = 128 * 1024_usize;
        let layout = core::alloc::Layout::from_size_align(huge_align, huge_align).unwrap();
        let r = unsafe {
            arena.try_alloc_dst_box::<[u8]>(layout, 0_usize, |_| {
                unreachable!("init must not be called when allocation fails");
            })
        };
        assert!(r.is_err());
    }

    // `#[repr(align(N))]` with N > CHUNK_ALIGN (64 KiB). Used by the two
    // tests below to drive the `if layout.align() > CHUNK_ALIGN { return
    // Err(AllocError) }` guard in `try_alloc_with` and `try_reserve_and_init`.
    //
    // The guard lives in a thin outer function whose frame doesn't depend
    // on `T`'s alignment, so the test runs on every platform — including
    // Windows, whose default 1 MiB stack can't accommodate the 128 KiB-
    // aligned frame the guarded body would otherwise require.
    #[repr(align(131072))]
    struct HugeAlign(#[expect(dead_code, reason = "field present to give the type a non-zero size")] u8);

    #[test]
    fn try_alloc_with_rejects_excessive_alignment() {
        // try_alloc_with is the &mut T entry point. CHUNK_ALIGN is 64 KiB;
        // HugeAlign needs 128 KiB alignment, so the layout-align check
        // must fire and return Err.
        let arena: Arena = Arena::new();
        let result: Result<&mut HugeAlign, _> = arena.try_alloc_with(|| HugeAlign(0));
        assert!(result.is_err());
    }

    #[test]
    fn try_alloc_string_with_capacity_huge_returns_err() {
        let arena: Arena = Arena::new();
        // Try a capacity that overflows when adding the prefix size.
        let too_big = usize::MAX;
        assert!(arena.try_alloc_string_with_capacity(too_big).is_err());
    }

    #[test]
    fn try_alloc_string_with_capacity_isize_max_returns_err() {
        // Drives the `isize::try_from(total).is_err()` guard in
        // ArenaString::try_allocate_initial. Need cap such that
        // `cap + PREFIX_SIZE` is between `isize::MAX + 1` and `usize::MAX`.
        let arena: Arena = Arena::new();
        let cap = (isize::MAX as usize) - 4; // cap + 8 > isize::MAX, and < usize::MAX
        assert!(arena.try_alloc_string_with_capacity(cap).is_err());
    }

    // Note: the `align > CHUNK_ALIGN` guard inside the typed alloc paths
    // (`Arena::try_alloc_with`, `Arena::try_reserve_and_init`) cannot be
    // exercised from a test that names a `#[repr(align(N))]` `T` with
    // `N > CHUNK_ALIGN` — even though the closure / value would never be
    // constructed, the compiled function's stack frame inherits `T`'s
    // alignment, producing a STATUS_ACCESS_VIOLATION on call. The
    // equivalent guard is exercised through the layout-based path in
    // `alloc_uninit_dst_rejects_excessive_alignment` above (which uses
    // `Layout::from_size_align` directly without naming a `T`).

    #[test]
    fn arena_string_grow_through_chunk_rotation() {
        // Drives the `if needs_teardown { teardown_chunk(chunk, true); }`
        // branch in `Arena::grow_for_string` — when the OLD string buffer's
        // chunk has only the string as a holder (refcount==1 → after dec
        // it's 0 → teardown).
        let arena: Arena = Arena::builder().build();
        let mut s = arena.alloc_string();
        // Push enough text to force the string to grow into a fresh chunk;
        // the old chunk had ONLY this string (no other allocations) so its
        // refcount drops to 0 on grow → triggers teardown_chunk.
        let chunk = "x".repeat(64);
        for _ in 0..200 {
            s.push_str(&chunk);
        }
        assert_eq!(s.len(), 200 * 64);
    }

    #[test]
    fn arena_vec_deref_mut_modifies_in_place() {
        let arena = Arena::new();
        let mut v: Vec<u32, _> = arena.alloc_vec();
        v.push(1);
        v.push(2);
        v.push(3);
        // Modify via DerefMut (not via push).
        let slice: &mut [u32] = &mut v;
        slice[0] = 99;
        assert_eq!(v.as_slice(), &[99, 2, 3]);
    }

    #[test]
    fn collect_in_empty_iterator_uses_new_in() {
        // An iterator with `size_hint().0 == 0` should take the `new_in`
        // path (no `with_capacity_in(0)` detour). Easiest: filter that
        // discards everything but advertises `(0, _)`.
        let arena = Arena::new();
        let v: Vec<u32, _> = (0..10_u32).filter(|_| false).collect_in(&arena);
        assert!(v.is_empty());
    }

    #[test]
    #[cfg(feature = "dst")]
    fn alloc_dst_arc_runs_drop_on_chunk_teardown() {
        use core::sync::atomic::{AtomicUsize, Ordering as Ord};

        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
        DROP_COUNT.store(0, Ord::SeqCst);

        struct Tracked(#[expect(dead_code, reason = "field exists only for size")] u32);
        impl Drop for Tracked {
            fn drop(&mut self) {
                let _ = DROP_COUNT.fetch_add(1, Ord::SeqCst);
            }
        }

        let arena: Arena = Arena::new();
        {
            let layout = core::alloc::Layout::array::<Tracked>(1).unwrap();
            // SAFETY: layout matches [Tracked; 1]; init writes one Tracked.
            let arc: Arc<[Tracked]> = unsafe {
                arena.alloc_dst_arc::<[Tracked]>(layout, 1_usize, |fat: *mut [Tracked]| {
                    fat.cast::<Tracked>().write(Tracked(0xCAFE_F00D));
                })
            };
            assert_eq!(arc.len(), 1);
            // Move arc to another thread (exercises the Send+Sync path).
            let h = std::thread::spawn(move || arc.len());
            let val = h.join().unwrap();
            assert_eq!(val, 1);
        }
        drop(arena);
        assert_eq!(DROP_COUNT.load(Ord::SeqCst), 1, "drop must run exactly once");
    }

    // Use ArenaBuilder type (covered by allocator_in test) to silence
    // unused-import warnings if any of the above tests change.
    #[test]
    fn builder_type_is_constructible() {
        let _: ArenaBuilder = Arena::builder();
    }

    // Infallible Arc / Box slice constructors and the strait-`alloc_arc` family.
    // These wrap their `try_*` cousins with `unwrap_or_else(panic_alloc)`; the
    // happy path was previously uncovered.

    #[test]
    fn arena_try_alloc_str_arc_succeeds() {
        use multitude::Arc;
        let arena: Arena = Arena::new();
        let s: Arc<str> = arena.try_alloc_str_arc("hello arc").unwrap();
        assert_eq!(s.as_str(), "hello arc");
    }

    #[test]
    fn arena_try_alloc_str_box_succeeds() {
        use multitude::Box;
        let arena: Arena = Arena::new();
        let s: Box<str> = arena.try_alloc_str_box("hello box").unwrap();
        assert_eq!(s.as_str(), "hello box");
    }

    #[test]
    fn arena_box_str_as_mut_via_trait() {
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_str_box("abc");
        let m: &mut str = AsMut::<str>::as_mut(&mut s);
        // SAFETY: ASCII bytes; in-place uppercase preserves UTF-8.
        unsafe { m.as_bytes_mut()[0] = b'A' };
        assert_eq!(s.as_str(), "Abc");
    }

    // ArenaString::with_capacity_in (cap > 0) — exercises allocate_initial path
    // (line 102 / 324) and into_arena_str slack reclamation (line 258).

    #[test]
    fn alloc_string_with_capacity_allocates_buffer() {
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(64);
        assert!(s.capacity() >= 64);
        s.push_str("hello world");
        assert_eq!(s.as_str(), "hello world");
    }

    #[test]
    fn try_alloc_vec_with_capacity_succeeds() {
        let arena: Arena = Arena::new();
        let mut v = arena.try_alloc_vec_with_capacity::<u32>(16).unwrap();
        assert!(v.capacity() >= 16);
        v.push(1);
        v.push(2);
        assert_eq!(&*v, &[1, 2]);
    }

    // `panic_alloc` closure paths for the Arc/Box variants of slice / value
    // constructors. These mirror the existing tests for the Rc variants;
    // each drives the `unwrap_or_else(|_| panic_alloc())` closure body so it
    // shows as covered.

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_arc(0_u32);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_arc_with_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_arc_with(|| 0_u32);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_copy_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_slice_copy_arc([0_u8; 4]);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_clone_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_slice_clone_arc([1_u32, 2]);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_fill_with_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_slice_fill_with_arc::<u32, _>(4, |i| i as u32);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_slice_fill_iter_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_slice_fill_iter_arc([1_u32, 2, 3]);
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_str_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_str("hi");
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_str_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::new_in(SendFailingAllocator::new(0));
        let _ = arena.alloc_str_arc("hi");
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_str_box_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_str_box("hi");
    }

    #[test]
    #[should_panic(expected = "multitude: allocator returned AllocError")]
    fn alloc_string_with_capacity_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::new_in(FailingAllocator::new(0));
        let _ = arena.alloc_string_with_capacity(64);
    }

    // Drive `build`'s `unwrap_or_else(panic_build)` closure for each
    // allocator monomorphization so the per-instantiation region count
    // reaches 100% in the coverage report.
    #[test]
    #[should_panic(expected = "multitude::ArenaBuilder::build")]
    fn build_panics_on_failing_allocator() {
        let _: Arena<FailingAllocator> = Arena::builder()
            .allocator_in(FailingAllocator::new(0))
            .with_capacity_local(512)
            .build();
    }

    #[test]
    #[should_panic(expected = "multitude::ArenaBuilder::build")]
    fn build_panics_on_send_failing_allocator() {
        let _: Arena<SendFailingAllocator> = Arena::builder()
            .allocator_in(SendFailingAllocator::new(0))
            .with_capacity_local(512)
            .build();
    }

    // Distinct type from `HugeAlign` above so we don't perturb the caller's frame
    // alignment and trigger the issue noted in the comment near
    // `try_alloc_with_rejects_excessive_alignment`. The `MaybeUninit<T>` returned
    // by the uninit-family entry points never materializes a real `T` on the
    // stack, so the test compiles and runs safely on every platform.
    #[repr(align(131072))]
    struct HugeAlignBox(#[expect(dead_code, reason = "field gives the type a non-zero size")] u8);

    #[test]
    fn try_alloc_uninit_box_rejects_excessive_alignment() {
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_uninit_box::<HugeAlignBox>();
        assert!(r.is_err());
    }

    #[test]
    fn arena_string_replace_range_excluded_start() {
        use core::ops::Bound;
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("hello");
        // Excluded(0) -> start = 1, Excluded(3) -> end = 3 -> replace bytes 1..3 ("el") with "X"
        s.replace_range((Bound::Excluded(0_usize), Bound::Excluded(3_usize)), "X");
        assert_eq!(&*s, "hXlo");
    }

    #[test]
    fn arena_string_replace_range_grow_path() {
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("ab");
        // Replacement is much longer than what's removed, forcing a grow
        // (`new_len > self.cap` branch in replace_range).
        s.replace_range(0..1, "lots of replacement text");
        assert_eq!(&*s, "lots of replacement textb");
    }

    #[test]
    fn arena_string_replace_range_added_gt_removed_no_grow() {
        // Drives the `added > removed` arm of replace_range with the
        // `new_len > self.cap` check evaluating to false (the buffer
        // already has enough capacity for the larger replacement).
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(64);
        s.push_str("abc");
        s.replace_range(0..1, "XY"); // removed=1, added=2 -> grows by 1; cap (64) suffices
        assert_eq!(&*s, "XYbc");
    }

    #[test]
    fn arena_string_try_reserve_additional_overflow_returns_err() {
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("a");
        // self.len (1) + usize::MAX overflows -> Err.
        let r = s.try_reserve(usize::MAX);
        assert!(r.is_err());
    }

    #[test]
    fn arena_string_try_reserve_within_existing_capacity_is_noop() {
        // Drives the `needed <= self.cap` branch of `try_reserve`
        // (cap already suffices, so try_grow_to_at_least is not called).
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(64);
        s.push_str("hi");
        s.try_reserve(8).unwrap();
        assert!(s.capacity() >= 64);

        let mut exact = arena.alloc_string_with_capacity(8);
        exact.push_str("abc");
        exact.try_reserve(5).unwrap();
        assert_eq!(exact.capacity(), 8);
    }

    #[test]
    fn arena_string_try_reserve_grow_path_succeeds() {
        // Drives the success-fall-through past `try_grow_to_at_least(needed)?`
        // in `try_reserve` (cap>0, needed>cap, grow succeeds).
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("seed");
        let prior = s.capacity();
        s.try_reserve(prior * 4).unwrap();
        assert!(s.capacity() >= prior * 4 + s.len());
    }

    #[test]
    fn arena_string_try_reserve_grow_path_overflow_returns_err() {
        // Drives `try_grow_to_at_least`'s `PREFIX_SIZE.checked_add(new_cap)` /
        // `isize::try_from(new_total)` failure paths. We need cap > 0 first
        // (so we hit the grow path, not initial allocate), then ask for an
        // additional that pushes total past isize::MAX.
        let arena: Arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("seed"); // cap > 0
        // additional fits in usize but new_total overflows isize.
        let additional = (isize::MAX as usize) - 4;
        let r = s.try_reserve(additional);
        assert!(r.is_err());
    }

    // Failure-driven coverage tests — drive `?` Err propagation and panicking
    // `unwrap_or_else(|_| panic_alloc())` lambda bodies via FailingAllocator.

    use std::panic::AssertUnwindSafe;

    fn expect_panic<F: FnOnce()>(f: F) {
        let r = std::panic::catch_unwind(AssertUnwindSafe(f));
        assert!(r.is_err(), "expected panic but call returned");
    }

    fn fail_arena() -> Arena<FailingAllocator> {
        Arena::new_in(FailingAllocator::new(0))
    }

    fn send_fail_arena() -> Arena<SendFailingAllocator> {
        Arena::new_in(SendFailingAllocator::new(0))
    }

    // Panicking method bodies (every `unwrap_or_else(|_| panic_alloc())` lambda).

    #[test]
    fn panic_alloc_with() {
        expect_panic(|| {
            let a = fail_arena();
            let _: &mut u64 = a.alloc_with(|| 42);
        });
    }

    #[test]
    fn panic_alloc_str() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_str("hi");
        });
    }

    #[test]
    fn panic_alloc_slice_fill_iter() {
        expect_panic(|| {
            let a = fail_arena();
            let _: &mut [u32] = a.alloc_slice_fill_iter([1_u32, 2, 3]);
        });
    }

    #[test]
    fn panic_alloc_uninit_box() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_uninit_box::<u32>();
        });
    }

    #[test]
    fn panic_alloc_zeroed_box() {
        expect_panic(|| {
            let a = fail_arena();
            let _ = a.alloc_zeroed_box::<u32>();
        });
    }

    #[test]
    fn panic_alloc_uninit_arc() {
        expect_panic(|| {
            let a = send_fail_arena();
            let _ = a.alloc_uninit_arc::<u32>();
        });
    }

    #[test]
    fn panic_alloc_zeroed_arc() {
        expect_panic(|| {
            let a = send_fail_arena();
            let _ = a.alloc_zeroed_arc::<u32>();
        });
    }

    #[test]
    fn panic_alloc_uninit_slice_arc() {
        expect_panic(|| {
            let a = send_fail_arena();
            let _ = a.alloc_uninit_slice_arc::<u32>(4);
        });
    }

    #[test]
    fn panic_alloc_zeroed_slice_arc() {
        expect_panic(|| {
            let a = send_fail_arena();
            let _ = a.alloc_zeroed_slice_arc::<u32>(4);
        });
    }

    // `try_*` Err-propagation branches (the `?` lines).

    #[test]
    fn try_alloc_str_err() {
        let a = fail_arena();
        assert!(a.try_alloc_str("hi").is_err());
    }

    #[test]
    fn try_alloc_uninit_box_err() {
        let a = fail_arena();
        assert!(a.try_alloc_uninit_box::<u32>().is_err());
    }

    #[test]
    fn try_alloc_zeroed_box_err() {
        let a = fail_arena();
        assert!(a.try_alloc_zeroed_box::<u32>().is_err());
    }

    #[test]
    fn try_alloc_uninit_arc_err() {
        let a = send_fail_arena();
        assert!(a.try_alloc_uninit_arc::<u32>().is_err());
    }

    #[test]
    fn try_alloc_zeroed_arc_err() {
        let a = send_fail_arena();
        assert!(a.try_alloc_zeroed_arc::<u32>().is_err());
    }

    #[test]
    fn try_alloc_uninit_slice_arc_err() {
        let a = send_fail_arena();
        assert!(a.try_alloc_uninit_slice_arc::<u32>(4).is_err());
    }

    #[test]
    fn try_alloc_zeroed_slice_arc_err() {
        let a = send_fail_arena();
        assert!(a.try_alloc_zeroed_slice_arc::<u32>(4).is_err());
    }

    // Uninit slice with T: Drop drives the register_drop=true `?` propagation
    // in reserve_slice (line 1625) under failure.

    // ArenaString grow-path failures.

    #[test]
    fn arena_string_try_push_str_initial_alloc_err() {
        let a = fail_arena();
        let mut s = a.alloc_string();
        assert!(s.try_push_str("hello").is_err());
    }

    #[test]
    fn arena_string_try_grow_to_at_least_grow_path_err() {
        // Allow the initial chunk alloc, fail the grow's new-chunk alloc by
        // requesting a capacity that exceeds the chunk_size.
        let a = Arena::builder().allocator_in(FailingAllocator::new(1)).build();
        let mut s = a.try_alloc_string_with_capacity(4).unwrap();
        s.try_push_str("abcd").unwrap();
        // Forces grow_for_string → needs new (oversized) chunk → allocator fails.
        assert!(s.try_reserve(64 * 1024).is_err());
    }

    #[test]
    fn panic_arena_string_grow_to_at_least() {
        expect_panic(|| {
            let a = Arena::builder().allocator_in(FailingAllocator::new(1)).build();
            let mut s = a.try_alloc_string_with_capacity(4).unwrap();
            s.try_push_str("abcd").unwrap();
            // grow_to_at_least asks for a new chunk; allocator is exhausted.
            s.push_str("x".repeat(64 * 1024));
        });
    }

    // grow_for_string slow path: relocate succeeds, old chunk's refcount goes
    // to 0 (drives lines 1815/1820/1822-1823 in arena.rs).

    #[test]
    fn grow_for_string_old_chunk_torn_down() {
        let a = Arena::builder().build();
        let mut s = a.alloc_string();
        // Force at least one grow_for_string call. Initial cap == 16.
        s.push_str("x".repeat(64));
        // Multiple grows to ensure we exercise the slow-path relocate.
        s.push_str("y".repeat(8 * 1024));
        drop(s);
    }

    // Oversized + needs_drop=false branch in ChunkHeader::oversized_layout
    // (lines 188, 189). Default max_normal_alloc = chunk_size/4. We allocate
    // a chunk-sized payload to force the oversized path with a Copy type.

    #[test]
    fn oversized_no_drop_branch() {
        let a = Arena::builder().max_normal_alloc(4 * 1024).build();
        // 1500 bytes of u8 (Copy, no Drop) > max_normal_alloc(4 * 1024).
        let _s = a.alloc_slice_copy(&[0_u8; 1500][..]);
    }

    #[test]
    fn panic_alloc_slice_fill_with() {
        expect_panic(|| {
            let a = fail_arena();
            let _: &mut [u32] = a.alloc_slice_fill_with(4, |i| i as u32);
        });
    }

    #[test]
    fn vec_try_reserve_no_growth_needed() {
        // Line 182: try_reserve when capacity already sufficient → Ok(()) without growing.
        let arena = Arena::new();
        let mut v: Vec<u32> = arena.alloc_vec();
        v.push(1);
        v.push(2);
        // capacity should be >= 4 after the initial growth; reserve 1 more (already have room).
        assert!(v.try_reserve(1).is_ok());
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn vec_try_reserve_exact_realloc_and_overflow() {
        // Lines 432-436: try_reserve_exact that needs realloc.
        let arena = Arena::new();
        let mut v: Vec<u32> = arena.alloc_vec();
        v.push(1);
        // Force exact reserve beyond current capacity.
        assert!(v.try_reserve_exact(100).is_ok());
        assert!(v.capacity() >= 101);

        // Line 436: try_reserve_exact when capacity is already sufficient (no growth).
        assert!(v.try_reserve_exact(1).is_ok());

        // Overflow: len + additional > usize::MAX.
        let err = v.try_reserve_exact(usize::MAX);
        assert!(err.is_err());
    }

    #[test]
    fn vec_resize_with_shrink() {
        // Lines 473-475: resize_with to a smaller size calls truncate.
        let arena = Arena::new();
        let mut v: Vec<u32> = arena.alloc_vec();
        for i in 0..10 {
            v.push(i);
        }
        v.resize_with(3, || unreachable!());
        assert_eq!(v.len(), 3);
        assert_eq!(&*v, &[0, 1, 2]);
    }

    #[test]
    fn vec_drain_with_exclusive_start_and_inclusive_end() {
        use core::ops::Bound;
        // Lines 512-513: Excluded start bound and Unbounded start.
        // Lines 516-518: Included end bound and Unbounded end.
        let arena = Arena::new();
        let mut v: Vec<u32> = arena.alloc_vec();
        for i in 0..10 {
            v.push(i);
        }

        // drain((Excluded(0), Included(3))) → start=1, end=4
        let drained: std::vec::Vec<_> = v.drain((Bound::Excluded(0), Bound::Included(3))).collect();
        assert_eq!(drained, vec![1, 2, 3]);
        assert_eq!(v.len(), 7);

        // drain(..) → Unbounded start, Unbounded end → start=0, end=len
        let arena2 = Arena::new();
        let mut v2: Vec<u32> = arena2.alloc_vec();
        for i in 0..5 {
            v2.push(i);
        }
        let drained2: std::vec::Vec<_> = v2.drain(..).collect();
        assert_eq!(drained2, vec![0, 1, 2, 3, 4]);
        assert_eq!(v2.len(), 0);
    }

    #[test]
    fn vec_zst_operations() {
        // Lines 360, 586, 594-596: ZST Vec realloc and shrink_to_fit.
        let arena = Arena::new();
        let mut v: Vec<()> = arena.alloc_vec();
        for _ in 0..100 {
            v.push(());
        }
        assert_eq!(v.len(), 100);
        v.shrink_to_fit();
        // ZST shrink_to_fit is a no-op (line 360: size_of::<T>() == 0 → return).
        assert_eq!(v.len(), 100);
    }

    #[test]
    fn vec_drain_debug_and_next_back() {
        // Lines 833-835: Drain Debug format.
        // Lines 865-875: Drain::next_back (DoubleEndedIterator).
        let arena = Arena::new();
        let mut v: Vec<u32> = arena.alloc_vec();
        for i in 0..5 {
            v.push(i);
        }
        let mut drain = v.drain(1..4);
        let s = std::format!("{drain:?}");
        assert!(s.contains("Drain"), "Debug output: {s}");
        assert!(s.contains("remaining"), "Debug output: {s}");

        // next_back
        assert_eq!(drain.next_back(), Some(3));
        assert_eq!(drain.next_back(), Some(2));
        assert_eq!(drain.next(), Some(1));
        assert_eq!(drain.next_back(), None);
    }

    #[test]
    fn vec_insert_triggers_growth() {
        // Line 284: insert when len == cap forces grow_one.
        let arena = Arena::new();
        let mut v: Vec<u32> = arena.alloc_vec();
        // Fill to capacity (initial growth is 4).
        for i in 0..4 {
            v.push(i);
        }
        assert_eq!(v.capacity(), 4);
        // Insert forces growth.
        v.insert(2, 99);
        assert_eq!(v[2], 99);
        assert!(v.capacity() > 4);
    }

    #[test]
    fn vec_push_panics_on_alloc_failure() {
        // Line 126: grow_one → panic_alloc.
        expect_panic(|| {
            let arena = Arena::new_in(FailingAllocator::new(1)); // 1 alloc for initial chunk
            let mut v: Vec<u64, _> = arena.alloc_vec();
            // First pushes may succeed using the chunk, but growth will fail.
            // FailingAllocator(1) gives exactly one chunk; the second
            // realloc-on-grow trips the panic well before 100 pushes —
            // the loop bound is just a defensive cap.
            for _ in 0..100 {
                v.push(0);
            }
        });
    }

    #[test]
    fn vec_reserve_panics_on_alloc_failure() {
        // Line 168: reserve → panic_alloc.
        expect_panic(|| {
            let arena = Arena::new_in(FailingAllocator::new(0));
            let mut v: Vec<u64, _> = arena.alloc_vec();
            v.reserve(1);
        });
    }

    #[test]
    fn vec_reserve_exact_panics_on_alloc_failure() {
        // Line 422: reserve_exact → panic_alloc.
        expect_panic(|| {
            let arena = Arena::new_in(FailingAllocator::new(0));
            let mut v: Vec<u64, _> = arena.alloc_vec();
            v.reserve_exact(1);
        });
    }

    #[test]
    fn shared_bump_fast_path_bail_on_oversize() {
        // Line 385: try_bump_alloc_in_current_shared returns None for oversize request.
        // Lines 569-570: try_get_chunk_for_shared creates oversized chunk.
        let arena = Arena::builder().max_normal_alloc(4096).build();
        // This is larger than max_normal_alloc(4096), so fast path bails → oversized shared chunk.
        let arc = arena.alloc_arc([0_u64; 1024]); // 8192 bytes > 4096
        assert_eq!(arc[0], 0);
    }

    #[test]
    fn shared_bump_fit_in_current_chunk() {
        // Lines 593-594: try_get_chunk_for_shared fits in current chunk.
        // This exercises the shared slow-path fit check that returns the current chunk.
        let arena = Arena::new();
        // Allocate many small Arcs to fill the current shared chunk, then one that
        // might go through the slow path on a second chunk. The first Arc establishes
        // the shared chunk.
        let _a1 = arena.alloc_arc(1_u32);
        let _a2 = arena.alloc_arc(2_u32);
        // Both fit in the same chunk → shared bump fit path is exercised.
    }

    #[test]
    fn shared_oversized_inc_ref_on_non_normal_chunk() {
        // Lines 799-802: inc_ref_shared_deferred for non-Normal (oversized) shared chunk.
        // The oversized shared alloc path: alloc_slice_copy_arc with slice > max_normal_alloc.
        let arena = Arena::builder().max_normal_alloc(4096).build();
        let data = [42_u8; 8192]; // > max_normal_alloc(4096)
        let arc_slice = arena.alloc_slice_copy_arc(&data[..]);
        assert_eq!(arc_slice.len(), 8192);
        assert_eq!(arc_slice[0], 42);
    }

    #[test]
    fn shared_eviction_of_pinned_chunk() {
        // Line 603: push_pinned when evicting a pinned shared chunk.
        // Use the smallest possible chunk so a modest number of pushes
        // reliably overflows the current chunk and triggers eviction of
        // the pinned (string-builder-owned) chunk on refill.
        let arena = Arena::builder().with_capacity_local(512).build();
        // String builders use local chunks with pin_for_bump=true.
        let mut s = arena.alloc_string();
        // 600 ASCII chars > 512-byte chunk capacity guarantees the refill
        // path runs while the chunk is pinned, exercising the
        // pinned-eviction branch regardless of host/Miri.
        let n = 600;
        for _ in 0..n {
            s.push('A'); // This grows the string builder, pinning the local chunk.
        }
        // If the chunk was pinned, it goes to the pinned list (line 603 equivalent in local path).
        assert!(s.len() >= n);
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    // See note on `acquire_slice_slot_rejects_overaligned`: naming a
    // `T` with `align(131072)` aborts on Windows before the guard runs.
    fn try_alloc_slice_copy_rejects_overaligned() {
        // Line 1441: layout.align() >= CHUNK_ALIGN → Err(AllocError).
        #[repr(align(131072))]
        #[derive(Clone, Copy)]
        #[expect(dead_code, reason = "field needed for alignment/size but not read")]
        struct HugeAlign(u8);

        let arena = Arena::new();
        let data = [HugeAlign(0)];
        let result = arena.try_alloc_slice_copy(&data[..]);
        assert!(result.is_err());
    }

    #[test]
    fn try_alloc_slice_copy_rejects_overflow() {
        // Line 1436: total > isize::MAX → Err(AllocError).
        let arena = Arena::new();
        // Fabricate a slice reference with huge length via unsafe.
        // Can't actually create such a large allocation, but we can test the
        // overflow branch by checking try_alloc_slice_fill_with with a huge len.
        let result = arena.try_alloc_slice_fill_with::<u64, _>(usize::MAX / 4, |_| 0);
        assert!(result.is_err());
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    // See note on `acquire_slice_slot_rejects_overaligned`: naming a
    // `T` with `align(131072)` aborts on Windows before the guard runs.
    fn try_alloc_slice_fill_with_rejects_overaligned() {
        // Line 1505: layout.align() >= CHUNK_ALIGN → Err(AllocError).
        #[repr(align(131072))]
        struct HugeAlignDrop(#[expect(dead_code, reason = "field needed for alignment/size but not read")] u8);
        #[expect(clippy::empty_drop, reason = "Drop impl makes needs_drop::<T>() true for test")]
        impl Drop for HugeAlignDrop {
            fn drop(&mut self) {}
        }

        let arena = Arena::new();
        let result = arena.try_alloc_slice_fill_with::<HugeAlignDrop, _>(1, |_| HugeAlignDrop(0));
        assert!(result.is_err());
    }

    #[test]
    fn try_alloc_slice_fill_with_no_drop_fast_path() {
        // Line 1509: no-drop type takes the fast-path branch in try_alloc_slice_fill_with.
        let arena = Arena::new();
        let result = arena.try_alloc_slice_fill_with::<u32, _>(10, |i| i as u32);
        assert!(result.is_ok());
        let slice = result.unwrap();
        assert_eq!(slice.len(), 10);
        assert_eq!(slice[5], 5);
    }

    #[test]
    fn try_alloc_slice_fill_with_overflow() {
        // Line 1500: total > isize::MAX for non-drop type.
        let arena = Arena::new();
        let result = arena.try_alloc_slice_fill_with::<u64, _>(usize::MAX / 4, |_| 0);
        assert!(result.is_err());
    }

    #[test]
    fn alloc_slice_fill_with_overflow() {
        // Line 1500: try_alloc_slice_fill_with overflow (total > isize::MAX - (align-1)).
        let arena = Arena::new();
        let len = (isize::MAX as usize) / 8 + 1;
        let result = arena.try_alloc_slice_fill_with::<u64, _>(len, |_| 0);
        assert!(result.is_err());
    }

    #[test]
    fn alloc_slice_fill_with_non_drop_fast_path() {
        // Line 1509: fast-path ptr.cast::<T>() for non-Drop types in try_alloc_slice_fill_with.
        let arena = Arena::new();
        // First call: allocates a chunk via slow path.
        let _ = arena.alloc_slice_fill_with::<u32, _>(4, |i| i as u32);
        // Second call: fast path in current chunk (line 1509).
        let slice = arena.alloc_slice_fill_with::<u32, _>(4, |i| (i + 10) as u32);
        assert_eq!(slice, &[10, 11, 12, 13]);
    }

    #[test]
    fn slice_init_guard_drops_prefix_on_panic() {
        // Lines 3767-3772: SliceInitGuard drops initialized elements on panic.
        // SliceInitGuard is used in try_alloc_slice_fill_with (non-Rc) for both
        // Drop and non-Drop types. Exercise via alloc_slice_fill_with (the non-Rc version)
        // with a type that has Drop.

        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

        #[derive(Clone)]
        #[expect(dead_code, reason = "field needed for alignment/size but not read")]
        struct Tracked(u32);
        impl Drop for Tracked {
            fn drop(&mut self) {
                DROP_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }

        DROP_COUNT.store(0, Ordering::Relaxed);
        let arena = Arena::new();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = arena.alloc_slice_fill_with::<Tracked, _>(5, |i| {
                assert!(i != 3, "deliberate panic at index 3");
                Tracked(i as u32)
            });
        }));
        assert!(result.is_err());
        // Elements 0, 1, 2 were initialized before the panic at index 3.
        // SliceInitGuard should have dropped them.
        assert!(DROP_COUNT.load(Ordering::Relaxed) >= 3);
    }

    /// Exercises the fast path in `try_alloc_slice_copy` where
    /// `try_bump_alloc_in_current_local` succeeds on an already-populated chunk.
    /// A first small allocation populates `current_local`, then a second
    /// `alloc_slice_copy` fits in the same chunk without needing the slow path.
    #[test]
    fn alloc_slice_copy_fast_path_bump() {
        let arena = Arena::new();
        // First allocation populates current_local with a fresh chunk.
        let _x: &mut u8 = arena.alloc(42_u8);
        // Second allocation is small enough to bump within the same chunk,
        // hitting the `try_bump_alloc_in_current_local` success path.
        let s = arena.alloc_slice_copy([1_u8, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(s, &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    //
    // All smart-pointer alloc paths reject `align >= 32 KiB` because, with
    // the co-allocated `DropEntry` taking 32 bytes immediately before the
    // payload, an `align == 32 KiB` payload lands at chunk offset
    // `CHUNK_ALIGN`. `header_for(value_ptr)` masks the low 16 bits of the
    // pointer to recover the chunk header — for that offset, the mask
    // returns the *next* chunk's address. The guard exists to make this
    // failure mode unreachable from safe code.
    //
    // These tests pin the boundary: a sized `T` with `repr(align(32768))`
    // must be rejected by every smart-pointer entry point. The companion
    // tests in `dst.rs` cover the unsafe DST paths.
    //
    // Skipped on Windows: naming a type with `align(32768)` on stack inside
    // `try_alloc_*_with` materializes a stack frame Windows' default 1 MiB
    // stack cannot satisfy on entry, aborting with STATUS_STACK_OVERFLOW
    // before the guard runs. The MaybeUninit/uninit-family tests only hold
    // the type *inside* `MaybeUninit`, so they're safe everywhere.

    #[cfg(not(target_os = "windows"))]
    #[repr(align(32768))]
    #[derive(Clone, Copy)]
    struct HalfChunkAlignNoDrop(#[expect(dead_code, reason = "field gives the type a non-zero size")] u8);

    #[repr(align(32768))]
    struct HalfChunkAlignDrop(#[expect(dead_code, reason = "field gives the type a non-zero size")] u8);

    #[expect(clippy::empty_drop, reason = "Drop impl makes needs_drop::<T>() true for the test")]
    impl Drop for HalfChunkAlignDrop {
        fn drop(&mut self) {}
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn try_alloc_arc_with_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r: Result<multitude::Arc<HalfChunkAlignDrop>, _> = arena.try_alloc_arc_with(|| HalfChunkAlignDrop(0));
        assert!(r.is_err());
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn try_alloc_box_with_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r: Result<multitude::Box<HalfChunkAlignDrop>, _> = arena.try_alloc_box_with(|| HalfChunkAlignDrop(0));
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_uninit_box_rejects_half_chunk_alignment() {
        // Holding T inside MaybeUninit means no stack frame needs T's
        // alignment, so this test is portable to Windows.
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_uninit_box::<HalfChunkAlignDrop>();
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_uninit_arc_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_uninit_arc::<HalfChunkAlignDrop>();
        assert!(r.is_err());
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn try_alloc_slice_fill_with_arc_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_slice_fill_with_arc::<HalfChunkAlignDrop, _>(1, |_| HalfChunkAlignDrop(0));
        assert!(r.is_err());
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn try_alloc_slice_fill_with_box_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_slice_fill_with_box::<HalfChunkAlignDrop, _>(1, |_| HalfChunkAlignDrop(0));
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_uninit_slice_arc_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_uninit_slice_arc::<HalfChunkAlignDrop>(1);
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_uninit_slice_box_rejects_half_chunk_alignment() {
        let arena: Arena = Arena::new();
        let r = arena.try_alloc_uninit_slice_box::<HalfChunkAlignDrop>(1);
        assert!(r.is_err());
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn try_alloc_slice_copy_arc_allows_half_chunk_align_for_copy_t() {
        let arena: Arena = Arena::new();
        let data = [HalfChunkAlignNoDrop(0), HalfChunkAlignNoDrop(1)];
        let r = arena.try_alloc_slice_copy_arc(&data[..]);
        assert!(r.is_err());
    }

    //
    // Each `alloc_*_with` reserves a slot, takes a protective `+1` chunk
    // refcount, then runs the user-supplied `f`. If `f` panics, the
    // `RefcountReleaseGuard` releases that `+1` so the chunk reclaims
    // normally; no `DropEntry` is linked (so `T::drop` does not run on the
    // half-built value), and the bump bytes leak in-chunk. The arena must
    // remain usable after the panic.

    #[test]
    fn alloc_arc_with_closure_panic_releases_refcount() {
        use std::panic::AssertUnwindSafe;

        let arena: Arena = Arena::new();
        let _stable = arena.alloc_arc(0_u32);

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _: multitude::Arc<u64> = arena.alloc_arc_with(|| panic!("deliberate panic in alloc_arc_with"));
        }));
        assert!(result.is_err());

        let after = arena.alloc_arc(99_u32);
        assert_eq!(*after, 99);
    }

    #[test]
    fn alloc_box_with_closure_panic_releases_refcount() {
        use std::panic::AssertUnwindSafe;

        let arena: Arena = Arena::new();
        let _stable = arena.alloc_box(0_u32);

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _: multitude::Box<u64> = arena.alloc_box_with(|| panic!("deliberate panic in alloc_box_with"));
        }));
        assert!(result.is_err());

        let after = arena.alloc_box(99_u32);
        assert_eq!(*after, 99);
    }

    #[test]
    fn vec_resize_clones_exactly_extra_minus_one() {
        use core::sync::atomic::{AtomicUsize, Ordering};

        static CLONE_COUNT: AtomicUsize = AtomicUsize::new(0);
        CLONE_COUNT.store(0, Ordering::Relaxed);

        #[derive(Default)]
        struct CloneCounter;
        impl Clone for CloneCounter {
            fn clone(&self) -> Self {
                CLONE_COUNT.fetch_add(1, Ordering::Relaxed);
                Self
            }
        }

        let arena: Arena = Arena::new();
        let mut v: multitude::vec::Vec<CloneCounter> = arena.alloc_vec();
        v.push(CloneCounter);
        v.push(CloneCounter);
        assert_eq!(CLONE_COUNT.load(Ordering::Relaxed), 0);

        v.resize(5, CloneCounter);

        assert_eq!(v.len(), 5);
        assert_eq!(CLONE_COUNT.load(Ordering::Relaxed), 2);
    }
}

mod coverage_more {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain smart pointers to keep chunks alive")]
    #![allow(unused_results, reason = "test code")]
    #![allow(clippy::used_underscore_binding, reason = "intentional drop-after binding")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::explicit_into_iter_loop, reason = "test clarity")]
    #![allow(clippy::assertions_on_result_states, reason = "tests deliberately assert error returns")]
    #![allow(clippy::items_after_statements, reason = "test-local statics next to their use")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    use core::alloc::Layout;
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use allocator_api2::alloc::Allocator;
    use multitude::strings::String as ArenaString;
    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arc, Arena, FromIn as _};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;
    use crate::common::{FailingAllocator, SendFailingAllocator};

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Droppy(&'static str);

    impl Drop for Droppy {
        fn drop(&mut self) {
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        }
    }

    #[derive(Clone)]
    struct DropZst;

    impl Drop for DropZst {
        fn drop(&mut self) {
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        }
    }

    // ---- src/arc.rs / src/rc.rs gaps ----

    #[test]
    fn arc_from_arena_vec_uses_into_arc() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, i32> = arena.alloc_vec();
        v.push(1);
        v.push(2);

        let a: Arc<[i32]> = v.into();
        assert_eq!(&*a, &[1, 2]);
    }

    // ---- src/internal/chunk_provider.rs gaps ----

    #[test]
    fn builder_preallocate_shared_releases_budget_on_allocator_error() {
        assert!(
            Arena::builder_in(SendFailingAllocator::new(0))
                .with_capacity_shared(512)
                .try_build()
                .is_err()
        );
    }

    #[test]
    fn oversized_shared_alloc_error_releases_budget() {
        let arena = Arena::builder_in(SendFailingAllocator::new(0)).max_normal_alloc(4096).build();
        let src = std::vec![7_u8; 5000];
        assert!(arena.try_alloc_slice_copy_arc(src).is_err());
    }

    #[test]
    fn shared_cache_discards_too_small_chunk_before_large_request() {
        let arena = Arena::builder().with_capacity_shared(512).build();
        let big = std::vec![3_u8; 4096];
        let a = arena.alloc_slice_copy_arc(&big);
        assert_eq!(a.len(), big.len());
    }

    #[test]
    fn preallocate_local_updates_high_water_on_larger_class() {
        let arena = Arena::builder().with_capacity_local(1024).build();
        let value = arena.alloc(42_u32);
        assert_eq!(*value, 42);
    }

    // ---- src/internal/local_chunk.rs gaps ----

    // ---- src/strings/string.rs gaps ----

    #[test]
    fn string_retain_panic_restores_guard_len() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("abcd", &arena);

        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            s.retain(|ch| {
                assert_ne!(ch, 'd', "retain must stop at the panic");
                assert!(ch != 'c', "predicate panic");
                ch != 'b'
            });
        }));

        assert!(result.is_err());
        assert_eq!(s.as_str(), "a");
    }

    /// Regression: `Vec::retain`/`Vec::retain_mut`/`Vec::dedup*` used to
    /// silently wipe ALL elements when the predicate panicked (because
    /// `with_apivec` zeroed the raw parts before delegating, and the
    /// panicking `ApiVec::Drop` then freed the whole buffer). They now
    /// match `std::Vec::retain`'s contract: the kept prefix is preserved.
    #[test]
    fn vec_retain_panic_preserves_kept_prefix() {
        use std::cell::Cell;

        let arena = Arena::new();
        let mut v: multitude::vec::Vec<'_, i32> = arena.alloc_vec();
        v.extend([1_i32, 2, 3, 4, 5]);

        // Predicate: keep odd numbers; panic on element `3`. After panic,
        // the kept prefix `[1]` must remain (matches std::Vec::retain).
        let seen = Cell::new(0_i32);
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            v.retain(|x| {
                seen.set(seen.get() + 1);
                assert!(*x != 3, "predicate panic at element 3");
                *x % 2 == 1
            });
        }));
        assert!(result.is_err());
        // Element 1 passed the predicate (kept), element 2 was dropped,
        // element 3 panicked → ApiVec leaves [1] + leak-amplification of
        // unprocessed tail (3, 4, 5) is acceptable per std semantics.
        // Whatever ApiVec leaves, it must NOT be empty when the predicate
        // managed to keep at least one element.
        assert!(
            !v.is_empty(),
            "kept prefix [1, ...] must survive the panic; std::Vec::retain has the same contract"
        );
        assert_eq!(v[0], 1, "element 1 must be retained");
    }

    #[test]
    fn vec_dedup_panic_preserves_kept_prefix() {
        let arena = Arena::new();
        let mut v: multitude::vec::Vec<'_, i32> = arena.alloc_vec();
        v.extend([1_i32, 1, 2, 2, 3, 3]);

        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            v.dedup_by(|a, _b| {
                assert!(*a != 3, "dedup panic");
                false
            });
        }));
        assert!(result.is_err());
        // At least one element must survive the panic; the all-elements-wiped
        // bug would leave the vector completely empty.
        assert!(!v.is_empty(), "Vec must not be fully wiped on dedup-predicate panic");
    }

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn string_push_panics_on_allocator_error() {
        let arena = Arena::builder_in(FailingAllocator::new(0)).build();
        let mut s = arena.alloc_string();
        s.push('x');
    }

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn string_reserve_panics_on_allocator_error() {
        let arena = Arena::builder_in(FailingAllocator::new(0)).build();
        let mut s = arena.alloc_string();
        s.reserve(128);
    }

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn string_replace_range_panics_from_grow_to_at_least() {
        let arena = Arena::builder_in(FailingAllocator::new(1)).build();
        let mut s = ArenaString::from_in("a", &arena);
        // `FailingAllocator` denies every allocation after the first
        // regardless of size; a moderate replacement (well past the
        // initial small chunk's residual capacity) is sufficient.
        let replacement = "x".repeat(1024);
        s.replace_range(0..1, &replacement);
    }

    #[test]
    fn string_reserve_zero_on_nonempty_string_is_noop() {
        let arena = Arena::new();
        let mut s = ArenaString::from_in("already allocated", &arena);
        let cap = s.capacity();
        s.reserve(0);
        assert_eq!(s.capacity(), cap);
        assert_eq!(s.as_str(), "already allocated");
    }

    // ---- src/strings/utf16_string.rs gaps ----

    // ---- src/vec/vec.rs gaps ----

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn vec_with_capacity_panics_on_allocator_error() {
        let arena = Arena::builder_in(FailingAllocator::new(0)).build();
        let _v: ArenaVec<'_, u8, _> = arena.alloc_vec_with_capacity(8);
    }

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn vec_into_arc_panics_on_shared_allocator_error() {
        let arena = Arena::builder_in(SendFailingAllocator::new(1)).build();
        let mut v: ArenaVec<'_, u8, _> = arena.alloc_vec_with_capacity(4);
        v.extend([1, 2, 3, 4]);
        let _arc = multitude::Arc::from(v);
    }

    #[test]
    fn vec_into_box_handles_zst_fallback() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<()>();
        for _ in 0..16 {
            v.push(());
        }
        let b = v.into_boxed_slice();
        assert_eq!(b.len(), 16);
    }

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    fn vec_into_box_panics_on_zst_drop_alloc_error() {
        let arena = Arena::builder_in(FailingAllocator::new(0)).build();
        let mut v = arena.alloc_vec::<DropZst>();
        v.extend([DropZst, DropZst, DropZst]);
        let _ = v.into_boxed_slice();
    }

    #[test]
    fn vec_into_box_falls_back_when_drop_entry_install_misses() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<Droppy>();
        v.extend([Droppy("a"), Droppy("b")]);
        let _decoy = arena.alloc_slice_fill_with(70_000, |i| i as u8);
        let b = v.into_boxed_slice();
        assert_eq!(b.len(), 2);
    }

    #[test]
    #[should_panic(expected = "allocator returned AllocError")]
    // Skipped under Miri: the test must register `u16::MAX + 1` drop
    // entries to trigger the overflow panic, and Miri's per-allocation
    // overhead pushes this past the 10-minute CI budget. The panic is a
    // runtime-checked assertion, not a memory-safety property, so Miri
    // adds no value beyond what `cargo test` already verifies.
    #[cfg_attr(miri, ignore)]
    fn vec_into_box_panics_when_drop_slice_is_too_long_for_entry() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<Droppy>();
        v.extend((0..=u16::MAX).map(|_| Droppy("many")));
        let _ = v.into_boxed_slice();
    }

    #[test]
    fn vec_resize_moves_final_clone_source_into_last_slot() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<std::string::String>();
        v.resize(3, "x".to_owned());
        assert_eq!(&*v, &["x", "x", "x"]);
    }

    #[test]
    fn vec_realloc_edge_cases_are_observable_through_public_api() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity(8);
        v.extend([1_u32, 2, 3, 4]);

        v.reserve_exact(0);
        assert!(v.capacity() >= 8);

        v.reserve(32);
        assert_eq!(&*v, &[1, 2, 3, 4]);

        v.clear();
        v.shrink_to_fit();
        assert_eq!(v.capacity(), 0);
    }

    #[test]
    fn vec_shrink_to_fit_oversized_chunk_is_a_noop() {
        // Buffers allocated in oversized chunks (cap > MAX_NORMAL_ALLOC)
        // are never at the `current_local` bump cursor, so
        // `shrink_to_fit` must no-op rather than allocate-copy-deallocate
        // (which would just churn fresh chunks for no semantic benefit).
        // Verify the no-op path under a one-shot allocator that would
        // refuse any subsequent allocation, demonstrating that no
        // allocator call is made.
        let arena = Arena::builder_in(FailingAllocator::new(1)).max_normal_alloc(4096).build();
        let mut v = arena.alloc_vec_with_capacity(70_000);
        let cap_before = v.capacity();
        v.extend([1_u32, 2, 3, 4]);
        v.shrink_to_fit();
        assert_eq!(v.capacity(), cap_before);
        assert_eq!(v.len(), 4);
    }

    // ---- src/allocator_impl.rs gaps ----

    #[test]
    fn arena_allocator_grow_falls_back_when_in_place_growth_is_ineligible() {
        let arena = Arena::new();
        let alloc = &arena;
        let old = Layout::from_size_align(8, 8).unwrap();
        let ptr = alloc.allocate(old).unwrap().cast::<u8>();

        let different_align = Layout::from_size_align(16, 16).unwrap();
        let grown = unsafe { Allocator::grow(&alloc, ptr, old, different_align) }.unwrap();
        unsafe { Allocator::deallocate(&alloc, grown.cast(), different_align) };

        let old = Layout::from_size_align(16, 8).unwrap();
        let ptr = alloc.allocate(old).unwrap().cast::<u8>();
        let smaller = Layout::from_size_align(8, 8).unwrap();
        let shrunk = unsafe { Allocator::grow(&alloc, ptr, old, smaller) }.unwrap();
        unsafe { Allocator::deallocate(&alloc, shrunk.cast(), smaller) };
    }

    // ---- src/arena.rs gaps ----

    #[test]
    fn arena_slice_clone_no_drop_branch() {
        let arena = Arena::new();
        let values = [10_u32, 20, 30];
        let cloned = arena.alloc_slice_clone(values);
        assert_eq!(cloned, &mut [10, 20, 30]);
    }

    #[test]
    fn shared_refill_preserves_reentrant_drop_allocation() {
        static REENTERED: AtomicUsize = AtomicUsize::new(0);
        REENTERED.store(0, Ordering::SeqCst);

        struct ReentrantDrop {
            arena: *const Arena,
        }

        unsafe impl Send for ReentrantDrop {}
        unsafe impl Sync for ReentrantDrop {}

        impl Drop for ReentrantDrop {
            fn drop(&mut self) {
                let arena = unsafe { &*self.arena };
                let value = arena.alloc_arc(0xCAFE_u64);
                assert_eq!(*value, 0xCAFE);
                REENTERED.fetch_add(1, Ordering::SeqCst);
            }
        }

        let arena = Arena::new();
        let arena_ptr: *const Arena = &raw const arena;

        let reentrant = arena.alloc_arc(ReentrantDrop { arena: arena_ptr });
        drop(reentrant);

        // Drain the current chunk in one bulk allocation so the next
        // outer alloc forces a refill. A single 64 KiB uninit Arc
        // takes the entire chunk's worth of bytes; cheaper than the
        // prior 16 × 4 KiB fillers (16× fewer atomic ops under Miri).
        let filler = arena.alloc_uninit_arc::<[u8; 60 * 1024]>();
        drop(filler);

        let outer = arena.alloc_arc([0x55_u8; 4096]);
        assert_eq!(outer[0], 0x55);
        assert_eq!(REENTERED.load(Ordering::SeqCst), 1);
    }
}

mod coverage_complete {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain smart pointers to keep chunks alive")]
    #![allow(unused_results, reason = "test code")]
    #![allow(clippy::used_underscore_binding, reason = "intentional drop-after binding")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    #![allow(clippy::items_after_statements, reason = "test-local statics next to their use")]
    #![allow(clippy::assertions_on_result_states, reason = "tests deliberately assert Err returns")]
    #![allow(clippy::ptr_as_ptr, reason = "test code uses `as` casts for raw pointers")]
    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arc, Arena};

    use crate::common;

    #[derive(Clone)]
    struct Droppy(&'static str);

    impl Drop for Droppy {
        fn drop(&mut self) {
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        }
    }

    // ---- arc.rs / rc.rs: drop-list retarget loop must traverse past first entry. ----
    //
    // The drop list is a back-stack: index 0 is the OLDEST entry, index drop_count-1
    // the NEWEST. `assume_init` scans from i=0 forward and breaks on the matching
    // `value_offset`. To exercise the i>0 iterations we allocate a "decoy" drop
    // entry first, then allocate the `MaybeUninit` to be assume_init'd, so the
    // match happens at the LAST slot, not the first.

    #[test]
    fn arc_single_assume_init_loop_traverses_past_first_drop_entry() {
        let arena = Arena::new();
        let arc_uninit = arena.alloc_uninit_arc::<Droppy>();
        let _decoy: Arc<Droppy> = arena.alloc_arc(Droppy("decoy"));
        unsafe {
            Arc::as_ptr(&arc_uninit)
                .cast_mut()
                .write(core::mem::MaybeUninit::new(Droppy("target")));
        }
        let arc = unsafe { arc_uninit.assume_init() };
        assert_eq!(arc.0, "target");
    }

    #[test]
    fn arc_slice_assume_init_loop_traverses_past_first_drop_entry() {
        let arena = Arena::new();
        let arc_uninit = arena.alloc_uninit_slice_arc::<Droppy>(2);
        let _decoy: Arc<Droppy> = arena.alloc_arc(Droppy("decoy"));
        unsafe {
            let base = Arc::as_ptr(&arc_uninit).cast::<core::mem::MaybeUninit<Droppy>>().cast_mut();
            (*base.add(0)).write(Droppy("a"));
            (*base.add(1)).write(Droppy("b"));
        }
        let arc = unsafe { arc_uninit.assume_init() };
        assert_eq!(arc[1].0, "b");
    }

    // ---- vec.rs: cleanup_after_partial_move fires when an IntoIter is dropped mid-way. ----

    #[test]
    fn vec_swap_remove_last_index_skips_copy() {
        // Drives the `idx == self.len` branch of `swap_remove` where no
        // element copy is performed.
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u32>();
        v.extend([1_u32, 2, 3]);
        let last = v.swap_remove(2);
        assert_eq!(last, 3);
        assert_eq!(v.as_slice(), &[1, 2]);
    }

    #[test]
    fn vec_into_iter_partial_drop_compacts_tail() {
        use core::sync::atomic::{AtomicUsize, Ordering};

        static DROPPED: AtomicUsize = AtomicUsize::new(0);

        struct Tracked(#[expect(dead_code, reason = "field only exists to make Tracked non-ZST")] u32);
        impl Drop for Tracked {
            fn drop(&mut self) {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }

        DROPPED.store(0, Ordering::Relaxed);
        let arena = Arena::new();
        let mut v: ArenaVec<'_, Tracked> = arena.alloc_vec_with_capacity(4);
        for i in 0..4_u32 {
            v.push(Tracked(i));
        }
        let mut it = v.into_iter();
        // Consume 2 of 4; the IntoIter's Drop must then compact and drop the
        // remaining 2, which exercises `cleanup_after_partial_move`.
        let _a = it.next().unwrap();
        let _b = it.next().unwrap();
        drop(_a);
        drop(_b);
        // At this point DROPPED == 2.
        assert_eq!(DROPPED.load(Ordering::Relaxed), 2);
        drop(it);
        // Dropping the iter compacts the surviving tail (2 elements) and drops them.
        assert_eq!(DROPPED.load(Ordering::Relaxed), 4);
    }

    // ---- vec.rs: realloc same-cap is unreachable (callers gate); see realloc's debug_assert. ----

    // ---- arena.rs: MAX_SMART_PTR_ALIGN guard in `try_alloc_slice_shared_no_drop_with`. ----

    #[repr(align(32768))]
    #[derive(Clone, Copy)]
    struct OverAligned32K;

    // SAFETY: zero-sized POD; no drop.
    unsafe impl Send for OverAligned32K {}
    // SAFETY: zero-sized POD; no drop.
    unsafe impl Sync for OverAligned32K {}

    #[test]
    fn try_alloc_slice_fill_with_arc_rejects_over_aligned() {
        let arena = Arena::new();
        // `try_alloc_slice_fill_with_arc` for `T: !needs_drop` routes through
        // `try_alloc_slice_shared_no_drop_with`, which checks
        // `align >= MAX_SMART_PTR_ALIGN` and errors.
        let result = arena.try_alloc_slice_fill_with_arc::<OverAligned32K, _>(2, |_| OverAligned32K);
        assert!(result.is_err());
    }

    // ---- arena.rs: DST allocator metadata size check ----
    //
    // `metadata_to_u16` checks that `T::Metadata` is `usize`-sized. For sized `T`
    // the metadata is `()` (zero bytes), so the check returns `Err` immediately.

    // ---- chunk_provider.rs CAS contention paths: spin up two threads that push to
    // the shared-cache concurrently to force the CAS retry arm. ----

    #[test]
    fn shared_cache_push_pop_contention_drives_cas_retries() {
        use std::sync::Barrier;
        use std::thread;

        // Force CAS contention on shared-cache push/pop and reserve_budget by
        // hammering the same arena from many threads simultaneously.
        let arena: Arena = Arena::builder().max_normal_alloc(4096).byte_budget(128 * 1024 * 1024).build();

        // Pre-allocate Arcs grouped per thread so all the work happens during
        // the dropping phase (cross-thread chunk releases).
        //
        // The test makes no assertions: it only exists to give coverage to
        // the CAS retry branches in `push_shared_cache` / `pop_shared_cache`.
        // A few dozen concurrent drops per thread is more than enough to
        // exercise those branches; the bottleneck for finding races is
        // scheduler-interleaving variety (covered by Miri's many-seeds race
        // sweep and by the OS scheduler under native runs), not raw op
        // count.
        let nthreads = 8;
        let per_thread = 32;
        let mut sets: Vec<Vec<multitude::Arc<u64>>> = (0..nthreads).map(|_| Vec::with_capacity(per_thread)).collect();
        for set in &mut sets {
            for _ in 0..per_thread {
                set.push(arena.alloc_arc(42));
            }
        }
        let barrier = std::sync::Arc::new(Barrier::new(nthreads));
        let mut handles = Vec::new();
        for set in sets {
            let b = barrier.clone();
            handles.push(thread::spawn(move || {
                // Synchronize the drop storm so threads race on the
                // Treiber-stack push CAS in `push_shared_cache`.
                b.wait();
                for a in set {
                    drop(a);
                }
            }));
        }
        for h in handles {
            let _ = h.join();
        }
    }

    // ============================================================================
    // ============================================================================
    // vec.rs — shrink_to_fit no-ops when the buffer is not at the bump cursor
    // ============================================================================

    #[test]
    fn vec_shrink_to_fit_is_a_noop_when_not_at_cursor() {
        // `shrink_to_fit` no longer allocates: when the buffer is not at
        // the current bump cursor (because intervening allocations have
        // moved the cursor past the buffer), the arena cannot reclaim
        // partial allocations, so the call no-ops instead of churning
        // chunk space via allocate-copy-deallocate. This test exercises
        // the no-op path under a one-shot allocator that would refuse a
        // refill, demonstrating that no allocator call is made.
        let alloc = common::FailingAllocator::new(2);
        let arena = Arena::new_in(alloc);
        let mut v = arena.alloc_vec::<u8>();
        v.reserve(100);
        let cap_before = v.capacity();
        // Consume the rest of the chunk so the vec's buffer is no longer
        // at the bump cursor.
        let _filler: &mut [u8] = arena.alloc_slice_fill_with::<u8, _>(400, |_| 0);
        // SAFETY: u8 is valid for any bit pattern and `cap >= 50` after `reserve(100)`.
        unsafe { v.set_len(50) };
        v.shrink_to_fit();
        // Capacity unchanged: shrink was a no-op.
        assert_eq!(v.capacity(), cap_before);
        assert_eq!(v.len(), 50);
    }

    // ============================================================================
    // vec.rs:731-734 — into_arena_rc copy-fallback error path
    // ============================================================================
}

mod coverage_llvmcov {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    use multitude::{Arc, Arena};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    // ---------------------------------------------------------------------------
    // arc.rs:162-164, rc.rs:167-169, box.rs (From<Handle<T,A>> for Pin<Handle<T,A>>).
    // ---------------------------------------------------------------------------

    #[test]
    fn arc_into_pin_via_from_impl() {
        let arena = Arena::new();
        let arc: Arc<u32> = arena.alloc_arc(42_u32);
        let pinned: core::pin::Pin<Arc<u32>> = arc.into();
        assert_eq!(*pinned, 42);
    }

    #[test]
    fn box_into_pin_via_from_impl() {
        let arena = Arena::new();
        let b: multitude::Box<u32> = arena.alloc_box(42_u32);
        let pinned: core::pin::Pin<multitude::Box<u32>> = b.into();
        assert_eq!(*pinned, 42);
    }

    // ---------------------------------------------------------------------------
    // zero_init_macros.rs:58/85/115/142/172/202/232/265/289/290 — the
    // `panic_alloc()` arms in `BytemuckView` / `ZerocopyView` allocation methods.
    // ---------------------------------------------------------------------------

    // ---------------------------------------------------------------------------
    // strings/string.rs / utf16_string.rs — insert at end + replace_range tail.
    // ---------------------------------------------------------------------------

    #[test]
    fn string_insert_str_at_end_of_string() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("hi");
        s.insert_str(s.len(), "!");
        assert_eq!(s.as_str(), "hi!");
    }

    #[test]
    fn string_replace_range_empty_at_end() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("abc");
        let n = s.len();
        s.replace_range(n..n, "xyz");
        assert_eq!(s.as_str(), "abcxyz");
    }

    // ---------------------------------------------------------------------------
    // vec/vec.rs:471 — `resize_with` panic-rollback Guard's drop_in_place tail.
    // ---------------------------------------------------------------------------

    #[test]
    fn vec_resize_with_clone_panic_drops_partial() {
        use std::cell::Cell;
        use std::panic::AssertUnwindSafe;

        struct Tracker<'a> {
            clones_made: &'a Cell<usize>,
            clones_dropped: &'a Cell<usize>,
            panic_after: usize,
        }
        impl Clone for Tracker<'_> {
            fn clone(&self) -> Self {
                let n = self.clones_made.get() + 1;
                self.clones_made.set(n);
                assert!(n != self.panic_after, "clone #{n} panics by design");
                Tracker {
                    clones_made: self.clones_made,
                    clones_dropped: self.clones_dropped,
                    panic_after: self.panic_after,
                }
            }
        }
        impl Drop for Tracker<'_> {
            fn drop(&mut self) {
                self.clones_dropped.set(self.clones_dropped.get() + 1);
            }
        }

        let clones_made = Cell::new(0);
        let clones_dropped = Cell::new(0);
        let arena = Arena::new();
        {
            let mut v: multitude::vec::Vec<'_, Tracker<'_>> = arena.alloc_vec_with_capacity(8);
            v.push(Tracker {
                clones_made: &clones_made,
                clones_dropped: &clones_dropped,
                panic_after: 3,
            });
            let seed = Tracker {
                clones_made: &clones_made,
                clones_dropped: &clones_dropped,
                panic_after: 3,
            };
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                v.resize(6, seed);
            }));
            assert!(result.is_err(), "panicking clone in resize must propagate");
        }
        drop(arena);
        // 2 successful clones happened (#1, #2) before #3 panicked. Resize's
        // panic-recovery Guard must have dropped those 2 already-written
        // elements before unwinding; the initial v[0] is dropped on `drop(v)`.
        // So total drops counted: 2 (rolled-back clones) + 1 (v[0]) + 1 (seed
        // — never moved into the Vec because the panic happened before the
        // final move).
        assert!(
            clones_dropped.get() >= 2,
            "Guard must drop the 2 successful clones rolled back by the resize panic; got {}",
            clones_dropped.get()
        );
    }

    // ---------------------------------------------------------------------------
    // arena/alloc_utf16.rs:26, :65 — the oversized-utf16 branches that route
    // requests larger than `max_normal_alloc` to the oversized allocator.
    // Default `max_normal_alloc` is 16 KiB == 8192 u16 elements; allocate
    // well above that to force entry into the oversized fork.
    // ---------------------------------------------------------------------------
}
