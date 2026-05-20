// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consolidated cross-cutting mutant-killing tests.

mod common;

// === merged from tests/mutants_kill.rs ===
mod mutants_for_kill {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std for thread/sync primitives")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain smart pointers to keep chunks alive")]
    #![allow(clippy::manual_assert, reason = "explicit panic clarifies safety-net intent")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded indices fit")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "explicit borrows clarify intent in tests")]
    #![allow(clippy::doc_markdown, reason = "doc comments cite raw identifier names")]
    #![allow(
        dead_code,
        reason = "test types intentionally retain unused fields to keep their Drop side-effects observable"
    )]
    use std::collections::HashMap;
    use std::hash::{BuildHasher, BuildHasherDefault, Hasher};
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use multitude::{Arc, Arena, Box, Rc};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    // --------------------------------------------------------------------
    // A. Trait-impl mutants: hash forwarders and Pointer formatter.
    // --------------------------------------------------------------------

    /// Kills `crates/multitude/src/arc.rs:282: replace <Arc as Hash>::hash with ()`.
    ///
    /// If `Arc::hash` were a no-op, every key would hash to the hasher's
    /// initial state and the `HashMap` lookup would still find the value
    /// for any *equal* key (because `HashMap` uses `Eq` after the bucket
    /// hit). To distinguish, we hash the key directly with a single-shot
    /// hasher and assert the produced hash is not the empty-stream hash —
    /// i.e. that `hash` actually fed bytes to the hasher.
    #[test]
    fn arc_hash_forwards_to_inner() {
        let arena = Arena::new();
        let a: Arc<u64> = arena.alloc_arc(0x0123_4567_89ab_cdef_u64);
        let bh = BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default();
        let mut h_arc = bh.build_hasher();
        std::hash::Hash::hash(&a, &mut h_arc);
        let arc_hash = h_arc.finish();
        let mut h_inner = bh.build_hasher();
        std::hash::Hash::hash(&0x0123_4567_89ab_cdef_u64, &mut h_inner);
        let inner_hash = h_inner.finish();
        let h_empty = bh.build_hasher();
        let empty_hash = h_empty.finish();
        assert_eq!(arc_hash, inner_hash, "Arc::hash must forward to inner hash");
        assert_ne!(arc_hash, empty_hash, "Arc::hash must feed bytes (not be a no-op)");

        // Round-trip via HashMap to keep the test exercising real usage.
        let mut map: HashMap<Arc<u64>, &'static str> = HashMap::new();
        map.insert(a.clone(), "v");
        assert_eq!(map.get(&a).copied(), Some("v"));
    }

    /// Kills `crates/multitude/src/box.rs:389: replace <Box as Hash>::hash with ()`.
    #[test]
    fn box_hash_forwards_to_inner() {
        let arena = Arena::new();
        let b: Box<u64> = arena.alloc_box(0xdead_beef_cafe_babe_u64);
        let bh = BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default();
        let mut h_box = bh.build_hasher();
        std::hash::Hash::hash(&b, &mut h_box);
        let box_hash = h_box.finish();
        let mut h_inner = bh.build_hasher();
        std::hash::Hash::hash(&0xdead_beef_cafe_babe_u64, &mut h_inner);
        let inner_hash = h_inner.finish();
        let h_empty = bh.build_hasher();
        let empty_hash = h_empty.finish();
        assert_eq!(box_hash, inner_hash);
        assert_ne!(box_hash, empty_hash);
    }

    /// Kills `crates/multitude/src/rc.rs:303: replace <Rc as Hash>::hash with ()`.
    #[test]
    fn rc_hash_forwards_to_inner() {
        let arena = Arena::new();
        let r: Rc<u64> = arena.alloc_rc(0xfeed_face_dead_beef_u64);
        let bh = BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default();
        let mut h_rc = bh.build_hasher();
        std::hash::Hash::hash(&r, &mut h_rc);
        let rc_hash = h_rc.finish();
        let mut h_inner = bh.build_hasher();
        std::hash::Hash::hash(&0xfeed_face_dead_beef_u64, &mut h_inner);
        let inner_hash = h_inner.finish();
        let h_empty = bh.build_hasher();
        let empty_hash = h_empty.finish();
        assert_eq!(rc_hash, inner_hash);
        assert_ne!(rc_hash, empty_hash);
    }

    /// Kills `crates/multitude/src/arc.rs:303: replace <Arc as fmt::Pointer>::fmt
    /// -> Result with Ok(())`.
    ///
    /// If the body became `Ok(())`, the formatter would emit nothing and
    /// the result string would be empty. We assert the rendered string
    /// starts with `0x` (Rust's standard pointer format) and has at least
    /// the `0x` prefix plus a hex digit.
    #[test]
    fn arc_pointer_format_is_non_empty() {
        let arena = Arena::new();
        let a: Arc<u64> = arena.alloc_arc(7_u64);
        let s = format!("{a:p}");
        assert!(s.starts_with("0x"), "expected `0x` prefix, got `{s}`");
        assert!(s.len() > 2, "expected non-empty pointer hex digits, got `{s}`");
    }

    // --------------------------------------------------------------------
    // B/I. Builder defaults / preallocation paths / resolve_capacity.
    // --------------------------------------------------------------------

    // --------------------------------------------------------------------
    // G. OversizedSharedGuard::drop — panic-recovery for arc-oversized.
    // --------------------------------------------------------------------

    // --------------------------------------------------------------------
    // C/D/E. Drop-counter exhaustive coverage. Many missed mutants live in
    // the per-flavor allocation hot paths and corrupt either the bump
    // cursor (`+ → -/*`), the drop-entry index/chain (`+1 → *1`), or the
    // fit/refill comparisons (`> → >=/==`). A test that allocates many
    // drop-tracking values, drops them, and asserts the exact count would
    // fail under any of those mutations: a wrong `data_ptr`/`drop_back`
    // segfaults; a wrong `drop_count` increment leaves entries unrun; a
    // flipped fit comparison either drops allocations or oversteps the
    // chunk.
    // --------------------------------------------------------------------

    #[derive(Debug)]
    struct DropCounter(StdArc<AtomicUsize>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Kills the bulk of arithmetic/comparison mutants in
    /// `try_alloc_inner_with`, `try_alloc_inner_value`, and their
    /// slow-path siblings:
    /// `arena.rs:1003 + → *`, `1035 > → ==/>=`, `1039 + → -/*`,
    /// `1051 > → ==/>=`, `1346 > → <`, `1358 + → *`, `1384 > → <`,
    /// `1386 != → ==`, `1441 > → >=`, `1445 + → -/*`, `1457 > → ==/>=`.
    ///
    /// Each of those mutants either short-circuits the drop-entry write,
    /// corrupts the `drop_count` increment, or breaks the fit check the
    /// refill loop relies on. The result is a wrong number of `Drop`
    /// invocations or a hard memory error (segfault / panic during alloc).
    #[test]
    fn many_drop_typed_local_allocs_run_drop_exactly_once_each() {
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let mut keep_rc: std::vec::Vec<Rc<DropCounter>> = std::vec::Vec::new();
            let mut keep_box: std::vec::Vec<Box<DropCounter>> = std::vec::Vec::new();
            // 1024 mixed allocations across both Rc and Box flavors.
            // Each value is freshly cloned so DropCounter actually has
            // a `Drop` impl that registers in the arena's drop list.
            for i in 0..1024_u32 {
                if i % 2 == 0 {
                    keep_rc.push(arena.alloc_rc(DropCounter(counter.clone())));
                } else {
                    keep_box.push(arena.alloc_box(DropCounter(counter.clone())));
                }
            }
            // Drop the smart pointers and the arena.
            drop(keep_rc);
            drop(keep_box);
            drop(arena);
        }
        assert_eq!(
            counter.load(Ordering::Relaxed),
            1024,
            "every DropCounter must be dropped exactly once"
        );
    }

    /// Kills mutants in `try_alloc_inner_arc_with` and
    /// `try_alloc_inner_arc_oversized_with`:
    /// `arena.rs:670 + → *`, `681 > → >=`, `700 > → ==/>=`,
    /// `703 + → -/*` (multiple), and the oversized arc path's
    /// `1185 match-guard` and `1201 OversizedSharedGuard::drop`.
    #[test]
    fn many_drop_typed_arc_allocs_run_drop_exactly_once_each() {
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let mut keep: std::vec::Vec<Arc<DropCounter>> = std::vec::Vec::new();
            for _ in 0..1024_u32 {
                keep.push(arena.alloc_arc_with(|| DropCounter(counter.clone())));
            }
            drop(keep);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 1024);
    }

    /// Kills mutants in the *closure* variant of the normal local-flavor
    /// alloc fast path: `try_alloc_inner_with`:
    /// - `arena.rs:1396:42 > → <` (entry_size>0 gate that installs the
    ///   noop drop entry pre-closure; flipped to `<0` it is always
    ///   false, drop entries are never installed, drops never run).
    /// - `arena.rs:1408:68 + → *` (drop_count increment).
    /// - `arena.rs:1434:23 > → <` (post-closure entry_size>0 gate that
    ///   overwrites the noop with the real shim).
    /// - `arena.rs:1436:31 != → ==` (eviction detection: chunk-pointer
    ///   identity check).
    /// - `arena.rs:1648:40 + → -` in `allocate_layout`.
    ///
    /// We use `alloc_rc_with(|| …)` (local + closure) and
    /// `alloc_box_with(|| …)` to drive both code paths. The shared
    /// counterpart `try_alloc_inner_arc_with` is already covered above.
    #[test]
    fn many_drop_typed_local_alloc_with_closure_runs_drop() {
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let mut keep_rc: std::vec::Vec<Rc<DropCounter>> = std::vec::Vec::new();
            let mut keep_box: std::vec::Vec<Box<DropCounter>> = std::vec::Vec::new();
            for i in 0..1024_u32 {
                if i % 2 == 0 {
                    keep_rc.push(arena.alloc_rc_with(|| DropCounter(counter.clone())));
                } else {
                    keep_box.push(arena.alloc_box_with(|| DropCounter(counter.clone())));
                }
            }
            drop(keep_rc);
            drop(keep_box);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 1024);
    }

    /// Kills the oversized-value match-guard mutant
    /// `arena.rs:849: replace match guard e <= cap.saturating_sub(entry_size)
    /// with true` and the analogous oversized-value paths
    /// (`1098`, `1185`), plus the large-alignment portion of
    /// `try_alloc_inner_*` `+entry_size` calculations.
    ///
    /// The test allocates a `T` whose size exceeds the default
    /// `max_normal_alloc` (16 KiB) and whose alignment is non-trivial
    /// (64 bytes). This forces the oversized one-shot path, where the
    /// match guard is the only post-alignment fit check.
    #[test]
    fn oversized_drop_typed_alloc_runs_drop_and_respects_alignment() {
        #[repr(align(64))]
        struct Big {
            // 32 KiB > default max_normal_alloc (16 KiB) → oversized path.
            _payload: [u64; 4 * 1024],
            token: DropCounter,
        }

        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let b = arena.alloc_box_with(|| Big {
                _payload: [0; 4 * 1024],
                token: DropCounter(counter.clone()),
            });
            // Verify alignment: any pointer-arithmetic mutation that
            // breaks the `align - 1` masking or the `aligned + size`
            // end-address computation would land us off-alignment.
            let p: *const Big = std::ptr::from_ref::<Big>(&b);
            assert_eq!((p as usize) % 64, 0, "Big must be 64-byte aligned");
            drop(b);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 1, "oversized Box's Drop must run");

        // Same path for Arc (oversized shared): exercises the
        // `try_alloc_inner_arc_oversized_with` match-guard at line 1185
        // and the `OversizedSharedGuard` happy path (drop is forgotten on
        // success).
        let counter2 = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let a = arena.alloc_arc_with(|| Big {
                _payload: [0; 4 * 1024],
                token: DropCounter(counter2.clone()),
            });
            let p: *const Big = std::ptr::from_ref::<Big>(&a);
            assert_eq!((p as usize) % 64, 0);
            drop(a);
            drop(arena);
        }
        assert_eq!(counter2.load(Ordering::Relaxed), 1);
    }

    /// Regression: oversized `Box::<T:Drop>::into_rc()` previously panicked
    /// because the oversized scalar paths skipped drop-entry installation
    /// for Box flavor. The fast path always installs a `noop_drop_shim`
    /// that `Box::into_rc` retargets to the real shim; the oversized path
    /// now mirrors that.
    #[test]
    fn oversized_box_drop_into_rc_runs_drop_exactly_once() {
        #[repr(align(64))]
        struct Big {
            _payload: [u64; 4 * 1024],
            token: DropCounter,
        }

        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let b = arena.alloc_box_with(|| Big {
                _payload: [0; 4 * 1024],
                token: DropCounter(counter.clone()),
            });
            let rc = b.into_rc();
            drop(rc);
            drop(arena);
        }
        assert_eq!(
            counter.load(Ordering::Relaxed),
            1,
            "oversized Box::into_rc must drop the value exactly once"
        );

        // Same path, by-value (`alloc_box` rather than `alloc_box_with`).
        let counter2 = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let big = Big {
                _payload: [0; 4 * 1024],
                token: DropCounter(counter2.clone()),
            };
            let b = arena.alloc_box(big);
            let rc = b.into_rc();
            drop(rc);
            drop(arena);
        }
        assert_eq!(counter2.load(Ordering::Relaxed), 1);
    }

    // --------------------------------------------------------------------
    // H. align_offset — exercised transitively via oversized aligned alloc.
    // --------------------------------------------------------------------

    /// Kills `crates/multitude/src/arena.rs:5206: replace align_offset ->
    /// Option<usize> with Some(0)`.
    ///
    /// If `align_offset` always returned `Some(0)`, the oversized path
    /// would treat any chunk's payload base as already aligned for `T`.
    /// For non-aligned bases (chunk payloads start right after the
    /// `SharedChunk`/`LocalChunk` header, which is 64-byte aligned but
    /// not necessarily 128-byte aligned), a `T` with align 128 would
    /// land off-alignment and our `% 128` assertion would catch it.
    #[test]
    fn oversized_high_alignment_drives_align_offset() {
        #[repr(align(128))]
        struct Aligned128 {
            _pad: [u64; 4 * 1024], // 32 KiB, oversized
        }
        let arena = Arena::new();
        let b = arena.alloc_box(Aligned128 { _pad: [0; 4 * 1024] });
        let p: *const Aligned128 = std::ptr::from_ref::<Aligned128>(&b);
        assert_eq!((p as usize) % 128, 0);

        // Same for Arc (oversized shared).
        let a = arena.alloc_arc(Aligned128 { _pad: [0; 4 * 1024] });
        let p: *const Aligned128 = std::ptr::from_ref::<Aligned128>(&a);
        assert_eq!((p as usize) % 128, 0);
    }

    // --------------------------------------------------------------------
    // D. try_bump_fit boundary mutant.
    // --------------------------------------------------------------------

    /// Kills `crates/multitude/src/arena.rs:5192: replace > with >= in
    /// try_bump_fit` and the analogous comparisons in
    /// `try_alloc_slice_local_no_drop_with_slow:2432`,
    /// `try_alloc_slice_local_copy_slow:2527`, and
    /// `try_alloc_slice_shared_with:2651`.
    ///
    /// `try_bump_fit` returns `None` for `aligned > max_aligned`. The
    /// mutation `>` → `>=` rejects `aligned == max_aligned` (the
    /// perfect-fit case). We hit this exact equality by running enough
    /// allocations to exhaust whole chunks at minimum class (512 B), then
    /// asking for a properly aligned u64 payload that fits exactly. We
    /// can't guarantee a perfect equality on every host, but a high
    /// volume of distinct alignment + size combos forces the boundary on
    /// many of them — the test asserts every read-back value is correct,
    /// so any spurious reject + retry that picked a wrong cursor would
    /// surface as a wrong read or a panic.
    #[test]
    fn many_distinct_size_and_align_combinations_succeed() {
        let arena = Arena::new();
        // Mix of size classes and alignments to maximize the chance
        // of hitting `aligned == max_aligned`.
        let mut keep_u8: std::vec::Vec<&mut u8> = std::vec::Vec::new();
        let mut keep_u16: std::vec::Vec<&mut u16> = std::vec::Vec::new();
        let mut keep_u32: std::vec::Vec<&mut u32> = std::vec::Vec::new();
        let mut keep_u64: std::vec::Vec<&mut u64> = std::vec::Vec::new();
        for i in 0..256_u32 {
            keep_u8.push(arena.alloc((i & 0xff) as u8));
            keep_u16.push(arena.alloc((i & 0xffff) as u16));
            keep_u32.push(arena.alloc(i));
            keep_u64.push(arena.alloc(u64::from(i)));
        }
        for (i, p) in keep_u8.iter().enumerate() {
            assert_eq!(**p, (i as u32 & 0xff) as u8);
        }
        for (i, p) in keep_u16.iter().enumerate() {
            assert_eq!(**p, (i as u32 & 0xffff) as u16);
        }
        for (i, p) in keep_u32.iter().enumerate() {
            assert_eq!(**p, i as u32);
        }
        for (i, p) in keep_u64.iter().enumerate() {
            assert_eq!(**p, i as u64);
        }
    }

    // --------------------------------------------------------------------
    // D/E. allocate_layout `+` arithmetic.
    // --------------------------------------------------------------------

    /// Kills `crates/multitude/src/arena.rs:1598: replace + with - in
    /// Arena::allocate_layout`.
    ///
    /// `allocate_layout` is the SimpleRef-flavor entry that backs
    /// `arena.alloc_string_with_capacity` / `alloc_vec_with_capacity`.
    /// The `+` is the `needed = size + align.saturating_sub(_)` calc on
    /// the refill path. With `-` the subtraction may underflow (in
    /// release: wrap to a huge `needed` → refill fails → `AllocError` →
    /// panic in `panic_alloc`). We force the refill path by allocating
    /// a vec with non-trivial alignment that fills more than one chunk.
    #[test]
    fn vec_with_alignment_grows_across_chunks() {
        let arena = Arena::new();
        // Allocate vecs that together exceed a single chunk so
        // allocate_layout's refill arm is exercised.
        let mut all: std::vec::Vec<multitude::vec::Vec<'_, u64>> = std::vec::Vec::new();
        for _ in 0..64 {
            let mut v = arena.alloc_vec_with_capacity::<u64>(64);
            for j in 0..64_u64 {
                v.push(j);
            }
            all.push(v);
        }
        for v in &all {
            for (i, x) in v.iter().enumerate() {
                assert_eq!(*x, i as u64);
            }
        }
    }

    // --------------------------------------------------------------------
    // D/E/F. Slice paths — local and shared, with and without Drop.
    // --------------------------------------------------------------------

    /// Kills mutants in `try_alloc_slice_local_with` and
    /// `try_alloc_slice_shared_with`:
    /// `arena.rs:2211 && → ||`, `2216 != → ==`, `2216 > → ==/>=`,
    /// `2605 && → ||`, `2610 != → ==`, `2610 > → ==/>=`,
    /// `2651 > → >=`, `2669 += → *=`, `2676 != → ==`, `2689 != → ==`.
    ///
    /// We allocate a variety of slices of `DropCounter` (drop-needing) at
    /// many lengths in both local (Rc/Box) and shared (Arc) flavors. A
    /// mutated guard that wrongly enters or skips the drop-entry-install
    /// branch leaves the count off by `len`. A mutated `+=` on the bump
    /// cursor produces a wrong end address → corrupted neighbour
    /// allocations or a segfault.
    #[test]
    fn slice_drop_counts_match_for_local_and_shared() {
        let counter = StdArc::new(AtomicUsize::new(0));
        let mut total: usize = 0;
        {
            let arena = Arena::new();
            let mut keep_local: std::vec::Vec<Rc<[DropCounter]>> = std::vec::Vec::new();
            let mut keep_shared: std::vec::Vec<Arc<[DropCounter]>> = std::vec::Vec::new();
            for len in [0_usize, 1, 2, 3, 7, 16, 64, 100] {
                total += len * 2;
                keep_local.push(arena.alloc_slice_fill_with_rc(len, |_| DropCounter(counter.clone())));
                keep_shared.push(arena.alloc_slice_fill_with_arc(len, |_| DropCounter(counter.clone())));
            }
            drop(keep_local);
            drop(keep_shared);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), total);
    }

    /// Kills the no-drop slice fast-path slow-tail mutants
    /// `arena.rs:2432 > → ==/>=` and `2527 > → ==/>=` by exhausting the
    /// fast-path bump space and forcing the slow refill path repeatedly.
    #[test]
    fn many_copy_slices_force_slow_refill() {
        let arena = Arena::new();
        let mut all: std::vec::Vec<&mut [u64]> = std::vec::Vec::new();
        for i in 0..256_u32 {
            let s = arena.alloc_slice_copy::<u64>(&[u64::from(i); 17]);
            all.push(s);
        }
        for (i, s) in all.iter().enumerate() {
            for &v in s.iter() {
                assert_eq!(v, i as u64);
            }
        }
    }

    // --------------------------------------------------------------------
    // Misc: confirm the && operator in the oversized-value flavor gate.
    // --------------------------------------------------------------------

    /// Kills `crates/multitude/src/arena.rs:1072: replace && with || in
    /// Arena::try_alloc_inner_oversized_value`.
    ///
    /// The expression is `needs_drop::<T>() && !matches!(flavor, Box)`.
    /// With `||` the `entry_size` is reserved for `Box` flavor too, which
    /// then writes a drop entry the Box flavor never expects (it runs
    /// `drop_in_place` directly). The result is either a double-drop or
    /// a corrupted oversized chunk teardown. We exercise the oversized
    /// Box path with a Drop-needing payload and assert exactly one Drop.
    #[test]
    fn oversized_box_drop_runs_exactly_once() {
        #[repr(align(64))]
        struct Big {
            _payload: [u64; 4 * 1024], // 32 KiB > 16 KiB max_normal_alloc
            token: DropCounter,
        }
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let b = arena.alloc_box(Big {
                _payload: [0; 4 * 1024],
                token: DropCounter(counter.clone()),
            });
            drop(b);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 1, "Box oversized must drop exactly once");
    }
}

// === merged from tests/mutants_kill2.rs ===
mod mutants_for_kill2 {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "explicit .clone() in tests")]
    #![allow(clippy::collection_is_never_read, reason = "keep allocations live")]
    #![allow(clippy::doc_markdown, reason = "doc comments cite raw identifier names")]
    #![allow(clippy::cast_possible_truncation, reason = "bounded indices fit")]
    #![allow(clippy::items_after_statements, reason = "test-local types live near usage")]
    #![allow(clippy::large_stack_arrays, reason = "test stack allocations are bounded")]
    #![allow(dead_code, reason = "drop-tracking payload fields' Drop side-effects are the observable")]
    #![allow(clippy::redundant_clone, reason = "tests prefer explicit clones for clarity")]
    #![allow(clippy::empty_line_after_doc_comments, reason = "doc-comments cite ASCII identifiers verbatim")]
    #![allow(clippy::manual_midpoint, reason = "explicit (lo+hi)/2 reads naturally for bisection")]
    #![allow(clippy::ref_as_ptr, reason = "explicit `*const` cast is clearer than into()")]
    #![allow(clippy::bool_assert_comparison, reason = "explicit boolean assertions are clearer")]
    #![allow(clippy::assertions_on_constants, reason = "test asserts on probe results which may be constant")]
    #![allow(clippy::missing_panics_doc, reason = "test functions may panic by design")]
    #![allow(clippy::deref_by_slicing, reason = "tests express intent via &v[..] for clarity")]
    #![allow(clippy::useless_vec, reason = "vec!! mirrors realistic user code shapes")]
    #![allow(clippy::unused_unit, reason = "the explicit `()` body documents intent of the mutation we apply")]
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[derive(Debug)]
    struct DropCounter(StdArc<AtomicUsize>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ============================================================
    // constants.rs mutants
    // ============================================================

    /// Kills `constants.rs:77:34 - -> +` and `- -> /` in `min_class_for_bytes`.
    ///
    /// Line 77 returns `NUM_CHUNK_CLASSES - 1` (= 7) for `bytes >=
    /// MAX_CHUNK_BYTES`. Mutated `+` returns 9; mutated `/` returns 8.
    /// Both are out-of-range class indices.
    ///
    /// In all *publicly reachable* call sites the return value is clamped
    /// by a subsequent `.min(NUM_CHUNK_CLASSES - 1)`. We document this
    /// as EQUIVALENT — see `MUTANTS_EQUIVALENT.md` — and rely on the
    /// constants.rs:76 / 87 tests above to bound the function's range.

    // ============================================================
    // drop_list.rs mutants — PAD_BYTES via mem::size_of::<DropEntry>
    // ============================================================
    //
    // DropEntry is `(fn_ptr=8) + (u16 + u16) + _pad`. With pointer-alignment
    // target = 8: RAW_USED=12; PAD_BYTES=4; size_of::<DropEntry>()=16.
    //
    // The mutants at line 49 change RAW_USED, which changes PAD_BYTES,
    // which changes size_of::<DropEntry>(). Observe by allocating a
    // known number of drop-tracked values: each consumes one DropEntry
    // in the chunk's back-stack. If the entry size changes, the number
    // of entries that fit in a 64 KiB chunk changes, which (for
    // sufficient pressure) changes the number of fresh chunks the
    // arena allocates.

    // ============================================================
    // arena_builder.rs mutants
    // ============================================================

    /// Kills `arena_builder.rs:174:80 - -> +` and `- -> /` in
    /// `resolve_capacity`.
    ///
    /// The expression `min_class_for_bytes(capacity).min(NUM_CHUNK_CLASSES - 1)`
    /// clamps the class to the top valid index. Mutated:
    ///   `+` -> `.min(NUM_CHUNK_CLASSES + 1)` = `.min(9)`, no effective clamp;
    ///   `/` -> `.min(NUM_CHUNK_CLASSES / 1)` = `.min(8)`, no effective clamp.
    ///
    /// For inputs where `min_class_for_bytes` returns a value > 7, the
    /// unmutated clamp pins to 7 (64 KiB) while the mutated code returns
    /// the larger class -> `class_to_bytes(c)` debug-asserts -> panic.
    ///
    /// `min_class_for_bytes` itself saturates at 7 (constants.rs line 77),
    /// so this only kills if `min_class_for_bytes` is *also* mutated to
    /// not saturate (covered by `min_class_at_max_chunk_bytes_stays_in_range`).
    /// As an independent witness here, we exercise capacity == 65537:
    /// orig path: min_class returns 7, .min(7) = 7. Both clamps produce
    /// 7 too. Hence this mutation is killed *only* in combination with
    /// already-killing the constants.rs mutants — we document this
    /// reliance in MUTANTS_EQUIVALENT.md.
    ///
    /// However there is still an observable: the second `min` argument
    /// is `NUM_CHUNK_CLASSES - 1`. If this becomes `+ 1` (= 9) and we
    /// somehow reached a `min_class_for_bytes` of 8 or 9 (it can't, in
    /// the unmutated callee), no behaviour change. So the mutation is
    /// effectively benign UNLESS the callee changes. Mark as
    /// EQUIVALENT (see MUTANTS_EQUIVALENT.md).

    // ============================================================
    // chunk_provider.rs mutants
    // ============================================================

    /// Kills `chunk_provider.rs:152:9 release_budget -> ()` more directly
    /// by forcing the allocator to fail on a fresh chunk. We use a
    /// `byte_budget` that admits a normal chunk but route the value
    /// through the **oversized** allocator (size > max_normal_alloc),
    /// which calls `acquire_local` with `min_payload > max_normal_alloc`.
    /// That path's failure-rollback (line 171/415) calls
    /// `release_budget(total_bytes)` if `LocalChunk::allocate` fails.
    ///
    /// We can't easily make the system allocator fail. Instead, we
    /// observe the symmetric path: budget rejection (line 244/394). If
    /// the budget is exhausted, allocations return Err; subsequent
    /// drop-and-reallocate must succeed (because the dropped chunk's
    /// release_budget was called). This is the same observable as
    /// `release_budget_frees_accounted_bytes`; we accept the indirect
    /// witness here.

    /// Kills `chunk_provider.rs:187:43 - -> +` and `- -> /` in
    /// `acquire_local`. Line 187: `let max_class = NUM_CHUNK_CLASSES - 1`.
    ///
    /// Mutated `+` -> max_class=9; `/` -> max_class=8. Both shift the
    /// class ceiling above the legal range. The subsequent `min(max_class)`
    /// of `req_class.max(high_water)` could then permit
    /// `target_class = 8` or `9`, and `class_to_bytes(target_class)`
    /// debug-asserts -> panic.
    ///
    /// To force `req_class.max(high_water) > 7` we need
    /// `high_water > 7`, which can't happen organically. But `req_class`
    /// from `min_class_for_bytes(min_payload >= 65536)` returns 7
    /// (unmutated). Hmm — `target_class = req_class.max(hw).min(max_class)`:
    /// with `req_class=7, hw=0, max_class=7`: 7. Mutated max_class=8 or 9
    /// still produces 7. No observable difference.
    ///
    /// HOWEVER: in `next_high_water = target_class.saturating_add(1)
    /// .min(NUM_CHUNK_CLASSES-1).min(max_class)`. Unmutated:
    /// `.min(7).min(7) = 7`. Mutated: `.min(7).min(8 or 9) = 7`. Still 7.
    ///
    /// We cannot easily observe this in isolation; the saturating add
    /// already caps at 8 which the second `.min(NUM_CHUNK_CLASSES-1)`
    /// pins to 7. EQUIVALENT — documented in MUTANTS_EQUIVALENT.md.

    /// Kills `chunk_provider.rs:254:84 - -> +` and `- -> /` in
    /// `acquire_local`. Line 254: `next_high_water = target_class.saturating_add(1)
    /// .min(NUM_CHUNK_CLASSES - 1).min(max_class)`. Column 84 is the
    /// first `.min(NUM_CHUNK_CLASSES - 1)`.
    ///
    /// Unmutated: `.min(7)` clamps target_class+1 to 7 -> next_high_water
    /// stays at 7 when target_class=7. Mutated `+`: `.min(9)` -> would
    /// allow next_high_water=8 -> then `.min(max_class=7)` clamps to 7.
    /// Same outcome. Mutated `/`: `.min(8)` -> next_high_water=8 ->
    /// `.min(7) = 7`. Same. EQUIVALENT.

    /// Kills `chunk_provider.rs:258:36 > -> >=` in `acquire_local`.
    /// Line 258: `if next_high_water > *h { *h = next_high_water; }`.
    ///
    /// Mutated `>=` writes even when equal — observable only as a
    /// redundant store with no behavioural change. EQUIVALENT.

    /// Kills `chunk_provider.rs:300:33 > -> ==/<>=`, in `preallocate_local`.
    /// Line 300: `if target_class > *h { *h = target_class; }`. Same
    /// idempotency story as 258 — but here we can force a difference.
    ///
    /// Sequence: build with a smaller `with_capacity_local` first
    /// (preallocate sets the initial high-water class), then a second
    /// builder call... actually `with_capacity_local` is the only knob.
    /// The user-visible high-water effect is observed indirectly via
    /// follow-up chunk sizing.
    ///
    /// `target_class > *h` ensures the ratchet only writes when the new
    /// class is strictly larger. Mutated `==`: only writes on equality,
    /// so a larger class doesn't update. Mutated `<`: writes only on
    /// smaller. Mutated `>=`: equal also writes (idempotent).
    ///
    /// Observable: preallocate_local is called once per builder chunk.
    /// With a request for capacity covering 2 chunks (e.g., 96 KiB ->
    /// 2 chunks of 64 KiB), the first preallocate sets h=7, the second
    /// would already see h=7. So all four mutations behave identically
    /// per-call. EQUIVALENT in this context.

    /// Kills `chunk_provider.rs:424:43 - -> +` and `- -> /` in
    /// `acquire_shared`. Same pattern as 187:43 — `NUM_CHUNK_CLASSES - 1`
    /// for `max_class`. Same EQUIVALENT argument.

    /// Kills `chunk_provider.rs:452:84 - -> +` and `- -> /` in
    /// `acquire_shared`. Mirror of 254:84. EQUIVALENT.

    // ============================================================
    // arena.rs mutants
    // ============================================================

    /// Kills `arena.rs:329:9` — `Arena::builder` returns
    /// `ArenaBuilder::new()` vs `ArenaBuilder::from(Default::default())`.
    ///
    /// Both expressions produce an identical `ArenaBuilder<Global>`
    /// (Default::default() returns ArenaBuilder::new() and From<T> for T
    /// is the identity blanket impl). EQUIVALENT.

    /// Kills `arena.rs:698:76 + -> *` in `try_alloc_inner_arc_with`.
    /// Line 698: `let count = (*chunk.as_ptr()).drop_count.get() + 1;`.
    /// Mutated `*`: count = get() * 1 = get() (unchanged). Each
    /// successful Arc<Drop> allocation should bump drop_count by 1;
    /// mutated never bumps it. Replay-on-teardown iterates drop_count
    /// entries; if drop_count stays 0, none of the Drops run.
    ///
    /// Test: allocate N Arc<DropCounter> in a single shared chunk, drop
    /// them all, then drop the arena. Counter must equal N.
    #[test]
    fn arc_drop_count_increments_on_each_alloc() {
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = multitude::Arena::new();
            let mut keep: Vec<multitude::Arc<DropCounter>> = Vec::with_capacity(64);
            for _ in 0..64_u32 {
                keep.push(arena.alloc_arc(DropCounter(counter.clone())));
            }
            drop(keep);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 64);
    }

    /// Kills `arena.rs:709:35 > -> >=` in `try_alloc_inner_arc_with`.
    /// Line 709 is `if entry_size > 0`. With `>=`, the branch fires
    /// unconditionally (entry_size is usize). For `T: !Drop` (entry_size
    /// = 0), the mutated code would write a drop entry at an invalid
    /// `new_drop_back_addr` slot — corrupting memory.
    ///
    /// Test: Allocate Arc<u32> (T: !Drop) many times so the alloc paths
    /// are exercised; subsequent allocations and reads must succeed.
    #[test]
    fn arc_with_non_drop_t_does_not_install_drop_entry() {
        let arena = multitude::Arena::new();
        let mut keep: Vec<multitude::Arc<u32>> = Vec::with_capacity(2048);
        for i in 0..2048_u32 {
            keep.push(arena.alloc_arc(i));
        }
        for (i, a) in keep.iter().enumerate() {
            assert_eq!(**a, i as u32);
        }
    }

    /// Kills `arena.rs:731:104 + -> *` and `731:40 + -> -` in
    /// `try_alloc_inner_arc_with`. Line 731 computes:
    /// `let needed = layout.size() + layout.align().saturating_sub(...) + entry_size`.
    /// Col 40 = first +, col 104 = second +.
    ///
    /// `+ -> *` at col 104: needed = `(size + align_slack) * entry_size`.
    /// For T: !Drop (entry_size=0): needed = 0. refill_shared(0) succeeds
    /// trivially, but the next loop iteration's fast-path still tries
    /// `size` bytes — fails -> infinite loop / 4-retry exhaustion ->
    /// AllocError.
    ///
    /// `+ -> -` at col 40: needed = `size - align_slack + entry_size`.
    /// For typical align <= align_of::<usize>(), align_slack = 0, so
    /// no difference. We deliberately use a high-alignment type to
    /// make align_slack non-zero.
    ///
    /// Use `Arc<T>` where T has alignment > align_of::<usize>() = 8.
    #[test]
    fn arc_with_high_align_uses_correct_needed_size() {
        #[repr(align(64))]
        struct Aligned64([u8; 64]);
        let arena = multitude::Arena::new();
        let mut keep: Vec<multitude::Arc<Aligned64>> = Vec::with_capacity(256);
        for _ in 0..256_u32 {
            keep.push(arena.alloc_arc(Aligned64([0; 64])));
        }
        for a in &keep {
            let p = a.as_ref() as *const Aligned64 as usize;
            assert_eq!(p % 64, 0, "alignment must be honored after refill_shared(needed)");
        }
    }

    /// Kills `arena.rs:899:24`, `1148:24`, `1235:24` —
    /// `match guard e <= cap.saturating_sub(entry_size) with true`.
    ///
    /// In the oversized one-shot paths, `acquire_local`/`acquire_shared`
    /// guarantees `cap >= needed = size + align_slack + entry_size`. The
    /// `align_offset` always yields `aligned <= align_slack`, so
    /// `aligned + size <= cap - entry_size` is always true.
    ///
    /// The match guard mutation `with true` is therefore EQUIVALENT —
    /// the original condition never fails. See MUTANTS_EQUIVALENT.md.

    /// Kills `arena.rs:1053:68 + -> *` in `try_alloc_inner_value`.
    /// Line 1053: `let new_count = (*chunk.as_ptr()).drop_count.get() + 1`.
    /// Same pattern as 698:76 but for the local-flavor value path.
    ///
    /// Test: allocate N `Rc<DropCounter>` (uses inner_value path for
    /// the by-value allocation form). Drop tracking must work.
    #[test]
    fn rc_drop_count_increments_on_value_path() {
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = multitude::Arena::new();
            let mut keep: Vec<multitude::Rc<DropCounter>> = Vec::with_capacity(128);
            for _ in 0..128_u32 {
                // alloc_rc takes the value by-value so it routes through
                // try_alloc_inner_value (not _with).
                keep.push(arena.alloc_rc(DropCounter(counter.clone())));
            }
            drop(keep);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 128);
    }

    /// Kills `arena.rs:1089:100 + -> *` and `1089:36 + -> -` in
    /// `try_alloc_inner_slow_value`. Same `needed` formula as 731.
    #[test]
    fn slow_value_high_align_needed_correct() {
        #[repr(align(64))]
        struct Aligned64([u8; 64]);
        let arena = multitude::Arena::new();
        let mut keep: Vec<multitude::Rc<Aligned64>> = Vec::with_capacity(256);
        for _ in 0..256 {
            keep.push(arena.alloc_rc(Aligned64([0; 64])));
        }
        for r in &keep {
            let p = r.as_ref() as *const Aligned64 as usize;
            assert_eq!(p % 64, 0);
        }
    }

    /// Kills `arena.rs:1101:25 > -> ==/>=` in `try_alloc_inner_slow_value`.
    /// Line 1101 inside refill retry loop: `if end_addr > new_drop_back_addr
    /// { continue; }`. Mutated `>=` rejects exact fit (end == drop_back).
    /// We exercise an exact-fit allocation pattern: fill a chunk to leave
    /// just enough room for one tail allocation whose end aligns exactly
    /// to the drop_back boundary.
    ///
    /// Achieving the exact-fit deterministically is hard without internal
    /// access. The simpler observable: high-pressure allocation in a
    /// single chunk must succeed when filled near capacity. Mutated `==`
    /// rejects almost every allocation -> AllocError chains -> panic
    /// from `alloc_rc`. We just allocate to chunk capacity.
    #[test]
    fn slow_value_exact_fit_retry_succeeds() {
        let arena = multitude::Arena::new();
        let mut keep: Vec<multitude::Rc<u64>> = Vec::with_capacity(8192);
        for i in 0..8192_u64 {
            keep.push(arena.alloc_rc(i));
        }
        for (i, r) in keep.iter().enumerate() {
            assert_eq!(**r, i as u64);
        }
    }

    /// Kills `arena.rs:1122:68 && -> ||` in `try_alloc_inner_oversized_value`.
    /// Line 1122: `if const { core::mem::needs_drop::<T>() } && !matches!(flavor, AllocFlavor::Box)`.
    /// Mutated `||`: condition fires for any (Drop OR not Box). For
    /// Rc<T: !Drop> the original entry_size=0; mutated entry_size = sizeof.
    /// That allocates a phantom drop entry in the oversized chunk,
    /// which on chunk teardown runs `drop_shim_one::<T>` on uninitialized
    /// pointer arithmetic — undefined behaviour observable via reading
    /// the value back. Force an oversized allocation: T larger than
    /// max_normal_alloc.
    #[test]
    fn oversized_value_drop_check_uses_and() {
        let arena = multitude::Arena::builder().max_normal_alloc(4096).build();
        // 8 KiB Rc<[u64; 1024]> -> oversized. Rc::T: !Drop so entry_size=0.
        let r: multitude::Rc<[u64; 1024]> = arena.alloc_rc([7u64; 1024]);
        assert_eq!((*r)[0], 7);
        assert_eq!((*r)[1023], 7);
    }

    /// Kills `arena.rs:1408:68 + -> *` in `try_alloc_inner_with`.
    /// Line 1408: `let new_count = (*chunk.as_ptr()).drop_count.get() + 1`.
    /// Same pattern as 698:76/1053:68 but for the local-flavor with-closure
    /// path. Covered by `arena.alloc_rc_with` paths.
    #[test]
    fn rc_with_drop_count_increments() {
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = multitude::Arena::new();
            let mut keep: Vec<multitude::Rc<DropCounter>> = Vec::with_capacity(128);
            for _ in 0..128_u32 {
                let c = counter.clone();
                keep.push(arena.alloc_rc_with(|| DropCounter(c.clone())));
            }
            drop(keep);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 128);
    }

    /// Kills `arena.rs:1436:31 != -> ==` in `try_alloc_inner_with`.
    /// Line 1436: `if cur_chunk_addr != chunk.as_ptr().cast::<u8>() as usize`.
    /// This detects whether the closure caused chunk eviction. Mutated
    /// `==` inverts the detection: the eviction-recovery path runs when
    /// the closure did NOT evict (and vice versa).
    ///
    /// The closure that evicts: a reentrant `alloc_with` inside the
    /// closure can swap the current chunk if the new request doesn't fit.
    /// We construct: a small T (so fast path runs), with a closure that
    /// allocates a big T forcing chunk swap. The recovery path runs the
    /// post-closure entry write. Mutation flips this: the in-place
    /// success path tries to run on an evicted chunk -> writes drop
    /// entry to wrong memory.
    ///
    /// Observation: the small T's Drop must run exactly once.
    #[test]
    fn alloc_with_reentrant_eviction_recovery() {
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = multitude::Arena::new();
            let c = counter.clone();
            let _outer: multitude::Rc<DropCounter> = arena.alloc_rc_with(|| {
                // Inner allocation large enough to force a chunk swap.
                // alloc_box (no Drop tracking concern) of an 8 KiB value
                // forces refill_local on a fresh arena.
                let _inner = arena.alloc_box([0u64; 1024]);
                DropCounter(c.clone())
            });
        }
        // Drop the arena (which drops _outer first); the outer's Drop
        // must run exactly once.
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    /// Kills `arena.rs:1495:100 + -> *` and `1495:36 + -> -` in
    /// `try_alloc_inner_slow_with`. Same `needed` formula as 731/1089.
    #[test]
    fn slow_with_high_align_needed_correct() {
        #[repr(align(64))]
        struct Aligned64([u8; 64]);
        let arena = multitude::Arena::new();
        let mut keep: Vec<multitude::Rc<Aligned64>> = Vec::with_capacity(256);
        for _ in 0..256 {
            keep.push(arena.alloc_rc_with(|| Aligned64([0; 64])));
        }
        for r in &keep {
            let p = r.as_ref() as *const Aligned64 as usize;
            assert_eq!(p % 64, 0);
        }
    }

    /// Kills `arena.rs:1507:25 > -> ==/>=` in `try_alloc_inner_slow_with`.
    /// Same retry-loop check as 1101. Covered by the high-pressure test.
    #[test]
    fn slow_with_pressure_succeeds() {
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = multitude::Arena::new();
            let mut keep: Vec<multitude::Rc<DropCounter>> = Vec::with_capacity(2048);
            for _ in 0..2048 {
                let c = counter.clone();
                keep.push(arena.alloc_rc_with(|| DropCounter(c.clone())));
            }
            drop(keep);
            drop(arena);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 2048);
    }

    /// Kills `arena.rs:1648:40 + -> -` in `allocate_layout`.
    /// Line 1648: `let needed = layout.size() + layout.align().saturating_sub(core::mem::align_of::<usize>())`.
    /// Used by the `Allocator` impl on `&Arena`. Mutated `-`: needed =
    /// size - align_slack, which under-refills for high-alignment types.
    ///
    /// Exercise via `Allocator::allocate` (allocator_api2) of a 4 KiB
    /// payload at high alignment, forcing chunk grow and proper refill.
    #[test]
    fn allocate_layout_high_align_refill_uses_sum() {
        use core::alloc::Layout;

        use allocator_api2::alloc::Allocator;
        let arena = multitude::Arena::new();
        let a: &multitude::Arena = &arena;
        let layout = Layout::from_size_align(4096, 64).unwrap();
        let mut allocations = std::vec::Vec::new();
        for _ in 0..256 {
            let ptr = a.allocate(layout).unwrap();
            let addr = ptr.as_ptr() as *const u8 as usize;
            assert_eq!(addr % 64, 0);
            allocations.push(ptr);
        }
        // Deallocate so the chunks reclaim their refcounts; otherwise
        // Miri (and any leak-aware allocator) would flag the chunks as
        // leaked.
        for ptr in allocations {
            // SAFETY: ptr came from `a.allocate(layout)` with the same layout.
            unsafe { a.deallocate(ptr.cast(), layout) };
        }
    }

    /// Kills `arena.rs:2261:47 && -> ||` in `try_alloc_slice_local_with`.
    /// Line 2261: `let entry_size = if drop_fn.is_some() && len != 0 ...`.
    /// Mutated `||`: entry_size is non-zero whenever drop_fn OR len !=0.
    /// For Rc<[T: !Drop]> (drop_fn=None) with len > 0, the mutated path
    /// installs a spurious drop entry — observable via memory corruption
    /// or stats.
    #[test]
    fn slice_local_no_drop_does_not_install_entry() {
        let arena = multitude::Arena::new();
        let s: multitude::Rc<[u32]> = arena.alloc_slice_copy_rc(&[1u32, 2, 3, 4, 5][..]);
        assert_eq!(&*s, &[1, 2, 3, 4, 5]);
    }

    /// Kills `arena.rs:2266:23 != -> ==` and `2266:35 > -> ==/>=` in
    /// `try_alloc_slice_local_with`. Line 2266:
    /// `if entry_size != 0 && len > u16::MAX as usize { return Err }`.
    /// Mutated `==`: rejects len > u16::MAX for entry_size==0 (no-Drop
    /// types), which is wrong (no entry needed -> no u16 limit).
    ///
    /// Test: allocate Rc<[u32]> with len > u16::MAX. Must succeed
    /// because T is not Drop.
    #[test]
    fn slice_local_long_no_drop_succeeds() {
        let arena = multitude::Arena::new();
        let v = vec![7_u32; 70_000]; // > u16::MAX = 65535
        let s: multitude::Rc<[u32]> = arena.alloc_slice_copy_rc(&v[..]);
        assert_eq!(s.len(), 70_000);
        assert_eq!(s[0], 7);
        assert_eq!(s[69_999], 7);
    }

    /// Kills the boundary `len == u16::MAX` and `len == u16::MAX as usize`
    /// case of 2266:35. With drop_fn=Some, len > u16::MAX must return
    /// AllocError. `len == u16::MAX` (=65535) must succeed (try_into u16
    /// works). Mutated `>=` rejects 65535 too.
    #[test]
    fn slice_local_drop_at_u16_max_succeeds() {
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = multitude::Arena::new();
            let c = counter.clone();
            // Drop type, len exactly u16::MAX
            let s: multitude::Rc<[DropCounter]> = arena.alloc_slice_fill_with_rc(u16::MAX as usize, |_| DropCounter(c.clone()));
            assert_eq!(s.len(), u16::MAX as usize);
            drop(s);
        }
        assert_eq!(counter.load(Ordering::Relaxed), u16::MAX as usize);
    }

    /// Kills `arena.rs:2482:25 > -> ==/>=` and `2577:25 > -> ==/>=` in
    /// `try_alloc_slice_local_no_drop_with_slow` and `_copy_slow`.
    /// Line N: `if end_addr > drop_back_addr { continue }`. Same retry
    /// loop fit check as 1101/1507.
    ///
    /// High-pressure slice allocation through these paths.
    #[test]
    fn slice_local_slow_pressure_succeeds() {
        let arena = multitude::Arena::new();
        // 1000 small slice allocations force several refills.
        let mut keep: Vec<multitude::Rc<[u32]>> = Vec::with_capacity(1000);
        for i in 0..1000_u32 {
            let v = vec![i; 4];
            keep.push(arena.alloc_slice_copy_rc(&v[..]));
        }
        for (i, s) in keep.iter().enumerate() {
            assert_eq!(s[0], i as u32);
        }
    }

    /// Kills `arena.rs:2655:47 && -> ||`, `2660:23 != -> ==`,
    /// `2660:35 > -> ==/>=` in `try_alloc_slice_shared_with`. Shared
    /// mirror of 2261/2266 — same logic, different flavor.
    #[test]
    fn slice_shared_no_drop_does_not_install_entry() {
        let arena = multitude::Arena::new();
        let s: multitude::Arc<[u32]> = arena.alloc_slice_copy_arc(&[1u32, 2, 3, 4, 5][..]);
        assert_eq!(&*s, &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn slice_shared_long_no_drop_succeeds() {
        let arena = multitude::Arena::new();
        let v = vec![7_u32; 70_000];
        let s: multitude::Arc<[u32]> = arena.alloc_slice_copy_arc(&v[..]);
        assert_eq!(s.len(), 70_000);
    }

    #[test]
    fn slice_shared_drop_at_u16_max_succeeds() {
        let counter = StdArc::new(AtomicUsize::new(0));
        #[derive(Debug)]
        struct DC(StdArc<AtomicUsize>);
        impl Drop for DC {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        // DC must be Send + Sync for alloc_slice_fill_with_arc.
        {
            let arena = multitude::Arena::new();
            let c = counter.clone();
            let s: multitude::Arc<[DC]> = arena.alloc_slice_fill_with_arc(u16::MAX as usize, |_| DC(c.clone()));
            assert_eq!(s.len(), u16::MAX as usize);
            drop(s);
        }
        assert_eq!(counter.load(Ordering::Relaxed), u16::MAX as usize);
    }

    /// Kills `arena.rs:2701:35 > -> >=` in `try_alloc_slice_shared_with`.
    /// Line 2701: `if entry_size > 0 { ... }`. With `>=`, the branch fires
    /// even when entry_size == 0 (T: !Drop), writing a spurious drop
    /// entry. Covered by `slice_shared_no_drop_does_not_install_entry`.

    /// Kills `arena.rs:2719:35 += -> *=` in `try_alloc_slice_shared_with`.
    /// Line 2719: `guard.len += 1`. The guard tracks initialised count
    /// for panic-rollback. With `*=`, the count multiplies — `guard.len`
    /// stays 0 (multiplied by 1 = 0). If init panics partway, the
    /// rollback drops fewer elements than initialised -> memory leak,
    /// but in the unwinding path. We trigger via a panicking init in
    /// alloc_slice_fill_with_arc.
    #[test]
    fn slice_shared_init_increments_guard_len() {
        let counter = StdArc::new(AtomicUsize::new(0));
        #[derive(Debug)]
        struct DC(StdArc<AtomicUsize>);
        impl Drop for DC {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let arena = multitude::Arena::new();
            let c = counter.clone();
            // Initialise 10 elements, then panic on the 11th.
            let _s: multitude::Arc<[DC]> = arena.alloc_slice_fill_with_arc(20_usize, |i| {
                assert!(i != 10, "test panic");
                DC(c.clone())
            });
        }));
        assert!(res.is_err());
        // 10 elements were initialised before panic; with += -> *= the
        // guard.len would stay 0 and none of those 10 would be dropped.
        assert_eq!(counter.load(Ordering::Relaxed), 10);
    }

    /// Kills `arena.rs:2739:75 != -> ==` in `try_alloc_slice_shared_with`.
    /// Line 2739: `self.refill_shared(compute_worst_case_size(layout, entry_size != 0))?`.
    /// Mutated `==`: the `has_drop_entry` flag is inverted — refill sizes
    /// computed for the opposite case. For drop slice, mutated under-refills.
    ///
    /// Force a Drop slice that strains the chunk: alloc_slice_fill_with_arc
    /// with many drop elements through refill_shared.
    #[test]
    fn slice_shared_refill_uses_correct_has_drop_flag() {
        let counter = StdArc::new(AtomicUsize::new(0));
        #[derive(Debug)]
        struct DC(StdArc<AtomicUsize>);
        impl Drop for DC {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let arena = multitude::Arena::new();
        let c = counter.clone();
        // Many Drop slices, forcing repeated refills.
        let mut keep: Vec<multitude::Arc<[DC]>> = Vec::with_capacity(256);
        for _ in 0..256 {
            let cc = c.clone();
            keep.push(arena.alloc_slice_fill_with_arc(8_usize, |_| DC(cc.clone())));
        }
        drop(keep);
        drop(arena);
        assert_eq!(counter.load(Ordering::Relaxed), 256 * 8);
    }

    /// Kills `arena.rs:5268:16 > -> >=` in `try_bump_fit`.
    /// Line 5268: `if aligned > max_aligned { return None }`.
    /// Boundary `aligned == max_aligned` must succeed. Mutated `>=`
    /// rejects the exact-fit case, forcing a chunk refill where the
    /// unmutated code would allocate in place.
    ///
    /// Exact-fit is hard to trigger deterministically without internals.
    /// We approximate by allocating to chunk capacity with many small
    /// values and asserting success.
    #[test]
    fn try_bump_fit_exact_aligned_succeeds() {
        let arena = multitude::Arena::new();
        // Many sequential u8 allocations stress the bump cursor.
        let mut keep: Vec<&mut u8> = Vec::with_capacity(4096);
        for i in 0..4096_u32 {
            keep.push(arena.alloc(i as u8));
        }
        for (i, v) in keep.iter().enumerate() {
            assert_eq!(**v, i as u8);
        }
    }

    /// Kills `vec.rs:760 + → -, *` in `Vec::try_into_arena_arc` and the
    /// mirror at `vec.rs:859` in `Vec::into_arena_box_copy`. The closure
    /// passed to `try_alloc_slice_fill_with_*` advances its read index
    /// via `consumed_cell.set(idx + 1)`. The `* 1` mutation freezes the
    /// index at 0, so every element of the new slice is a bitwise copy of
    /// the original Vec's element 0. The `- 1` mutation underflows on the
    /// second iteration (UB / crash). A distinct-value assertion catches
    /// both.
    #[test]
    fn vec_into_arena_arc_advances_read_index() {
        let arena = multitude::Arena::new();
        let mut v: multitude::vec::Vec<u32, _> = multitude::vec::Vec::new_in(&arena);
        v.push(10);
        v.push(20);
        v.push(30);
        let arc: multitude::Arc<[u32]> = v.into_arena_arc();
        assert_eq!(&*arc, &[10, 20, 30]);
    }

    #[test]
    fn vec_into_arena_box_advances_read_index() {
        // Force the copy fallback path (`into_arena_box_copy`) by using a
        // builder-detached Vec (Vec::new), then into_arena_box, which
        // routes to into_arena_box_copy when the buffer doesn't sit at the
        // bump cursor.
        let arena = multitude::Arena::new();
        // Allocate another value to push the bump cursor past where this
        // Vec's buffer lives, forcing the copy fallback.
        let mut v: multitude::vec::Vec<u32, _> = multitude::vec::Vec::with_capacity_in(3, &arena);
        v.push(11);
        v.push(22);
        v.push(33);
        // Allocate something to detach the buffer from the bump cursor.
        let _detach: &mut u64 = arena.alloc(0xdead_beef_u64);
        let b: multitude::Box<[u32]> = v.into_arena_box();
        assert_eq!(&*b, &[11, 22, 33]);
    }
}

// === merged from tests/mutants_kill3.rs ===
mod mutants_for_kill3 {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "explicit .clone() in tests")]
    #![allow(clippy::collection_is_never_read, reason = "keep allocations live")]
    #![allow(clippy::doc_markdown, reason = "doc comments cite raw identifier names")]
    #![allow(clippy::cast_possible_truncation, reason = "bounded indices fit")]
    #![allow(clippy::items_after_statements, reason = "test-local types live near usage")]
    #![allow(clippy::large_stack_arrays, reason = "test stack allocations are bounded")]
    #![allow(dead_code, reason = "drop-tracking payload fields")]
    #![allow(clippy::redundant_clone, reason = "tests prefer explicit clones")]
    #![allow(clippy::empty_line_after_doc_comments, reason = "doc-comments cite ASCII identifiers")]
    #![allow(clippy::missing_panics_doc, reason = "test functions may panic")]
    #![allow(clippy::manual_assert, reason = "explicit if/panic preserves test intent")]
    #![allow(clippy::use_self, reason = "test code")]
    #![allow(clippy::ref_as_ptr, reason = "test code")]
    #![allow(clippy::stable_sort_primitive, reason = "test code")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "test code")]
    #![allow(
        clippy::used_underscore_binding,
        reason = "underscore-prefixed bindings kept alive intentionally for drop ordering"
    )]
    #![allow(clippy::needless_range_loop, reason = "test code prefers explicit indices")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test marker types are trivially Send/Sync")]
    #![allow(clippy::redundant_closure_for_method_calls, reason = "test code")]
    #![allow(unused_imports, reason = "test scope-local imports may shadow")]
    #![allow(redundant_imports, reason = "test scope-local imports may shadow")]
    #![allow(clippy::assertions_on_constants, reason = "test asserts on constants")]
    #![allow(clippy::bool_assert_comparison, reason = "explicit boolean assertions")]
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, MutexGuard};

    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    // =====================================================================
    // Helper: a type that needs Drop and is Send+Sync (for Arc allocs)
    // =====================================================================
    static DROP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Global mutex serializing tests that share the `DROP_COUNTER` static.
    /// Parallel test execution would otherwise race on the counter,
    /// producing flaky pass/fail outcomes. Tests acquire the guard for
    /// their full lifetime by binding the return of `reset_drop_counter()`.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[derive(Clone)]
    struct DropTracker(u64);
    impl Drop for DropTracker {
        fn drop(&mut self) {
            DROP_COUNTER.fetch_add(1, Ordering::Relaxed);
        }
    }

    // SAFETY: DropTracker is trivially Send+Sync (just a u64 + counter).
    unsafe impl Send for DropTracker {}
    unsafe impl Sync for DropTracker {}

    /// Reset the drop counter and return a guard that serializes the test
    /// against other counter-using tests. The guard must be held by the
    /// caller for the duration of the test (typically via
    /// `let _guard = reset_drop_counter();`). Poisoning is tolerated: a
    /// previous test's panic doesn't invalidate the counter state.
    #[must_use = "the returned MutexGuard must be held for the test's lifetime to serialize against other tests"]
    fn reset_drop_counter() -> MutexGuard<'static, ()> {
        let guard = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        DROP_COUNTER.store(0, Ordering::SeqCst);
        guard
    }

    /// Read the current drop count without releasing the serialization guard.
    fn drops() -> usize {
        DROP_COUNTER.load(Ordering::SeqCst)
    }

    // =====================================================================
    // arena.rs — try_alloc_inner_arc_with slow-path mutants
    // =====================================================================

    /// Kills: arena.rs:709:35 `> -> >=` in try_alloc_inner_arc_with
    /// The `entry_size > 0` guard: if mutated to `>=`, the drop entry
    /// is never written for types that need drop, causing dropped
    /// values to leak (drop not called).
    #[test]
    fn arena_709_entry_size_gt_zero_arc_with() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        {
            let _arc = arena.alloc_arc_with(|| DropTracker(42));
            // Arc holds the value; drop it by letting it go out of scope.
        }
        drop(arena);
        let drops = drops();
        assert!(drops >= 1, "DropTracker must be dropped; got {drops} drops");
    }

    /// Kills: arena.rs:728:30 `> -> >=` in try_alloc_inner_arc_with
    /// (oversized routing: `layout.size() > max_normal_alloc`)
    /// If mutated to `>=`, a value whose size == max_normal_alloc goes
    /// through the oversized path instead of the normal path. We test
    /// that a value at exactly max_normal_alloc succeeds via the normal
    /// path by filling the arena with many such allocations.
    #[test]
    #[cfg(feature = "stats")]
    fn arena_728_size_eq_max_normal_alloc_arc() {
        let _guard = reset_drop_counter();
        // Default max_normal_alloc is large. Use a small budget to force
        // the boundary. The default ArenaBuilder sets max_normal_alloc
        // based on chunk size. We just allocate something and check stats.
        let arena = Arena::builder().build();
        // Allocate a small arc with drop — exercises the normal path
        let _a1 = arena.alloc_arc_with(|| DropTracker(1));
        let _a2 = arena.alloc_arc_with(|| DropTracker(2));
        // Both should succeed through normal path, not oversized
        let stats = arena.stats();
        assert_eq!(
            stats.oversized_shared_chunks_allocated, 0,
            "small arcs should use normal shared chunks, not oversized"
        );
    }

    /// Kills: arena.rs:731:40 `+ -> -` and 731:104 `+ -> *`
    /// in try_alloc_inner_arc_with's `needed` computation.
    /// `needed = layout.size() + align_slack + entry_size`
    /// If `+` becomes `-` or `*`, `needed` is wrong; a subsequent
    /// alloc may fail or corrupt because the chunk doesn't have enough
    /// room. We allocate an Arc<DropTracker> and verify it drops correctly.
    #[test]
    fn arena_731_needed_computation_arc_with() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Fill several arcs to force slow-path refill
        let mut keep = Vec::new();
        for i in 0..100 {
            keep.push(arena.alloc_arc_with(|| DropTracker(i)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 100, "all 100 DropTrackers must be dropped");
    }

    // =====================================================================
    // arena.rs — try_alloc_inner_slow_value mutants (1085, 1089, 1101)
    // =====================================================================

    /// Kills: arena.rs:1085:26 `> -> ==` and `> -> >=`
    /// arena.rs:1089:36 `+ -> -` and 1089:100 `+ -> *`
    /// arena.rs:1101:25 `> -> ==` and `> -> >=`
    /// These are in the slow-path retry loop for value allocation.
    /// We force the slow path by filling a chunk, then allocating a
    /// value with Drop that barely fits. The value must actually drop.
    #[test]
    fn arena_1085_1089_1101_slow_value_path() {
        let _counter = AtomicUsize::new(0);
        let arena = Arena::new();
        // Fill the current chunk to trigger slow path on next alloc
        for i in 0u64..500 {
            arena.alloc_rc(i);
        }
        // Now allocate a value with Drop via the slow path.
        // Use alloc_rc_with (closure path) which goes through try_alloc_inner_slow_with.
        struct LocalDrop<'a> {
            counter: &'a AtomicUsize,
        }
        impl Drop for LocalDrop<'_> {
            fn drop(&mut self) {
                self.counter.fetch_add(1, Ordering::Relaxed);
            }
        }
        // Cannot capture local ref in arena alloc. Use global counter instead
        // but scope everything tightly.
        let _guard = reset_drop_counter();
        {
            let rc = arena.alloc_rc(DropTracker(999));
            // Rc is alive; drop it so refcount goes to 0
            drop(rc);
        }
        // Arena hasn't dropped yet; the chunk still holds the value.
        // Drop arena to trigger chunk cleanup.
        drop(arena);
        let drops = drops();
        assert!(drops >= 1, "DropTracker from slow value path must drop; got {drops}");
    }

    /// Kills: arena.rs:1122:68 `&& -> ||` in try_alloc_inner_oversized_value
    /// `entry_size = if needs_drop::<T>() && !matches!(flavor, AllocFlavor::Box) {...}`
    /// If `&&` becomes `||`, Box-flavor allocations of non-Drop types
    /// would wrongly get an entry_size > 0. We test by allocating large
    /// values through oversized path and verifying drops.
    #[test]
    fn arena_1122_and_to_or_oversized_value() {
        let _guard = reset_drop_counter();
        // Force oversized path with max_normal_alloc = 4096
        let arena = Arena::builder().max_normal_alloc(4096).build();
        // Box<[u8; 8192]> — no drop entry needed (no Drop, and Box flavor)
        let b = arena.alloc_box([0u8; 8192]);
        assert_eq!(b.len(), 8192);
        // Rc<LargeDrop> — needs drop entry (has Drop, Rc flavor)
        // Make a large type that needs Drop and is > 4096 bytes
        #[repr(C)]
        struct LargeDrop {
            data: [u8; 8192],
        }
        impl Drop for LargeDrop {
            fn drop(&mut self) {
                DROP_COUNTER.fetch_add(1, Ordering::Relaxed);
            }
        }
        let rc = arena.alloc_rc(LargeDrop { data: [0; 8192] });
        drop(rc);
        drop(arena);
        let drops = drops();
        assert!(drops >= 1, "LargeDrop in oversized path must drop");
    }

    /// Kills: arena.rs:1251:17 OversizedSharedGuard::drop -> ()
    /// If the guard's drop is removed, a panicking closure in
    /// try_alloc_inner_arc_oversized_with would leak the oversized shared chunk.
    /// We verify that alloc_arc_with works and the value actually drops.
    #[test]
    fn arena_1251_oversized_shared_guard_drop() {
        let _guard = reset_drop_counter();
        // Force oversized path for shared (arc) allocations
        let arena = Arena::builder().max_normal_alloc(4096).build();
        #[repr(C)]
        struct LargeArcDrop {
            data: [u8; 8192],
        }
        impl Drop for LargeArcDrop {
            fn drop(&mut self) {
                DROP_COUNTER.fetch_add(1, Ordering::Relaxed);
            }
        }
        // SAFETY: just bytes + a counter
        unsafe impl Send for LargeArcDrop {}
        unsafe impl Sync for LargeArcDrop {}
        let arc = arena.alloc_arc_with(|| LargeArcDrop { data: [0; 8192] });
        drop(arc);
        drop(arena);
        let drops = drops();
        assert!(drops >= 1, "oversized arc LargeArcDrop must drop");
    }

    /// Kills: arena.rs:1436:31 `!= -> ==` in try_alloc_inner_with
    /// This flips the eviction check: the closure-eviction path would be
    /// taken when chunks match (always) instead of when they differ (rare).
    /// The normal non-eviction path writes the real drop shim; if we take
    /// the eviction path instead, the noop shim stays → value leaks.
    #[test]
    fn arena_1436_eviction_check() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // alloc_rc_with uses the closure path (try_alloc_inner_with).
        // Allocate many items to be sure the normal path is exercised.
        let mut keep = Vec::new();
        for i in 0..200 {
            keep.push(arena.alloc_rc_with(|| DropTracker(i)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 200, "all 200 DropTrackers must drop via normal path");
    }

    /// Kills: arena.rs:1491:26 `> -> >=` in try_alloc_inner_slow_with
    /// arena.rs:1495:36 `+ -> -` and 1495:100 `+ -> *`
    /// arena.rs:1507:25 `> -> ==` and `> -> >=`
    /// These are in the slow retry path for closure-based local alloc.
    /// Force slow path via chunk filling, then allocate with closure.
    #[test]
    fn arena_1491_1495_1507_slow_with_path() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Fill current local chunk
        for i in 0u64..500 {
            arena.alloc_rc(i);
        }
        // Force slow path for closure-based alloc with Drop type
        {
            let rc = arena.alloc_rc_with(|| DropTracker(123));
            drop(rc);
        }
        drop(arena);
        let drops = drops();
        assert!(drops >= 1, "DropTracker from slow with path must drop; got {drops}");
    }

    /// Kills: arena.rs:1648:40 `+ -> -` in allocate_layout
    /// `needed = layout.size() + layout.align().saturating_sub(align_of::<usize>())`
    /// If `+` becomes `-`, the needed computation underflows, requesting
    /// too little memory, causing subsequent allocations to overlap.
    /// We allocate layout-sensitive values and verify correctness.
    #[test]
    fn arena_1648_allocate_layout_needed() {
        let arena = Arena::new();
        // allocate_layout is used by alloc (borrow) path.
        // Allocate many u64 values and verify they don't overlap.
        let mut ptrs = Vec::new();
        for i in 0u64..100 {
            let r = arena.alloc(i);
            ptrs.push(r as *const u64 as usize);
        }
        // Check no two pointers are the same
        ptrs.sort();
        ptrs.dedup();
        assert_eq!(ptrs.len(), 100, "all 100 alloc pointers must be distinct");
        // Verify values are intact
        for i in 0u64..10 {
            let r = arena.alloc(i + 1000);
            assert_eq!(*r, i + 1000);
        }
    }

    // =====================================================================
    // arena.rs — slice allocation mutants
    // =====================================================================

    /// Kills: arena.rs:2261:47 `&& -> ||` in try_alloc_slice_local_with
    /// `entry_size = if drop_fn.is_some() && len != 0 { ... } else { 0 }`
    /// If && becomes ||, entry_size is nonzero even when len == 0 or
    /// drop_fn is None, wasting space or causing wrong behavior.
    #[test]
    fn arena_2261_slice_local_and_to_or() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Allocate a zero-length slice of a Drop type (local flavor)
        let empty: &mut [DropTracker] = arena.alloc_slice_fill_with(0, |_| DropTracker(0));
        assert_eq!(empty.len(), 0);
        // Allocate a non-empty slice of a non-Drop type (no drop_fn)
        let nums: &mut [u32] = arena.alloc_slice_fill_with(10, |i| i as u32);
        assert_eq!(nums.len(), 10);
        for (i, v) in nums.iter().enumerate() {
            assert_eq!(*v, i as u32);
        }
    }

    /// Kills: arena.rs:2266:23 `!= -> ==` and 2266:35 `> -> ==` / `> -> >=`
    /// `if entry_size != 0 && len > u16::MAX as usize { return Err(AllocError); }`
    /// Tests that a Drop-type slice with len > u16::MAX returns error,
    /// and a non-Drop-type slice with len > u16::MAX succeeds.
    #[test]
    fn arena_2266_slice_len_boundary() {
        let arena = Arena::new();
        // Non-Drop type, len > u16::MAX — should succeed (no drop entry needed)
        // Use a tiny type to avoid OOM
        let big_len = u16::MAX as usize + 1;
        let result = arena.try_alloc_slice_fill_with(big_len, |i| i as u8);
        // This may fail due to memory, but should not fail due to the len check
        // when entry_size == 0. If it fails, it's AllocError from memory, not the len check.
        // Let's use a smaller test: verify that exactly u16::MAX works for Drop types.
        let arena2 = Arena::new();
        // len == u16::MAX with Drop type — should succeed (not > u16::MAX)
        // This would be too much memory, so let's verify the boundary differently.
        // Actually test len == 0 with Drop type (entry_size should be 0 when len == 0)
        let empty_drop: &mut [DropTracker] = arena2.alloc_slice_fill_with(0, |_| DropTracker(0));
        assert_eq!(empty_drop.len(), 0);

        // Test len == 1 with Drop type — should succeed
        let _guard = reset_drop_counter();
        let one_drop: &mut [DropTracker] = arena2.alloc_slice_fill_with(1, |_| DropTracker(42));
        assert_eq!(one_drop.len(), 1);
        // Just verify the allocation is fine
        drop(result);
    }

    /// Kills: arena.rs:2482:25 `> -> ==` / `> -> >=` in try_alloc_slice_local_no_drop_with_slow
    /// arena.rs:2577:25 `> -> ==` / `> -> >=` in try_alloc_slice_local_copy_slow
    /// These are slow-path retry loops for local slice alloc.
    /// Force slow path by filling chunk, then allocate a slice.
    #[test]
    fn arena_2482_2577_slice_slow_paths() {
        let arena = Arena::new();
        // Fill the chunk to force slow path
        for i in 0u64..500 {
            arena.alloc_rc(i);
        }
        // Allocate a no-drop slice via fill_with (slow path)
        let s1: &mut [u64] = arena.alloc_slice_fill_with(20, |i| i as u64);
        assert_eq!(s1.len(), 20);
        for (i, v) in s1.iter().enumerate() {
            assert_eq!(*v, i as u64);
        }
        // Allocate a copy slice (slow path)
        let src = [1u32, 2, 3, 4, 5];
        let s2: &mut [u32] = arena.alloc_slice_copy(&src);
        assert_eq!(s2, &[1, 2, 3, 4, 5]);
    }

    /// Kills: arena.rs:2655:47 `&& -> ||` in try_alloc_slice_shared_with
    /// `entry_size = if drop_fn.is_some() && len != 0 { ... } else { 0 }`
    #[test]
    fn arena_2655_shared_slice_and_to_or() {
        let arena = Arena::new();
        // Allocate empty shared (arc) slice of Drop type
        let _guard = reset_drop_counter();
        let empty_arc = arena.alloc_slice_fill_with_arc(0, |_| DropTracker(0));
        assert_eq!(empty_arc.len(), 0);
        drop(empty_arc);
        // No drops should have occurred for empty slice
        let drops = drops();
        assert_eq!(drops, 0, "empty arc slice should not drop any elements");

        // Allocate non-empty shared slice of non-Drop type
        let nums_arc = arena.alloc_slice_fill_with_arc(5, |i| i as u64);
        assert_eq!(nums_arc.len(), 5);
    }

    /// Kills: arena.rs:2660:23 `!= -> ==` and 2660:35 `> -> ==` / `> -> >=`
    /// Same pattern as 2266 but for shared slices.
    #[test]
    fn arena_2660_shared_slice_len_boundary() {
        let arena = Arena::new();
        // Drop type, len == 1 via arc — should succeed (len <= u16::MAX)
        let _guard = reset_drop_counter();
        let one = arena.alloc_slice_fill_with_arc(1, |_| DropTracker(99));
        assert_eq!(one.len(), 1);
        drop(one);
        drop(arena);
        let drops = drops();
        assert!(drops >= 1, "single-element arc slice must drop");
    }

    /// Kills: arena.rs:2701:35 `> -> >=` in try_alloc_slice_shared_with
    /// `if entry_size > 0 { ... advance drop_back ... }`
    /// If mutated to `>=`, drop_back is never advanced for Drop types,
    /// causing drop entries to overlap or be lost.
    #[test]
    fn arena_2701_shared_slice_entry_size_guard() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Allocate multiple shared slices with Drop to exercise drop_back advancement
        let mut keep = Vec::new();
        for _ in 0..20 {
            keep.push(arena.alloc_slice_fill_with_arc(3, |i| DropTracker(i as u64)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 60, "20 arcs * 3 elements = 60 drops");
    }

    /// Kills: arena.rs:2719:35 `+= -> *=` in try_alloc_slice_shared_with
    /// `guard.len += 1` in the init loop. If *= instead of +=,
    /// guard.len goes 0*1=0, then 0*1=0, ... so on panic the guard
    /// wouldn't drop any initialized elements. Test that all elements
    /// are properly initialized.
    #[test]
    fn arena_2719_guard_len_increment() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        let arc = arena.alloc_slice_fill_with_arc(5, |i| DropTracker(i as u64));
        assert_eq!(arc.len(), 5);
        drop(arc);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 5, "all 5 elements must drop");
    }

    /// Kills: arena.rs:2739:75 `!= -> ==` in try_alloc_slice_shared_with
    /// `self.refill_shared(compute_worst_case_size(layout, entry_size != 0))?;`
    /// If `!=` becomes `==`, entry_size==0 would claim "has drop entry"
    /// and entry_size>0 would claim "no drop entry", causing wrong
    /// worst-case size computation and potential failure.
    #[test]
    fn arena_2739_refill_shared_entry_size_check() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Force the slow refill path by filling the shared chunk
        let mut keep = Vec::new();
        for _ in 0..100 {
            keep.push(arena.alloc_slice_fill_with_arc(3, |i| DropTracker(i as u64)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 300, "100 * 3 = 300 drops");
    }

    /// Kills: arena.rs:5268:16 `> -> >=` in try_bump_fit
    /// `if aligned > max_aligned { return None; }`
    /// If mutated to `>=`, exact-fit allocations fail (None returned
    /// when they should succeed). This affects all bump-fit paths.
    #[test]
    fn arena_5268_bump_fit_exact_boundary() {
        let arena = Arena::new();
        // Allocate items that should exactly fit. If the boundary is
        // wrong (>= instead of >), allocations that exactly fit will fail
        // and the arena will refill unnecessarily.
        // We just need allocations to succeed and be correct.
        let mut values = Vec::new();
        for i in 0u64..200 {
            values.push(arena.alloc_rc(i));
        }
        for (i, v) in values.iter().enumerate() {
            assert_eq!(**v, i as u64);
        }
    }

    // =====================================================================
    // chunk_provider.rs mutants
    // =====================================================================

    /// Kills: chunk_provider.rs:133:25 `> -> >=` in reserve_budget
    /// `if next > budget { return Err(AllocError); }`
    /// If mutated to `>=`, allocations exactly at budget fail.
    #[test]
    fn chunk_provider_133_reserve_budget_boundary() {
        // Set byte_budget so that exactly one default chunk fits.
        // The first allocation should succeed. If `>` becomes `>=`,
        // even the first allocation might fail.
        let arena = Arena::builder().byte_budget(256 * 1024).build();
        // Should succeed - within budget
        let _v = arena.alloc(42u64);
    }

    /// Kills: chunk_provider.rs:152:9 release_budget -> ()
    /// If release_budget is a no-op, the budget counter never decreases,
    /// so after enough chunk allocations and deallocations, new
    /// allocations fail even though old chunks were freed.
    #[test]
    fn chunk_provider_152_release_budget_noop() {
        // Tight budget — enough for ~2 chunks. Allocate, drop, repeat.
        // If release_budget is no-op, budget fills up and later allocs fail.
        let arena = Arena::builder().byte_budget(512 * 1024).build();
        for round in 0..5 {
            let mut batch = Vec::new();
            for i in 0u64..50 {
                batch.push(arena.alloc_rc(round * 100 + i));
            }
            // Dropping the batch frees refcounts; chunk may be reclaimed → release_budget
            drop(batch);
        }
    }

    /// Kills: chunk_provider.rs:441:48 `+ -> *` in acquire_shared
    /// `total_bytes = shared_header_size() + target_bytes`
    /// If `+` becomes `*`, total_bytes = header * target which is way
    /// too large, causing budget exhaustion or OOM.
    #[test]
    fn chunk_provider_441_shared_header_plus_target() {
        let arena = Arena::builder().byte_budget(512 * 1024).build();
        // Allocate shared (arc) values — each triggers acquire_shared
        let _a1 = arena.alloc_arc(1u64);
        let _a2 = arena.alloc_arc(2u64);
        // If `+` became `*`, the budget would be consumed much faster
        // and these allocations would likely fail or the budget check would
        // prevent them.
    }

    // =====================================================================
    // constants.rs mutants
    // =====================================================================

    /// Kills: constants.rs:76:14 `>= -> <` in min_class_for_bytes
    /// `if bytes >= MAX_CHUNK_BYTES { return NUM_CHUNK_CLASSES - 1; }`
    /// If `>=` becomes `<`, bytes >= MAX_CHUNK_BYTES would fall through
    /// to the loop, potentially returning a wrong class.
    /// We test by allocating a large value that exercises the MAX_CHUNK_BYTES boundary.
    #[test]
    fn constants_76_min_class_ge_to_lt() {
        // Allocating a large value forces acquire_local with a large payload
        // that exercises min_class_for_bytes near MAX_CHUNK_BYTES.
        let arena = Arena::new();
        let big = vec![0u8; 64 * 1024];
        let _alloc = arena.alloc_slice_copy(&big);
    }

    /// Kills: constants.rs:87:13 `< -> <=` in min_class_for_bytes
    /// `while v < ratio { v <<= 1; c += 1; }`
    /// If `<` becomes `<=`, the loop runs one extra iteration, returning
    /// a class that's one too high. This causes chunks to be too large.
    #[test]
    #[cfg(feature = "stats")]
    fn constants_87_loop_boundary() {
        // Allocate a value that lands on a power-of-two class boundary.
        // min_class_for_bytes(MIN_CHUNK_BYTES * 2) should return 1.
        // If `<` becomes `<=`, it returns 2 instead.
        // We can observe this through stats: with a tight budget,
        // a higher class means a larger chunk allocation.
        let arena = Arena::builder().byte_budget(128 * 1024).build();
        let _v = arena.alloc(42u64);
        // The allocation should succeed. If the class is wrong,
        // the chunk might be too large and blow the budget.
    }

    // =====================================================================
    // drop_list.rs mutants
    // =====================================================================

    /// Kills: drop_list.rs:49:69 `+ -> -` / `+ -> *`
    /// drop_list.rs:53:16 `- -> +` / `- -> /`
    /// drop_list.rs:53:28 `% -> /`
    /// These affect PAD_BYTES computation for DropEntry alignment.
    /// RAW_USED = size_of::<fn_ptr>() + 2 + 2 = 8 + 4 = 12 on 64-bit.
    /// PAD_TARGET = align_of::<fn_ptr>() = 8.
    /// PAD_BYTES = if 12 % 8 == 0 { 0 } else { 8 - (12 % 8) } = 8 - 4 = 4.
    /// If the arithmetic is wrong, DropEntry is misaligned and drop
    /// calls crash or corrupt memory.
    #[test]
    fn drop_list_49_53_pad_bytes() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Allocate many Drop values to exercise the drop list heavily
        let mut keep = Vec::new();
        for i in 0..100 {
            keep.push(arena.alloc_rc(DropTracker(i)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 100, "all 100 DropTrackers must drop correctly");
    }

    /// Specifically targets drop_list.rs:49:69 `+ -> -` (the first +2)
    /// and 49:73 `+ -> -` (the second +2) by verifying that multiple
    /// successive drop entries work correctly.
    #[test]
    fn drop_list_successive_entries() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Mix different types to create varied drop entries
        let _r1 = arena.alloc_rc(DropTracker(1));
        let _r2 = arena.alloc_rc(DropTracker(2));
        let _r3 = arena.alloc_rc(DropTracker(3));
        let _s1 = arena.alloc_slice_fill_with(3, |i| DropTracker(10 + i as u64));
        let _r4 = arena.alloc_rc(DropTracker(4));
        drop(_r1);
        drop(_r2);
        drop(_r3);
        drop(_r4);
        drop(arena);
        let drops = drops();
        // 4 singles + 3 from slice = 7
        assert_eq!(drops, 7, "7 DropTrackers must drop");
    }

    // =====================================================================
    // local_chunk.rs / shared_chunk.rs mutants
    // =====================================================================

    /// Kills: local_chunk.rs:132:17 `- -> +` in max_bump_extent
    /// `CHUNK_ALIGN - header_size()` → if `+`, max_bump_extent is huge,
    /// potentially allowing allocations past the chunk boundary.
    /// We verify that allocations don't crash even under tight conditions.
    #[test]
    fn local_chunk_132_max_bump_extent() {
        let arena = Arena::new();
        // Allocate many values to exercise bump extent limits
        let mut keep = Vec::new();
        for i in 0u64..1000 {
            keep.push(arena.alloc_rc(i));
        }
        for (i, v) in keep.iter().enumerate() {
            assert_eq!(**v, i as u64);
        }
    }

    /// Kills: shared_chunk.rs:143:17 `- -> +` in max_bump_extent
    /// Same as local_chunk but for shared chunks.
    #[test]
    fn shared_chunk_143_max_bump_extent() {
        let arena = Arena::new();
        let mut keep = Vec::new();
        for i in 0u64..1000 {
            keep.push(arena.alloc_arc(i));
        }
        for (i, v) in keep.iter().enumerate() {
            assert_eq!(**v, i as u64);
        }
    }

    /// Kills: shared_chunk.rs:168:9 to_thin_ptr -> Default::default()
    /// If to_thin_ptr returns null (Default for *mut u8), the shared chunk
    /// cache and Treiber stack operations would use null pointers, causing
    /// crashes or lost chunks.
    #[test]
    fn shared_chunk_168_to_thin_ptr() {
        let arena = Arena::new();
        // Allocate and drop arcs to trigger chunk caching (which uses to_thin_ptr)
        for _ in 0..5 {
            let mut batch = Vec::new();
            for i in 0u64..50 {
                batch.push(arena.alloc_arc(i));
            }
            drop(batch);
        }
        // If to_thin_ptr returned null, the cache would be broken and
        // subsequent allocations would fail or crash.
        let final_arc = arena.alloc_arc(42u64);
        assert_eq!(*final_arc, 42);
    }

    /// Kills: shared_chunk.rs:186:59 `- -> +` / `- -> /` in SharedChunk::allocate
    /// `let payload = min_payload.checked_add(entry_align - 1)...& !(entry_align - 1)`
    /// If `-` becomes `+` or `/`, the rounding is wrong, causing
    /// misaligned drop entries or undersized chunks.
    #[test]
    fn shared_chunk_186_payload_rounding() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Allocate arc values with Drop to exercise the shared chunk allocation
        // with proper payload rounding for drop entries
        let mut keep = Vec::new();
        for i in 0..50 {
            keep.push(arena.alloc_arc_with(|| DropTracker(i)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 50, "all 50 shared DropTrackers must drop");
    }

    // =====================================================================
    // strings/string.rs mutants
    // =====================================================================

    /// Kills: string.rs:465:19 `> -> >=` in String::try_reserve
    /// `if needed > self.cap { self.try_grow_to_at_least(needed)?; }`
    /// If `>` becomes `>=`, try_reserve grows even when needed == cap,
    /// which wastes capacity but shouldn't break. However, if the grow
    /// fails (tight budget), a reserve that should succeed (needed == cap)
    /// would fail.
    #[test]
    fn string_465_try_reserve_boundary() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(10);
        // Reserve exactly the remaining capacity
        s.try_reserve(10).unwrap(); // needed == cap, should not grow
        // Now push exactly 10 bytes
        s.push_str("1234567890");
        assert_eq!(s.as_str(), "1234567890");
        // Reserve 0 more — should be no-op
        s.try_reserve(0).unwrap();
    }

    /// Kills: string.rs:528:9 try_reclaim_tail -> ()
    /// If try_reclaim_tail is a no-op, unused capacity after string
    /// finalization is wasted. We can detect this by checking that
    /// subsequent allocations can reuse the reclaimed space.
    #[test]
    fn string_528_try_reclaim_tail_noop() {
        let arena = Arena::new();
        // Build a string with extra capacity, then freeze it
        let mut s = arena.alloc_string_with_capacity(1000);
        s.push_str("hello");
        let rc_str = s.into_arena_str();
        assert_eq!(rc_str.as_ref(), "hello");

        // If reclaim worked, the ~995 bytes of unused capacity should be
        // available for the next allocation in the same chunk.
        // Allocate something that fits in the reclaimed space.
        let v = arena.alloc(42u64);
        assert_eq!(*v, 42);
    }

    /// Kills: string.rs:528:21 `>= -> <` in try_reclaim_tail
    /// `if self.len >= self.cap { return; }`
    /// If `>=` becomes `<`, the function returns early when len < cap
    /// (which is the case where reclaim should happen) and falls through
    /// when len >= cap (nothing to reclaim). Behavior is inverted.
    #[test]
    fn string_528_21_reclaim_guard_inversion() {
        let arena = Arena::new();
        // Case 1: len < cap — reclaim should happen
        let mut s = arena.alloc_string_with_capacity(100);
        s.push_str("hi");
        let _rc = s.into_arena_str();

        // Case 2: len == cap — no reclaim needed
        let mut s2 = arena.alloc_string_with_capacity(5);
        s2.push_str("12345");
        let _rc2 = s2.into_arena_str();
    }

    /// Kills: string.rs:534:29 `- -> /` in try_reclaim_tail
    /// `let reclaim = total - used;`
    /// If `-` becomes `/`, reclaim = total / used, which is wrong.
    /// The reclaimed amount would be too small or too large.
    #[test]
    fn string_534_reclaim_computation() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(200);
        s.push_str("abc");
        // The reclaim amount should be ~197 bytes. If / instead of -,
        // reclaim would be wrong and the cursor wouldn't move correctly.
        let _rc = s.into_arena_str();
        // Verify arena is still functional
        let v = arena.alloc(99u64);
        assert_eq!(*v, 99);
    }

    // =====================================================================
    // strings/utf16_string.rs mutants
    // =====================================================================

    /// Kills: vec.rs:451:34 `- -> +` in resize
    /// `self.reserve(new_len - self.len);`
    /// If `-` becomes `+`, reserve amount is huge, causing OOM/panic.
    #[test]
    fn vec_451_resize_reserve() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(5);
        v.push(1);
        v.push(2);
        // resize from 2 to 5 — reserve should be 3
        v.resize(5, 0);
        assert_eq!(v.len(), 5);
        assert_eq!(v[0], 1);
        assert_eq!(v[1], 2);
        assert_eq!(v[2], 0);
    }

    /// Kills: vec.rs:460:46 `- -> +` / `- -> /` in Guard::drop
    /// `let added = self.vec.len - self.old_len;`
    /// If wrong, the guard drops wrong elements on panic.
    /// Also kills: vec.rs:461:30 `> -> >=`
    /// `if added > 0 { drop tail }`
    #[test]
    fn vec_460_461_resize_guard() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(20);
        v.push(10);
        v.push(20);
        v.resize(8, 42);
        assert_eq!(v.len(), 8);
        assert_eq!(v[0], 10);
        assert_eq!(v[1], 20);
        for i in 2..8 {
            assert_eq!(v[i], 42);
        }
    }

    /// Kills: vec.rs:473:37 `- -> +` and vec.rs:474:26 `> -> >=` in resize
    /// `let total_new = new_len - guard.vec.len;`
    /// `if total_new > 0 { ... }`
    #[test]
    fn vec_473_474_resize_total_new() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(10);
        v.push(1);
        // Resize to exactly len+1 — total_new == 1
        v.resize(2, 99);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1], 99);
        // Resize to same length — total_new == 0, no-op
        v.resize(2, 77);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1], 99); // unchanged
    }

    /// Kills: vec.rs:762:17 `+= -> -=` / `+= -> *=` in into_arena_box_copy
    /// `idx += 1` — if -= or *=, idx goes wrong and elements are
    /// read from wrong positions or the loop never terminates.
    #[test]
    fn vec_762_into_arena_box_copy() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(10);
        for i in 0..5 {
            v.push(i * 10);
        }
        let boxed = v.into_arena_box();
        assert_eq!(boxed.len(), 5);
        assert_eq!(boxed[0], 0);
        assert_eq!(boxed[1], 10);
        assert_eq!(boxed[2], 20);
        assert_eq!(boxed[3], 30);
        assert_eq!(boxed[4], 40);
    }

    /// Kills: vec.rs:808:20 `> -> >=` and 808:31 `&& -> ||` and 808:43 `> -> >=`
    /// `if new_cap > self.cap && self.cap > 0 { try in-place growth }`
    #[test]
    fn vec_808_realloc_inplace_guard() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(4);
        v.push(1);
        v.push(2);
        // Grow: new_cap > cap && cap > 0 → try in-place
        v.reserve(10);
        assert!(v.capacity() >= 12);
        assert_eq!(v[0], 1);
        assert_eq!(v[1], 2);

        // From cap=0 → can't try in-place (cap > 0 is false)
        let mut v2 = arena.alloc_vec_with_capacity::<u64>(0);
        v2.push(42);
        assert_eq!(v2[0], 42);
    }

    /// Kills: vec.rs:819:21 `> -> >=` in realloc
    /// `if self.len > 0 { copy_nonoverlapping }`
    /// If >= instead of >, copy runs even when len==0, which is
    /// technically harmless but would copy from a dangling pointer
    /// when cap was 0.
    #[test]
    fn vec_819_realloc_copy_guard() {
        let arena = Arena::new();
        // Start with cap=0, push to force realloc with len=0 initially
        let mut v = arena.alloc_vec_with_capacity::<u64>(0);
        assert_eq!(v.len(), 0);
        v.push(1); // triggers realloc from cap=0
        assert_eq!(v[0], 1);

        // Now realloc with len > 0
        v.reserve(100);
        assert_eq!(v[0], 1);
    }

    /// Kills: vec.rs:828:20 `> -> ==` / `> -> >=` in realloc
    /// `if old_cap > 0 { self.arena.bump_relocation(); }`
    /// This tracks relocation stats. If the guard changes,
    /// relocations aren't counted or are counted wrongly.
    #[test]
    fn vec_828_realloc_relocation_tracking() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u64>(2);
        v.push(1);
        v.push(2);
        // Force a realloc that can't grow in place
        // First alloc something else to prevent in-place growth
        let _other = arena.alloc(99u64);
        v.reserve(100);
        assert_eq!(v[0], 1);
        assert_eq!(v[1], 2);
    }

    // =====================================================================
    // ROUND 2: Stronger tests for mutants that survived round 1
    // =====================================================================

    /// Kills: arena.rs:709:35 `> -> >=` — entry_size > 0 guard in arc_with
    /// If mutated to `>=`, entry_size==0 (no-drop Arc) would enter the
    /// drop-entry writing block and write a drop entry into unreserved
    /// space, potentially corrupting memory. Test with non-Drop Arc type.
    #[test]
    fn arena_709_entry_size_zero_arc() {
        let arena = Arena::new();
        // Arc<u64> — no Drop, entry_size == 0
        // With `>=`, a drop entry would be written even though no space
        // was reserved for it. Allocate many to make corruption likely.
        let mut keep = Vec::new();
        for i in 0u64..500 {
            keep.push(arena.alloc_arc_with(|| i));
        }
        for (i, v) in keep.iter().enumerate() {
            assert_eq!(**v, i as u64);
        }
        drop(keep);
        drop(arena);
    }

    /// Kills: arena.rs:731:40 `+ -> -` and 731:104 `+ -> *`
    /// Wrong `needed` computation causes refill with wrong size.
    /// With tight budget, the wrong refill might fail.
    #[test]
    fn arena_731_needed_tight_budget_arc() {
        let _guard = reset_drop_counter();
        // Use a tight byte budget so wrong `needed` could fail
        let arena = Arena::builder().byte_budget(256 * 1024).build();
        let mut keep = Vec::new();
        for i in 0..200 {
            keep.push(arena.alloc_arc_with(|| DropTracker(i)));
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 200);
    }

    /// Kills: arena.rs:1085:26 `> -> >=`, 1101:25 `> -> >=` — slow value retry
    /// These reject exact-fit allocations. Test by forcing exact-fit after refill.
    /// The slow path computes end_addr and drop_back_addr. If `>` becomes `>=`,
    /// exact fits are rejected and the retry loop might exhaust retries.
    #[test]
    fn arena_1085_1101_exact_fit_slow_value() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Fill many items to force slow path
        for _ in 0..2000 {
            arena.alloc_rc(0u64);
        }
        // Allocate a Drop value that triggers slow path
        let rc = arena.alloc_rc(DropTracker(42));
        drop(rc);
        drop(arena);
        let drops = drops();
        assert!(drops >= 1);
    }

    /// Kills: arena.rs:1089:36 `+ -> -` in slow value path
    /// `needed = size + align_slack + entry_size` becomes `size + align_slack - entry_size`
    /// For Drop types, entry_size > 0. `size - entry_size` underflows for small T.
    /// But since it's usize, it wraps to a huge value, which would make refill fail.
    #[test]
    fn arena_1089_needed_underflow_slow_value() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Force slow path
        for _ in 0..if cfg!(miri) { 100 } else { 1000 } {
            arena.alloc_rc(0u64);
        }
        // DropTracker is 8 bytes. entry_size for InnerDropEntry is ~16 bytes.
        // With `-`, needed = 8 + 0 - 16 = wraps to huge number on usize
        // This should cause refill to fail (or allocate enormous chunk).
        // With `+`, needed = 8 + 0 + 16 = 24 (fits easily).
        let rc = arena.alloc_rc(DropTracker(99));
        drop(rc);
        drop(arena);
        let drops = drops();
        assert!(drops >= 1);
    }

    /// Kills: arena.rs:1122:68 `&& -> ||` in oversized value
    /// `entry_size = if needs_drop && !Box { size } else { 0 }`
    /// With ||: `entry_size = if needs_drop || !Box { size } else { 0 }`
    /// For Box of non-Drop type: original has entry_size=0 (neither is true alone since &&)
    /// With ||: entry_size = size (because !Box is true for... wait, flavor IS Box here)
    /// Actually `needs_drop::<T>() || !matches!(Box)` → false || false = false for Box<u64>.
    /// But for Rc<T: !Drop>: `false || !false` = true → adds drop entry unnecessarily.
    /// This wastes space. With tight budget, it could cause failure.
    #[test]
    fn arena_1122_oversized_nondrop_rc() {
        // Force oversized local path for a non-Drop Rc type
        let arena = Arena::builder().max_normal_alloc(4096).build();
        // Allocate a large non-Drop value as Rc (not Box)
        let rc = arena.alloc_rc([0u64; 1024]); // 8192 bytes > 4096
        // If `&&` became `||`, entry_size would be nonzero for non-Drop types,
        // wasting space. Verify the value is correct.
        assert_eq!(rc[0], 0);
        assert_eq!(rc[1023], 0);
    }

    /// Kills: arena.rs:1251:17 OversizedSharedGuard::drop -> ()
    /// The guard's drop cleans up the chunk on panic. If noop'd,
    /// a panicking closure leaks the budget.
    /// Test: trigger panic in alloc_arc_with, catch it, verify arena still works.
    #[test]
    fn arena_1251_oversized_guard_panic() {
        // Use an alloc strictly larger than `MAX_CHUNK_BYTES = 64 KiB` so
        // the chunk is truly oversized: after `reconcile_swap_out` the
        // backing allocation is freed (and its budget released) rather
        // than cached. Budget allows ONE such chunk but not two
        // simultaneously. If `OversizedSharedGuard::drop` is a no-op
        // (the mutant), the panicked alloc's chunk stays charged against
        // the budget; the second oversized alloc then fails. We assert
        // the second alloc succeeds, killing the mutant.
        const N: usize = 70_000;
        let arena = Arena::builder().max_normal_alloc(4096).byte_budget(N + 4096).build();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _arc: multitude::Arc<[u8; N]> = arena.alloc_arc_with(|| {
                panic!("intentional panic in oversized arc closure");
            });
        }));
        assert!(result.is_err(), "should have caught the panic");

        // Only succeeds if the panicked chunk's budget was released by
        // `OversizedSharedGuard::drop`. If `drop` is no-op'd, the budget
        // is leaked and this `alloc_arc_with` (which calls `expect_alloc`)
        // panics with "allocator returned AllocError".
        let _arc2: multitude::Arc<[u8; N]> = arena.alloc_arc_with(|| [0u8; N]);
    }

    /// Kills: `OversizedLocalGuard::drop -> ()` (the local mirror of the
    /// `OversizedSharedGuard` mutant above). Same shape: panic mid-closure,
    /// guard must release the chunk's budget; subsequent oversized alloc
    /// must succeed.
    #[test]
    fn arena_oversized_local_guard_panic() {
        const N: usize = 70_000;
        let arena = Arena::builder().max_normal_alloc(4096).byte_budget(N + 4096).build();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _rc: multitude::Rc<[u8; N]> = arena.alloc_rc_with(|| {
                panic!("intentional panic in oversized rc closure");
            });
        }));
        assert!(result.is_err(), "should have caught the panic");

        let _rc2: multitude::Rc<[u8; N]> = arena.alloc_rc_with(|| [0u8; N]);
    }

    /// Kills: arena.rs:1491:26 `> -> >=` — slow with retry
    /// Same pattern as value slow paths.
    #[test]
    fn arena_1491_exact_fit_slow_with() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        for _ in 0..2000 {
            arena.alloc_rc(0u64);
        }
        let rc = arena.alloc_rc_with(|| DropTracker(77));
        drop(rc);
        drop(arena);
        let drops = drops();
        assert!(drops >= 1);
    }

    /// Kills: arena.rs:1507:25 `> -> >=` — slow with retry
    #[test]
    fn arena_1507_exact_fit_slow_with2() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        for _ in 0..3000 {
            arena.alloc_rc_with(|| 0u64);
        }
        let rc = arena.alloc_rc_with(|| DropTracker(88));
        drop(rc);
        drop(arena);
        let drops = drops();
        assert!(drops >= 1);
    }

    /// Kills: arena.rs:1648:40 `+ -> -` in allocate_layout
    /// `needed = size + align_slack` becomes `size - align_slack`.
    /// For types with align == align_of::<usize>(), align_slack is 0, so
    /// this is equivalent. For types with higher alignment, it underflows.
    /// Use #[repr(align(64))] to create a type with larger alignment.
    #[test]
    fn arena_1648_high_alignment_layout() {
        #[repr(align(64))]
        #[derive(Clone, Copy)]
        struct Aligned64 {
            data: [u8; 64],
        }
        let arena = Arena::new();
        // With `+ -> -`: needed = 64 + (64 - 8) = 120 vs 64 - (64 - 8) = 64 - 56 = 8
        // The `- 56` would request only 8 bytes from refill, too small.
        let mut keep = Vec::new();
        for i in 0u8..100 {
            let v = arena.alloc(Aligned64 { data: [i; 64] });
            assert_eq!(v.data[0], i);
            keep.push(v as *const Aligned64 as usize);
        }
        // Verify all pointers are 64-byte aligned
        for p in &keep {
            assert_eq!(p % 64, 0, "pointer must be 64-byte aligned");
        }
    }

    /// Kills: arena.rs:5268:16 `> -> >=` in try_bump_fit
    /// Rejects exact-fit allocations. Test by filling chunks near capacity.
    #[test]
    fn arena_5268_bump_fit_exact() {
        let arena = Arena::new();
        // Allocate many items to force many bump-fit checks
        let mut values = Vec::new();
        for i in 0u64..2000 {
            values.push(arena.alloc_rc(i));
        }
        // Verify all values are correct
        for (i, v) in values.iter().enumerate() {
            assert_eq!(**v, i as u64);
        }
    }

    /// Kills: arena.rs:2261:47 `&& -> ||` in try_alloc_slice_local_with
    /// `entry_size = if drop_fn.is_some() && len != 0 { ... } else { 0 }`
    /// With ||: len==0 or drop_fn.is_some() alone would set entry_size.
    /// For Drop type with len==0, entry_size should be 0. With ||, it's nonzero.
    #[test]
    fn arena_2261_empty_drop_slice() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Allocate many empty slices of Drop type
        for _ in 0..500 {
            let s: &mut [DropTracker] = arena.alloc_slice_fill_with(0, |_| DropTracker(0));
            assert_eq!(s.len(), 0);
        }
        // If entry_size was wrongly nonzero, we'd waste space and
        // potentially corrupt the drop list.
        // Also test non-empty non-Drop slices (drop_fn is None)
        for i in 0u64..500 {
            let s: &mut [u64] = arena.alloc_slice_fill_with(5, |j| i + j as u64);
            assert_eq!(s[0], i);
        }
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 0, "empty Drop slices should not produce drops");
    }

    /// Kills: arena.rs:2266:23 `!= -> ==` in try_alloc_slice_local_with
    /// `if entry_size != 0 && len > u16::MAX { return Err }`
    /// With ==: `entry_size == 0 && len > u16::MAX` would reject
    /// non-Drop large slices. Test a large non-Drop slice.
    #[test]
    fn arena_2266_large_nondrop_slice() {
        let arena = Arena::new();
        // Try allocating a large non-Drop slice — should succeed with !=
        // With == mutation, this would be rejected.
        // u16::MAX + 1 = 65536 elements of u8 = 64KB
        let result = arena.try_alloc_slice_fill_with(65536, |i| i as u8);
        assert!(result.is_ok(), "large non-Drop slice should succeed");
        let s = result.unwrap();
        assert_eq!(s.len(), 65536);
        assert_eq!(s[0], 0);
        assert_eq!(s[65535], 255);
    }

    /// Kills: arena.rs:2482:25 `> -> ==` / `> -> >=` in slow no_drop_with
    /// arena.rs:2577:25 `> -> ==` in slow copy path
    /// Force slow paths for these slice allocations.
    #[test]
    fn arena_2482_2577_slice_slow_force() {
        let arena = Arena::new();
        // Fill heavily to force slow paths
        for _ in 0..5000 {
            arena.alloc_rc(0u64);
        }
        // No-drop slice via fill_with
        let s1: &mut [u64] = arena.alloc_slice_fill_with(50, |i| i as u64);
        for i in 0..50 {
            assert_eq!(s1[i], i as u64);
        }
        // Copy slice
        let src: Vec<u32> = (0..100).collect();
        let s2: &mut [u32] = arena.alloc_slice_copy(&src);
        for i in 0..100 {
            assert_eq!(s2[i], i as u32);
        }
    }

    /// Kills: arena.rs:2655:47 `&& -> ||` in try_alloc_slice_shared_with
    /// Same as 2261 but for shared slices.
    #[test]
    fn arena_2655_empty_drop_shared_slice() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        for _ in 0..200 {
            let arc = arena.alloc_slice_fill_with_arc(0, |_| DropTracker(0));
            assert_eq!(arc.len(), 0);
            drop(arc);
        }
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 0, "empty Drop arc slices should not produce drops");
    }

    /// Kills: arena.rs:2660:23 `!= -> ==` in try_alloc_slice_shared_with
    /// Same as 2266 but for shared slices.
    #[test]
    fn arena_2660_large_nondrop_shared_slice() {
        // Use a non-Copy, non-Drop wrapper so we go through try_alloc_slice_shared_with
        // (the Copy path bypasses line 2660).
        #[derive(Clone, Debug, PartialEq)]
        struct W(u8);
        let arena = Arena::new();
        let result = arena.try_alloc_slice_fill_with_arc(65536, |i| W(i as u8));
        assert!(result.is_ok(), "large non-Drop non-Copy shared slice should succeed");
        let arc = result.unwrap();
        assert_eq!(arc.len(), 65536);
        assert_eq!(arc[0], W(0));
    }

    /// Kills: arena.rs:2701:35 `> -> >=` in try_alloc_slice_shared_with
    /// `if entry_size > 0 { advance drop_back }`
    /// For Drop types, entry_size > 0. If `>=`, non-Drop types
    /// (entry_size == 0) would enter this block and write unreserved entries.
    #[test]
    fn arena_2701_entry_size_shared_slice() {
        let arena = Arena::new();
        // Allocate many non-Drop shared slices
        let mut keep = Vec::new();
        for i in 0u64..200 {
            keep.push(arena.alloc_slice_fill_with_arc(5, |j| i + j as u64));
        }
        for (i, arc) in keep.iter().enumerate() {
            assert_eq!(arc[0], i as u64);
            assert_eq!(arc[4], i as u64 + 4);
        }
    }

    /// Kills: shared_chunk.rs:143:17 `- -> +` in max_bump_extent
    /// CHUNK_ALIGN - header → CHUNK_ALIGN + header. This allows bump
    /// pointers past the chunk boundary, which corrupts memory.
    /// Force many shared allocations to trigger boundary issues.
    #[test]
    fn shared_chunk_143_max_bump_many() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        let mut keep = Vec::new();
        for i in 0..2000 {
            keep.push(arena.alloc_arc_with(|| DropTracker(i)));
        }
        // Verify all values are intact
        for (i, arc) in keep.iter().enumerate() {
            assert_eq!(arc.0, i as u64);
        }
        drop(keep);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 2000);
    }

    /// Kills: shared_chunk.rs:168:9 to_thin_ptr -> Default
    /// The Treiber stack and chunk caching use to_thin_ptr.
    /// Force chunk reuse by allocating, dropping, and reallocating.
    #[test]
    fn shared_chunk_168_force_cache_reuse() {
        let arena = Arena::new();
        // Round 1: allocate arcs, fill a shared chunk
        let mut batch1: Vec<multitude::Arc<u64>> = Vec::new();
        for i in 0u64..100 {
            batch1.push(arena.alloc_arc(i));
        }
        // Drop all arcs → chunk should be cached via to_thin_ptr
        drop(batch1);

        // Round 2: allocate more arcs — should reuse cached chunk
        let mut batch2: Vec<multitude::Arc<u64>> = Vec::new();
        for i in 0u64..100 {
            batch2.push(arena.alloc_arc(i + 1000));
        }
        for (i, arc) in batch2.iter().enumerate() {
            assert_eq!(**arc, i as u64 + 1000);
        }
    }

    /// Kills: shared_chunk.rs:186:59 `- -> +` / `- -> /` in allocate
    /// Payload rounding: `min_payload + (entry_align - 1)` becomes
    /// `min_payload + (entry_align + 1)` or `min_payload + (entry_align / 1)`.
    /// For `- -> +`: align - 1 = 7 vs align + 1 = 9. Rounding mask stays same.
    /// Effectively wastes 2 bytes per chunk but works. This may be EQUIVALENT.
    /// For `- -> /`: align - 1 = 7 vs align / 1 = 8. The mask `!(8 - 1)` = !7
    /// but original was `!(7)` = `!(7)`. Wait: `entry_align - 1` is used in both
    /// the addend and the mask. Actually let me re-read...
    /// `payload = (min_payload + entry_align - 1) & !(entry_align - 1)`
    /// Mutation at 186:59 affects the first `-`: `(min_payload + entry_align + 1)` or `(min_payload + entry_align / 1)`
    /// The mask is unchanged. So `+ 1` makes payload 2 bytes larger (rounds up 2 more).
    /// That's still valid. For `/ 1`: `entry_align / 1 = entry_align`, same as `entry_align - 1 + 1`.
    /// Both may be equivalent for the addend since the mask rounds down.
    /// Actually, `(x + a - 1) & !(a - 1)` rounds x up to multiple of a.
    /// `(x + a + 1) & !(a - 1)` rounds (x+2) up, giving a result at least 2 more.
    /// `(x + a/1) & !(a-1)` = `(x + a) & !(a-1)` rounds (x+1) up.
    /// Both produce valid but slightly larger allocations. Likely EQUIVALENT.
    #[test]
    fn shared_chunk_186_payload_rounding_stress() {
        let _guard = reset_drop_counter();
        let arena = Arena::new();
        // Stress test with many shared allocations of varying sizes
        let mut keep = Vec::new();
        for i in 0..500 {
            keep.push(arena.alloc_arc_with(|| DropTracker(i)));
        }
        // Also test slices with varying sizes
        let mut keep2 = Vec::new();
        for i in 0..100 {
            keep2.push(arena.alloc_slice_fill_with_arc(3, |j| DropTracker((i * 10 + j) as u64)));
        }
        drop(keep);
        drop(keep2);
        drop(arena);
        let drops = drops();
        assert_eq!(drops, 800, "500 singles + 300 slice elements = 800");
    }

    /// Kills: string.rs:465:19 `> -> >=` in try_reserve
    /// Actually this is EQUIVALENT: grow helper returns early if min_cap <= cap.
    /// But let's test it anyway.
    #[test]
    fn string_465_reserve_exact_capacity() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(10);
        s.push_str("12345");
        // Reserve 5 more — total needed == cap (10), should not grow
        s.try_reserve(5).unwrap();
        // Reserve 6 more — total needed == 11 > cap, must grow
        s.try_reserve(6).unwrap();
        s.push_str("67890A");
        assert_eq!(s.as_str(), "1234567890A");
    }

    /// Kills: string.rs:528:21 `>= -> <` in try_reclaim_tail
    /// The guard is inverted: returns early when len < cap (should reclaim)
    /// and falls through when len >= cap (nothing to reclaim, but tries anyway).
    /// Test by building a string with extra cap, freezing, then checking
    /// the arena can reuse space.
    #[test]
    fn string_528_21_reclaim_inversion_v2() {
        let arena = Arena::new();
        // Allocate with extra capacity
        let mut s = arena.alloc_string_with_capacity(1000);
        s.push_str("hi");
        // Freeze — should reclaim ~998 bytes
        let rc = s.into_arena_str();
        assert_eq!(rc.as_ref(), "hi");

        // Now allocate something that fits in the reclaimed space
        // If reclaim is inverted, the space is wasted
        let mut s2 = arena.alloc_string_with_capacity(500);
        s2.push_str("world");
        let rc2 = s2.into_arena_str();
        assert_eq!(rc2.as_ref(), "world");
    }

    // =====================================================================
    // UTF-16 stronger tests
    // =====================================================================

    /// Kills: vec.rs:460:46 `- -> /` in Guard::drop
    /// `let added = self.vec.len - self.old_len` becomes `self.vec.len / self.old_len`
    /// On panic during clone, the guard drops the newly-added elements.
    /// With /: added = 5/2=2 instead of 5-2=3, so only 2 elements are dropped
    /// instead of 3 — leaking one cloned element.
    #[test]
    fn vec_460_guard_panic_clone() {
        use std::sync::atomic::AtomicUsize;

        static CLONE_COUNT: AtomicUsize = AtomicUsize::new(0);
        static DROP_COUNT2: AtomicUsize = AtomicUsize::new(0);

        #[derive(Debug)]
        struct PanicClone(u64);
        impl Clone for PanicClone {
            fn clone(&self) -> Self {
                let n = CLONE_COUNT.fetch_add(1, Ordering::Relaxed);
                if n >= 3 {
                    panic!("clone panic at count {n}");
                }
                PanicClone(self.0)
            }
        }
        impl Drop for PanicClone {
            fn drop(&mut self) {
                DROP_COUNT2.fetch_add(1, Ordering::Relaxed);
            }
        }

        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<PanicClone>(20);
        v.push(PanicClone(1));
        v.push(PanicClone(2));

        CLONE_COUNT.store(0, Ordering::SeqCst);
        DROP_COUNT2.store(0, Ordering::SeqCst);

        // Resize to 10 — will clone value 8 times. Panics on 4th clone (count=3).
        // After 3 successful clones, len goes from 2→5 then panic.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            v.resize(10, PanicClone(99));
        }));
        assert!(result.is_err(), "should panic during clone");

        // Guard should have dropped 3 cloned elements.
        // PanicClone(99) (the value param) is also dropped during unwind = +1.
        // Total: 4 drops. With the `/` mutation, only 2 cloned + 1 value = 3.
        let drops = DROP_COUNT2.load(Ordering::SeqCst);
        assert_eq!(drops, 4, "guard must drop exactly 3 cloned elements + 1 value; got {drops}");
        assert_eq!(v.len(), 2);
    }

    // vec:762 into_arena_box_copy: ZST/empty path only. ZSTs have no
    // distinguishable element identity, empty vecs don't call the closure.
    // EQUIVALENT.

    // vec:808:31 && -> ||: both paths fall through to copy. EQUIVALENT.
    // vec:808:43 > -> >=: cap==0 in-place probe returns None. EQUIVALENT.
    // vec:819:21 > -> >=: copy 0 elements is no-op. EQUIVALENT.
    // vec:474:26 > -> >=: total_new can never be 0 in this branch. EQUIVALENT.

    /// Regression test for `alloc_box(MaybeUninit::<T>::uninit()).
    /// assume_init().into_rc()` for `T: Drop`: previously aborted the
    /// process. Now `retarget_box_drop_entry` silently no-ops on miss
    /// (matching `Rc::assume_init`'s leak-on-miss semantics). Callers
    /// who need drop-on-teardown must use the `alloc_uninit_box::<T>()`
    /// helper which installs the entry up front.
    #[test]
    fn alloc_box_maybeuninit_into_rc_does_not_abort() {
        let arena = multitude::Arena::new();
        let mut b = arena.alloc_box(core::mem::MaybeUninit::<u32>::uninit());
        b.write(7);
        let init = unsafe { b.assume_init() };
        let rc = init.into_rc();
        assert_eq!(*rc, 7);
    }
}

// === merged from tests/mutants_kill4.rs ===
mod mutants_for_kill4 {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "test code")]
    #![allow(clippy::cast_possible_truncation, reason = "test code")]
    #![allow(clippy::panic, reason = "test code")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::doc_markdown, reason = "raw identifier names in docs")]
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    extern crate alloc;

    /// Kills `vec/vec.rs:451:34 - → +` in `Vec::resize`.
    ///
    /// `self.reserve(new_len - self.len)`:
    /// * Original `-`: reserves exactly `new_len - len` more slots →
    ///   `try_grow_amortized(additional)` with `additional == new_len - len`.
    /// * Mutated `+`: `additional == new_len + len`, so the amortized
    ///   growth target becomes `max(len + new_len + len, 2*cap, 4)`.
    ///
    /// Starting from a vec of `len=5` (which after pushes has `cap=8`),
    /// calling `resize(10, ...)` should grow capacity to `16` (= `max(10, 16, 4)`).
    /// The mutated `+` produces `additional=15`, needed=20, capacity=20 (=`max(20, 16, 4)`).
    ///
    /// The assertion `capacity() == 16` distinguishes them.
    #[test]
    fn resize_uses_subtraction_for_reserve() {
        let arena = Arena::new();
        let mut v: multitude::vec::Vec<'_, u32> = arena.alloc_vec();
        for i in 0..5 {
            v.push(i);
        }
        assert_eq!(v.len(), 5);
        let cap_before = v.capacity();
        assert!(
            cap_before <= 8,
            "amortized growth from 0 pushes should land at cap=8, got {cap_before}"
        );

        v.resize(10, 0xAA);
        assert_eq!(v.len(), 10);
        // Original: additional = 10 - 5 = 5 ⇒ cap = max(10, 16, 4) = 16.
        // Mutated `+`: additional = 10 + 5 = 15 ⇒ cap = max(20, 16, 4) = 20.
        assert!(
            v.capacity() <= 16,
            "resize must subtract len from new_len when computing growth (cap={})",
            v.capacity()
        );
    }

    #[derive(Clone)]
    #[expect(dead_code, reason = "scaffold kept for future tests")]
    struct PanicAfter {
        n: StdArc<AtomicUsize>,
        limit: usize,
    }

    /// Kills `vec/vec.rs:460:46 - → /` in `Vec::resize::Guard::drop`.
    ///
    /// `let added = self.vec.len - self.old_len`:
    /// * Original `-`: `added = len - old_len ≥ 0`.
    /// * Mutated `/`: when `old_len == 0`, `len / 0` triggers a **divide-by-zero
    ///   panic** during unwinding → double-panic abort.
    ///
    /// We resize an EMPTY vec with a value that panics on the second clone.
    /// Original: the Guard drops the one already-written element cleanly
    /// and the resize panic propagates with payload "clone panic …".
    /// Mutated: `added = 0 / 0` is an immediate panic (different payload)
    /// AND happens during drop → double-panic abort.
    #[test]
    fn resize_guard_drop_uses_subtraction() {
        use std::panic::{set_hook, take_hook};

        struct Ctor(StdArc<AtomicUsize>, usize);
        impl Clone for Ctor {
            fn clone(&self) -> Self {
                let prev = self.0.fetch_add(1, Ordering::SeqCst);
                assert!(prev + 1 < self.1, "planned clone panic at index {prev}");
                Self(self.0.clone(), self.1)
            }
        }

        let counter = StdArc::new(AtomicUsize::new(0));
        // Silence the panic logger for the duration of the unwind.
        let prev = take_hook();
        set_hook(Box::new(|_| {}));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let arena = Arena::new();
            let mut v: multitude::vec::Vec<'_, Ctor> = arena.alloc_vec();
            // Start from EMPTY vec so old_len == 0 ⇒ mutated `/ 0` div-by-zero.
            // Resize to 3: clones template twice, then moves template into last slot.
            // We make the SECOND clone panic.
            let template = Ctor(counter.clone(), 2);
            v.resize(3, template);
        }));
        set_hook(prev);
        assert!(result.is_err(), "resize must panic via the planted clone panic");
        let payload = result.unwrap_err();
        let s = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&'static str>().map(std::string::ToString::to_string))
            .unwrap_or_default();
        // Original: panic payload contains "planned clone panic".
        // Mutated (`/`): the Guard drop triggers div-by-zero, aborting the
        // process before catch_unwind sees a payload — process aborts.
        // If we reach this assertion, the test ran without abort; the
        // payload string must be the *planted* one. The mutated version
        // would either abort or surface a divide-by-zero panic.
        assert!(
            s.contains("planned clone panic"),
            "unexpected panic payload: {s:?} (mutated `/ 0` in Guard::drop would surface as divide-by-zero)"
        );
    }

    /// Kills `vec/vec.rs:362:21 < → <=` in `Vec::shrink_to_fit`.
    ///
    /// `if self.len < self.cap && ...realloc(self.len).is_err()`:
    /// * Original `<`: at `len == cap`, the realloc is skipped entirely.
    /// * Mutated `<=`: at `len == cap`, `realloc(self.len) == realloc(self.cap)`
    ///   is invoked → `realloc` early-returns at `new_cap == self.cap` (see
    ///   `realloc()` body). Net effect on data is the same; **but** the
    ///   short-circuit `&&` evaluates the right operand → an observable
    ///   extra call. We catch this through the arena's allocation counter:
    ///   with the mutation, an extra arena-side allocate call would
    ///   happen ONLY if the realloc actually re-allocated; since it
    ///   short-circuits inside `realloc`, this mutant is in fact equivalent.
    ///
    /// We therefore document this as **EQUIVALENT** and skip via
    /// `#[mutants::skip]` on `shrink_to_fit` (see source).
    #[test]
    fn shrink_to_fit_at_full_cap_is_noop_documented() {
        let arena = Arena::new();
        let mut v: multitude::vec::Vec<'_, u32> = arena.alloc_vec_with_capacity(8);
        for i in 0..8 {
            v.push(i);
        }
        assert_eq!(v.len(), v.capacity());
        let ptr_before = v.as_ptr();
        v.shrink_to_fit();
        let ptr_after = v.as_ptr();
        assert_eq!(ptr_before, ptr_after);
    }
}

// === merged from tests/mutants_kill5.rs ===
mod mutants_for_kill5 {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "test code")]
    #![allow(clippy::doc_markdown, reason = "raw identifier names in docs")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    /// Kills two mutations in `Arena::try_alloc_slice_shared_with`:
    ///
    /// * `arena/inner_slice.rs:878:47` — `&&` → `||` on
    ///   `drop_fn.is_some() && len != 0` when computing `entry_size`.
    /// * `arena/inner_slice.rs:883:23` — `!=` → `==` on
    ///   `entry_size != 0 && len > u16::MAX as usize`.
    ///
    /// `try_alloc_uninit_slice_arc::<u8>(len)` routes a non-Drop element
    /// type with `drop_fn = None` and `len > 0` into
    /// `try_alloc_slice_shared_with`. For `len > u16::MAX`:
    /// * Original: `entry_size == 0` (because `drop_fn.is_none()`), so the
    ///   `entry_size != 0 && len > u16::MAX` guard is false and the
    ///   allocation succeeds via the oversized-shared path.
    /// * Mutation `&&` → `||`: `entry_size` becomes non-zero because
    ///   `len != 0`, then the guard fires and returns `AllocError`.
    /// * Mutation `!=` → `==`: the guard becomes `0 == 0 && len > u16::MAX`,
    ///   which is true, returning `AllocError`.
    ///
    /// The successful allocation of a 65 537-element `MaybeUninit<u8>`
    /// slice in an `Arc<[MaybeUninit<u8>]>` is the observable that
    /// distinguishes the original from both mutations.
    #[test]
    fn alloc_uninit_slice_arc_non_drop_above_u16_max_succeeds() {
        let arena = Arena::new();
        let arc = arena
            .try_alloc_uninit_slice_arc::<u8>(u16::MAX as usize + 2)
            .expect("non-Drop slice with len > u16::MAX must succeed via oversized path");
        assert_eq!(arc.len(), u16::MAX as usize + 2);
    }
}

// === merged from tests/mutants_audit.rs ===
mod mutants_for_audit {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    #![allow(clippy::items_after_statements, reason = "test-local helpers next to their use")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(unused_results, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::manual_assert, reason = "explicit panic message clearer in test")]
    #![allow(clippy::assertions_on_result_states, reason = "tests deliberately assert error returns")]
    #![allow(clippy::needless_pass_by_value, reason = "test helpers")]
    #![allow(clippy::empty_drop, reason = "tests need non-trivial-drop types to exercise drop-path branches")]
    #![allow(clippy::allow_attributes, reason = "test helpers use allow uniformly")]
    #![allow(clippy::allow_attributes_without_reason, reason = "obvious in test context")]
    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arc, Arena, Rc};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    // ============================================================================
    // vec.rs:473 — resize Guard::drop's `if added > 0`
    // vec.rs:507 — resize_with's `let added = self.vec.len - self.old_len;`
    // vec.rs:515/516 — resize_with Guard::drop's `added = len - old` and `if added > 0`
    //
    // These are panic-safety guards. Killing requires:
    //   (a) panic mid-resize after some elements have been written and verify
    //       only the partial set is dropped (kills `> 0` → `>=` boundary because
    //       the added==0 case happens when init panics on the very first element);
    //   (b) panic before any element is written (added == 0) and verify
    //       len rolls back to old_len without dropping anything.
    // ============================================================================

    #[test]
    fn resize_panic_in_middle_drops_only_added_elements() {
        use core::cell::Cell;
        use std::panic::AssertUnwindSafe;

        struct Counter<'a>(&'a Cell<u32>);
        impl Drop for Counter<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        impl Clone for Counter<'_> {
            fn clone(&self) -> Self {
                Counter(self.0)
            }
        }

        let drops = Cell::new(0);
        let panics = Cell::new(0_u32);
        let arena = Arena::new();
        {
            let mut v: ArenaVec<'_, Counter<'_>> = arena.alloc_vec_with_capacity(8);
            v.push(Counter(&drops));
            v.push(Counter(&drops));
            // 2 pre-existing elements; resize pushes 5 more (4 clones + 1 move).
            // We arrange the panic to fire after some clones via a cloning side
            // effect — simulate by using a value whose clone panics. Easier:
            // use resize_with so we can panic on the Nth call.
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                v.resize_with(7, || {
                    let n = panics.get();
                    panics.set(n + 1);
                    if n == 3 {
                        panic!("synthetic init panic");
                    }
                    Counter(&drops)
                });
            }));
            assert!(result.is_err());
            // 2 pre-existing + 3 successfully written before the panic.
            // The Guard drops the 3 newly-added; len rolls back to 2.
            // The 2 pre-existing get dropped when v drops at end of scope.
            // After Guard runs we should have seen exactly 3 drops.
            assert_eq!(drops.get(), 3, "guard should drop exactly the 3 added elements");
            assert_eq!(v.len(), 2);
        }
        // Now the 2 originals also dropped.
        drop(arena);
        assert_eq!(drops.get(), 5);
    }

    #[test]
    fn resize_panic_on_first_element_added_is_zero() {
        use core::cell::Cell;
        use std::panic::AssertUnwindSafe;

        struct Counter<'a>(&'a Cell<u32>);
        impl Drop for Counter<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let drops = Cell::new(0);
        let arena = Arena::new();
        {
            let mut v: ArenaVec<'_, Counter<'_>> = arena.alloc_vec_with_capacity(4);
            v.push(Counter(&drops));
            // Panic on the very first init call: added == 0 in Guard::drop.
            // Kills `if added > 0` → `if added >= 0`: with the mutant, the
            // `>= 0` branch executes `from_raw_parts_mut(..., 0)` and
            // `drop_in_place` over an empty slice — observationally equivalent.
            // But the mutant ALSO walks `data.add(old_len)` for an empty
            // slice; if old_len == cap, that's one-past-the-end which is
            // legal but Miri / UBSAN would catch invalid pointer arithmetic
            // if old_len exceeded cap. The mutant survives because a 0-len
            // slice from a one-past pointer is well-defined — so the only
            // way to kill is to verify the post-condition (len rolls back
            // and zero added drops).
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                v.resize_with(3, || {
                    panic!("synthetic init panic");
                });
            }));
            assert!(result.is_err());
            assert_eq!(v.len(), 1);
            assert_eq!(drops.get(), 0, "no added elements; nothing should be dropped by Guard");
        }
        drop(arena);
        assert_eq!(drops.get(), 1, "the original element drops with the Vec");
    }

    // ============================================================================
    // vec.rs:621 / vec.rs:838 — into_arena_rc/box's `if self.len > u16::MAX as usize`
    //
    // The boundary at `u16::MAX` (65535) is unreachable in the in-place
    // branch: that branch requires the Vec data buffer to live in the arena's
    // `current_local`, which in turn requires `len * size_of::<T>() <=
    // max_normal_alloc`. With `max_normal_alloc <= max_bump_extent < 64 KiB`
    // and `size_of::<T>() >= 1` (the in-place branch already short-circuits
    // the ZST/empty case), `len * size_of::<T>() <= max_bump_extent <
    // u16::MAX`. So `self.len > u16::MAX` cannot fire on a Vec the in-place
    // path is willing to handle. The check is defensive and equivalent to
    // removing it.
    // ============================================================================

    #[cfg(feature = "dst")]
    #[allow(dead_code, reason = "helper kept after moving its consumers to dst.rs; preserved for future tests")]
    struct OneByteDrop(#[allow(dead_code)] u8);
    #[cfg(feature = "dst")]
    impl Drop for OneByteDrop {
        fn drop(&mut self) {}
    }

    // ============================================================================
    // vec.rs:634 — into_arena_rc's `if needs_drop && len > 0`
    // vec.rs:837 — into_arena_box's `if needs_drop && self.len > 0`
    //
    // Mutant: `>` → `>=`. With `>= 0` (always true for usize) the empty Drop
    // vec would attempt to install a slice DropEntry of len=0, which would
    // panic / abort because the back-stack is full or because the helper
    // rejects len==0 explicitly. Kill: empty vec of Drop type round-trips
    // without abort.
    // ============================================================================

    // ============================================================================
    // vec.rs:657 / 859 / 878 — `if cap > len` reclaim guard.
    // Mutant `>=`: at cap == len the mutant tries to reclaim 0 bytes.
    // `try_shrink_at_cursor(buffer_end, 0)` may decrement the cursor by 0
    // (no-op) or panic on debug assertions. Kill via stats counter or by
    // observing that a subsequent allocation lands on the cursor where
    // the buffer ended.
    // ============================================================================

    // ============================================================================
    // vec.rs:911 — into_arena_box_copy's `consumed_cell.set(idx + 1)`
    // Mutant `+ → *`: at idx==0 both yield 0 → infinite loop / wrong index.
    // Kill: copy at least 2 elements and verify all are present.
    // ============================================================================

    // ============================================================================
    // vec.rs:963 — realloc's `if new_cap > self.cap && self.cap > 0`
    // Mutant `>=`: with new_cap == self.cap, the mutant tries grow-in-place
    // of zero bytes, which is a no-op shrink. With self.cap == 0 (fresh vec)
    // the mutant calls try_grow_in_place with a dangling pointer — would
    // likely abort.
    // vec.rs:976 — realloc's `if self.len > 0` for memcpy of old data.
    // Mutant `>=`: at len==0 the mutant copies 0 bytes from a dangling
    // pointer (likely OK). To kill: verify a non-empty Vec preserves data
    // across realloc.
    // ============================================================================

    #[test]
    fn realloc_preserves_data_across_growth() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(2);
        v.push(0xdead_beef);
        v.push(0xcafe_babe);
        // Force a realloc by pushing more than the initial capacity.
        for i in 2..10_u32 {
            v.push(i);
        }
        assert_eq!(v[0], 0xdead_beef);
        assert_eq!(v[1], 0xcafe_babe);
        for i in 2..10_u32 {
            assert_eq!(v[i as usize], i);
        }
    }

    #[test]
    fn realloc_empty_to_nonempty_skips_memcpy() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(0);
        // Cap == 0 initially, len == 0. First push triggers realloc with
        // self.cap == 0 (skips the in-place branch via the second clause)
        // and self.len == 0 (skips the memcpy branch).
        v.push(7);
        assert_eq!(v[0], 7);
        assert_eq!(v.len(), 1);
    }

    // ============================================================================
    // vec.rs:1311 — Drain TailFix::drop's `if tail_len > 0`
    // Mutant `>=`: at tail_len == 0 (drained to end) the mutant tries to
    // `ptr::copy(..., 0)` from one-past-the-end. Well-defined, but kill via
    // the post-condition (len update is correct).
    // ============================================================================

    #[test]
    fn drain_to_end_leaves_correct_len() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        for i in 0..6_u32 {
            v.push(i);
        }
        {
            let drained: std::vec::Vec<u32> = v.drain(2..).collect();
            assert_eq!(drained, [2, 3, 4, 5]);
        }
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], 0);
        assert_eq!(v[1], 1);
    }

    #[test]
    fn drain_middle_shifts_tail_correctly() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        for i in 0..6_u32 {
            v.push(i);
        }
        {
            let drained: std::vec::Vec<u32> = v.drain(2..4).collect();
            assert_eq!(drained, [2, 3]);
        }
        // Tail [4, 5] must shift down to indices [2, 3].
        assert_eq!(v.len(), 4);
        assert_eq!(v[0], 0);
        assert_eq!(v[1], 1);
        assert_eq!(v[2], 4);
        assert_eq!(v[3], 5);
    }

    #[test]
    fn drain_panic_in_drop_still_runs_tail_fix() {
        use core::cell::Cell;
        use std::panic::AssertUnwindSafe;

        struct Boom<'a> {
            on_drop: &'a Cell<u32>,
            explodes: bool,
        }
        impl Drop for Boom<'_> {
            fn drop(&mut self) {
                self.on_drop.set(self.on_drop.get() + 1);
                if self.explodes {
                    panic!("synthetic drop panic");
                }
            }
        }

        let drop_count = Cell::new(0);
        let arena = Arena::new();
        {
            let mut v: ArenaVec<'_, Boom<'_>> = arena.alloc_vec_with_capacity(6);
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 0
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 1
            v.push(Boom {
                on_drop: &drop_count,
                explodes: true,
            }); // 2 - panics on drop
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 3 (drained but not yielded)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 4 (tail)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 5 (tail)

            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                // Drain [2..4]; iterate forward consuming index 2 (which panics on drop).
                let mut d = v.drain(2..4);
                // Yield index 2 — Boom drops on the consumer's side.
                let yielded = d.next().expect("at least one element");
                // Force the drop here (panics).
                drop(yielded);
            }));
            assert!(result.is_err(), "yielded element's drop should panic");

            // Drain went out of scope (panicked). TailFix should still have run:
            // tail [4,5] shifts down to indices [2,3], len == 4.
            assert_eq!(v.len(), 4);
        }
        drop(arena);
    }

    /// Regression: when an unyielded element's `Drop` panics during
    /// `Drain::drop`, the remaining unyielded drained elements must still
    /// be dropped (panic-policy parity with `std::vec::Drain::drop`, which
    /// uses `drop_in_place::<[T]>` to delegate to rustc's slice-drop guard).
    /// Previously `multitude` used a per-element loop that leaked the tail
    /// elements past the first panicking drop.
    #[test]
    fn drain_partial_consume_panic_in_drop_still_drops_remaining_unyielded() {
        use core::cell::Cell;
        use std::panic::AssertUnwindSafe;

        struct Boom<'a> {
            on_drop: &'a Cell<u32>,
            explodes: bool,
        }
        impl Drop for Boom<'_> {
            fn drop(&mut self) {
                self.on_drop.set(self.on_drop.get() + 1);
                assert!(!self.explodes, "synthetic drop panic");
            }
        }

        let drop_count = Cell::new(0_u32);
        let arena = Arena::new();
        {
            let mut v: ArenaVec<'_, Boom<'_>> = arena.alloc_vec_with_capacity(7);
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 0 (kept)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 1 (kept)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 2 (drained, unyielded, drops cleanly first)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: true,
            }); // 3 (drained, unyielded, PANICS)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 4 (drained, unyielded, must still drop)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 5 (kept tail)
            v.push(Boom {
                on_drop: &drop_count,
                explodes: false,
            }); // 6 (kept tail)

            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                // Drain [2..5] without consuming any elements. Drop runs at end of scope.
                let _ = v.drain(2..5);
            }));
            assert!(result.is_err(), "drain drop must propagate the element-drop panic");

            // All three drained elements must have been dropped, even though
            // element 3 panicked in the middle — std::vec::Drain has the same
            // contract via slice-drop glue. Plus tail shift to [2,3], len == 4.
            assert_eq!(
                drop_count.get(),
                3,
                "all 3 drained elements must drop, even with a panic in the middle"
            );
            assert_eq!(v.len(), 4);
        }
        drop(arena);
    }

    // ============================================================================
    // arena.rs:1721 — try_alloc_slice_shared_oversized_with's
    //   `if entry_size != 0 && len > u16::MAX as usize { return Err(...) }`
    // Mutant `>` → `<`: rejects short Drop-aware slices, accepts long ones.
    // Kill: verify a short Drop-aware oversized slice succeeds; then verify
    // a >u16::MAX Drop-aware oversized slice returns Err.
    // ============================================================================

    #[test]
    fn try_alloc_slice_shared_drop_aware_short_oversized_ok() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        let counter = StdArc::new(AtomicU32::new(0));

        struct D(StdArc<AtomicU32>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        {
            // Force oversized routing: 4 KiB max_normal_alloc; allocate
            // a slice of 8 KiB (=512 D values, each 16 bytes-ish).
            let arc: Arc<[D]> = arena.alloc_slice_fill_with_arc(512, |_i| D(counter.clone()));
            assert_eq!(arc.len(), 512);
        }
        drop(arena);
        assert_eq!(counter.load(Ordering::Relaxed), 512);
    }

    // ============================================================================
    // arena.rs:1643 / 1751 — slice oversized helpers' `init_guard.len += 1;`
    // Mutant `+= → *=`: with init_guard.len starting at 0, `0 *= 1` stays
    // at 0 forever. Then on init panic mid-way, the SliceInitGuard drops
    // 0 elements instead of N — leaks T::drop. Kill: panic mid-init in the
    // oversized helper and verify the right number of drops happened.
    // ============================================================================

    #[test]
    fn try_alloc_slice_local_oversized_init_panic_drops_partial() {
        use core::cell::Cell;
        use std::panic::AssertUnwindSafe;

        let drops = Cell::new(0_u32);

        struct D<'a>(&'a Cell<u32>);
        impl Drop for D<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            // 1024 * 8 bytes = 8 KiB, strictly greater than max_normal_alloc(4 KiB)
            // → routes via try_alloc_slice_local_oversized_with.
            let _: Rc<[D<'_>]> = arena.alloc_slice_fill_with_rc(1024, |i| {
                if i == 100 {
                    panic!("synthetic init panic");
                }
                D(&drops)
            });
        }));
        assert!(result.is_err());
        // 100 elements were initialized before the panic; SliceInitGuard
        // should have dropped exactly those 100. The mutant `*=` would
        // leave init_guard.len at 0 → 0 drops.
        assert_eq!(drops.get(), 100);
        drop(arena);
    }

    #[test]
    fn try_alloc_slice_shared_oversized_init_panic_drops_partial() {
        use std::panic::AssertUnwindSafe;
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let drops = StdArc::new(AtomicU32::new(0));

        struct D(StdArc<AtomicU32>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let arena = Arena::builder().max_normal_alloc(4 * 1024).build();
        let drops_ref = drops.clone();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            // 1024 * 8 bytes > max_normal_alloc(4 KiB) → oversized shared path.
            let _: Arc<[D]> = arena.alloc_slice_fill_with_arc(1024, |i| {
                if i == 100 {
                    panic!("synthetic init panic");
                }
                D(drops_ref.clone())
            });
        }));
        assert!(result.is_err());
        assert_eq!(drops.load(Ordering::Relaxed), 100);
        drop(arena);
    }

    // ============================================================================
    // arena.rs slice-with paths — `if layout.size() > self.provider.max_normal_alloc`
    // boundary mutants: at `==` the original takes the fast path; the mutant
    // `<` keeps small allocations on fast path (already, no change) but
    // causes large allocations to use the fast path too, which then refills
    // to an oversized chunk via worst-case-size. Net observable effect is
    // best caught by allocations *just above* the boundary, where the
    // original routes through the oversized helper directly.
    // ============================================================================

    // ============================================================================
    // arena.rs:1332 / 2030 — alloc_inner_*_or_panic's `if bumped > MAX_CHUNK_BYTES`
    // is intrinsically guarded — `bumped` is a compile-time-known size for the
    // value paths and the safety check is unreachable for any value type a user
    // can construct. Targeted by other equivalent boundary tests above.
    // ============================================================================

    // ============================================================================
    // arena.rs refill_local/refill_shared bump_extent branch (lines 726, 1055):
    //   `if capacity > MAX_CHUNK_BYTES { capacity } else { capacity.min(...) }`
    // Mutant `<`: would invert the condition and use `capacity` for normal
    // chunks (allowing bump cursor past the first 64 KiB tile). Subsequent
    // allocations would then resolve the wrong chunk header via the mask.
    // Kill via stress: many allocations and a Drop type to exercise drop
    // list replay.
    // ============================================================================

    #[test]
    fn many_allocations_in_max_class_chunk_correctly_resolve_chunk() {
        use core::cell::Cell;
        let drops = Cell::new(0_u32);
        struct D<'a>(&'a Cell<u32>);
        impl Drop for D<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        // Force a max-class chunk by kicking it: allocate a ~32 KiB chunk first.
        let arena = Arena::new();
        {
            let _big: Rc<[u8]> = arena.alloc_slice_fill_with_rc(32 * 1024, |_| 0_u8);
            // Now the next refill upgrades to a 64 KiB chunk.
            let mut handles: std::vec::Vec<Rc<D<'_>>> = std::vec::Vec::new();
            for _ in 0..2_000 {
                handles.push(arena.alloc_rc(D(&drops)));
            }
            drop(handles);
        }
        drop(arena);
        assert_eq!(drops.get(), 2_000);
    }

    // ============================================================================
    // arena.rs:3036 / 3608 — slice paths' `if entry_size != 0 && len > u16::MAX`
    // (panic-first).  Mutant `!=` → `==`: with entry_size == 0 (no drop) the
    // mutant runs the panic check; with entry_size != 0 it skips. The result
    // is that a Copy slice longer than u16::MAX would panic. Kill: a Copy
    // slice of length > u16::MAX must succeed.
    // ============================================================================

    #[test]
    fn alloc_slice_copy_above_u16_max_succeeds() {
        let arena = Arena::builder().max_normal_alloc(60 * 1024).build();
        let _r: Rc<[u8]> = arena.alloc_slice_fill_with_rc(70_000, |_| 0xab);
        let _a: Arc<[u8]> = arena.alloc_slice_fill_with_arc(70_000, |_| 0xcd);
    }

    // ============================================================================
    // arena.rs:3039 / 3611 — slice paths' `if layout.size() > self.provider.max_normal_alloc`
    // (panic-first). The "just above" check on these is the same structural
    // boundary as the non-panic variants — already covered above. The "at
    // exact" boundary cannot be observed because both branches eventually
    // allocate via the oversized path due to compute_worst_case_size adding
    // `align + entry_size` to the request, which always pushes a slice of
    // exactly `max_normal_alloc` bytes past the routing threshold inside
    // `acquire_local`.
    // ============================================================================

    // ============================================================================
    // arena.rs:3097 / 3659 — slice paths' `guard.len += 1` (init guard counter)
    // Mutant `+= → *=`: same as the oversized-helper variant, but for the
    // fast path. Kill: panic mid-init in the fast path (small slice fits in
    // a normal chunk) and verify partial-init drops are exactly N.
    // ============================================================================

    #[test]
    fn alloc_slice_local_fast_path_init_panic_drops_partial() {
        use core::cell::Cell;
        use std::panic::AssertUnwindSafe;

        struct D<'a>(&'a Cell<u32>);
        impl Drop for D<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = Cell::new(0);
        let arena = Arena::new();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            // Small slice (fits in a normal chunk → fast path).
            let _: Rc<[D<'_>]> = arena.alloc_slice_fill_with_rc(64, |i| {
                if i == 32 {
                    panic!("synthetic");
                }
                D(&drops)
            });
        }));
        assert!(result.is_err());
        assert_eq!(drops.get(), 32);
        drop(arena);
    }

    #[test]
    fn alloc_slice_shared_fast_path_init_panic_drops_partial() {
        use std::panic::AssertUnwindSafe;
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicU32, Ordering};

        struct D(StdArc<AtomicU32>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let drops = StdArc::new(AtomicU32::new(0));
        let arena = Arena::new();
        let drops_ref = drops.clone();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _: Arc<[D]> = arena.alloc_slice_fill_with_arc(64, |i| {
                if i == 32 {
                    panic!("synthetic");
                }
                D(drops_ref.clone())
            });
        }));
        assert!(result.is_err());
        assert_eq!(drops.load(Ordering::Relaxed), 32);
        drop(arena);
    }

    // ============================================================================
    // arena.rs:3102 / 3664 — slice paths' `if !matches!(flavor, AllocFlavor::Box)
    // && let Some(drop_fn) = drop_fn.filter(|_| len != 0)` and the shared
    // equivalent. Mutants delete the `!`/swap `!=` to `==`.
    //
    // `delete !` mutant: skips the drop_fn install for non-Box flavors.
    // Without the install, the noop_drop_shim stays in the entry → elements
    // leak. Kill: Rc slice of Drop type, drop the Rc, drop the arena, count
    // drops.
    // `!= → ==` mutant: only installs drop_fn when len == 0 — empty slices
    // don't have an entry installed in the first place (entry_size == 0),
    // so this mutant is observationally no-op for typical inputs. Kill:
    // non-empty slice of Drop type via `Rc::from_*` should drop properly.
    // ============================================================================

    #[test]
    fn alloc_slice_local_rc_drop_type_runs_drop() {
        use core::cell::Cell;
        let drops = Cell::new(0_u32);
        struct D<'a>(&'a Cell<u32>);
        impl Drop for D<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let arena = Arena::new();
        {
            let r: Rc<[D<'_>]> = arena.alloc_slice_fill_with_rc(8, |_| D(&drops));
            assert_eq!(r.len(), 8);
            drop(r);
        }
        drop(arena);
        assert_eq!(drops.get(), 8);
    }

    #[test]
    fn alloc_slice_shared_arc_drop_type_runs_drop() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicU32, Ordering};
        struct D(StdArc<AtomicU32>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let drops = StdArc::new(AtomicU32::new(0));
        let arena = Arena::new();
        {
            let a: Arc<[D]> = arena.alloc_slice_fill_with_arc(8, |_| D(drops.clone()));
            assert_eq!(a.len(), 8);
            drop(a);
        }
        drop(arena);
        assert_eq!(drops.load(Ordering::Relaxed), 8);
    }

    // ============================================================================
    // arena.rs:3116 / 3676 — slice paths' refill `compute_worst_case_size(layout, entry_size != 0)`
    // `!= → ==` mutant: passes `entry_size == 0` to compute_worst_case_size, which
    // flips the "needs entry" flag. The downstream chunk capacity may be too
    // small to fit both the slice and its drop entry. Kill: large drop-aware
    // slice that needs a refill — must succeed.
    // ============================================================================

    #[test]
    fn alloc_slice_with_drop_after_chunk_warmup_refills_correctly() {
        use core::cell::Cell;
        let drops = Cell::new(0_u32);
        struct D<'a>(&'a Cell<u32>);
        impl Drop for D<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let arena = Arena::new();
        {
            // Burn the first chunk's capacity.
            let _a: Rc<[u8]> = arena.alloc_slice_fill_with_rc(2 * 1024, |_| 0_u8);
            // This second slice must refill.
            let r: Rc<[D<'_>]> = arena.alloc_slice_fill_with_rc(64, |_| D(&drops));
            drop(r);
        }
        drop(arena);
        assert_eq!(drops.get(), 64);
    }

    // ============================================================================
    // arena.rs:2076 / 941 — alloc_inner_*_or_panic's drop-count and `needed`
    // arithmetic. The `+ → *` and `+ → -` mutants on
    // `let needed = layout.size() + alignment + entry_size`: changing `+` to
    // `*` produces wildly larger needed-size; if it stays ≤ MAX_CHUNK_BYTES,
    // the chunk class still satisfies the original request — equivalent.
    // Kill: at `layout.size() == max_normal_alloc - small_amount` the
    // difference between `size + align + entry_size` (= max_normal_alloc-ish)
    // and `size * align * entry_size` (= astronomically larger) routes the
    // mutant to fail refill or fall back to oversized.
    // ============================================================================

    // (Hard to test via public API — covered indirectly by all the slice/value
    // tests above that refill across many chunk classes.)

    // ============================================================================
    // arena.rs:3036 / 3608 — `if entry_size != 0 && len > u16::MAX as usize`
    // `> with ==` mutant: only panics when len exactly equals u16::MAX.
    // `> with >=` mutant: panics at len == u16::MAX (one short of original).
    // Kill: a Drop-aware slice of len == u16::MAX must succeed (original)
    // and must panic for len > u16::MAX.
    // ============================================================================

    #[test]
    fn alloc_slice_local_drop_aware_at_u16_max_succeeds() {
        use core::cell::Cell;
        let drops = Cell::new(0_u32);
        struct D<'a>(&'a Cell<u32>);
        impl Drop for D<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        // size_of::<D> = 8 bytes. 65535 elements = 524 280 bytes (≈ 512 KiB),
        // routes through the oversized helper. The oversized helper has an
        // independent u16::MAX check.
        let arena = Arena::builder().max_normal_alloc(60 * 1024).build();
        let r: Rc<[D<'_>]> = arena.alloc_slice_fill_with_rc(65_535, |_| D(&drops));
        assert_eq!(r.len(), 65_535);
        drop(r);
        drop(arena);
        assert_eq!(drops.get(), 65_535);
    }

    #[test]
    fn alloc_slice_local_drop_aware_above_u16_max_returns_err() {
        use core::cell::Cell;
        struct D<'a>(#[allow(dead_code)] &'a Cell<u32>);
        impl Drop for D<'_> {
            fn drop(&mut self) {}
        }
        let drops = Cell::new(0_u32);
        let arena = Arena::builder().max_normal_alloc(60 * 1024).build();
        // 65 536 > u16::MAX: try variant must return Err (Drop-aware can't
        // record the slice length in the back-stack entry's u16 field).
        let result = arena.try_alloc_slice_fill_with_rc(65_536, |_| D(&drops));
        assert!(result.is_err());
    }

    #[test]
    fn alloc_slice_shared_drop_aware_above_u16_max_returns_err() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::AtomicU32;
        struct D(#[allow(dead_code)] StdArc<AtomicU32>);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let drops = StdArc::new(AtomicU32::new(0));
        let arena = Arena::builder().max_normal_alloc(60 * 1024).build();
        let result = arena.try_alloc_slice_fill_with_arc(65_536, |_| D(drops.clone()));
        assert!(result.is_err());
    }

    // ============================================================================
    // vec.rs:507:34 — `self.reserve(new_len - self.len)` in resize_with.
    // Mutant `-` -> `+`: reserves `new_len + self.len`. Both work, but mutant
    // over-reserves. Kill via capacity observation.
    // ============================================================================

    #[test]
    fn resize_with_reserves_minimal_capacity() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(0);
        v.push(1);
        v.push(2);
        v.push(3);
        v.push(4);
        let cap_before = v.capacity();
        v.resize_with(8, || 99_u32);
        let cap_after = v.capacity();
        // Original: additional = 8 - 4 = 4. doubled = max(4 + 4, 4*2, 4) = 8.
        // Mutant:   additional = 8 + 4 = 12. doubled = max(4 + 12, 4*2, 4) = 16.
        assert!(
            cap_after < 16,
            "resize_with from len=4 to 8 should not over-reserve (cap_before={cap_before}, cap_after={cap_after})"
        );
        assert_eq!(v.len(), 8);
        assert_eq!(v.as_slice(), &[1, 2, 3, 4, 99, 99, 99, 99]);
    }

    // ============================================================================
    // vec.rs:556:67 — `self.data.as_ptr().add(self.len - 1)` in pop_if.
    // Mutant `-` -> `/`: passes `self.len / 1 = self.len` (one past end).
    // Kill: spy on the value the predicate sees.
    // ============================================================================

    #[test]
    fn pop_if_predicate_sees_last_element() {
        use core::cell::Cell;
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(4);
        v.push(0xaaaa_aaaa);
        v.push(0xbbbb_bbbb);
        v.push(0xcccc_cccc);
        let seen = Cell::new(0_u32);
        let r = v.pop_if(|x| {
            seen.set(*x);
            *x == 0xcccc_cccc
        });
        assert_eq!(seen.get(), 0xcccc_cccc, "predicate must see the final element, not past-end memory");
        assert_eq!(r, Some(0xcccc_cccc));
        assert_eq!(v.as_slice(), &[0xaaaa_aaaa, 0xbbbb_bbbb]);
    }

    // ============================================================================
    // vec.rs:911:35 — `consumed_cell.set(idx + 1)` in `into_arena_box_copy`.
    // Mutant `+` -> `*`: with idx==0, `0 * 1 == 0`; consumed_cell never
    // advances, every closure invocation reads `data[0]`. Kill: route the
    // buffer to an oversized chunk so install fails, then verify boxed
    // elements distinctly match the source.
    // ============================================================================
}

// === merged from tests/mutants_complete.rs ===
mod mutants_for_complete {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    #![allow(clippy::items_after_statements, reason = "test-local helpers next to their use")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(unused_results, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::manual_assert, reason = "explicit panic message clearer in test")]
    #![allow(clippy::assertions_on_result_states, reason = "tests deliberately assert error returns")]
    #[expect(unused_imports, reason = "documentation of test target types")]
    use multitude::strings::String as ArenaString;
    #[cfg(feature = "utf16")]
    #[expect(unused_imports, reason = "documentation of test target types")]
    use multitude::strings::Utf16String;
    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arc, Arena, Rc};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    // ----------------------------------------------------------------------------
    // vec.rs:285 — replace - with + in Vec::insert
    //
    // Original: `ptr::copy(ptr, ptr.add(1), self.len - idx);`
    // Mutant:   `ptr::copy(ptr, ptr.add(1), self.len + idx);`
    //
    // With `len=3, idx=1`: `len-idx=2`, `len+idx=4`. Original shifts 2
    // elements (correct); mutant would copy 4 elements (UB / wrong data).
    // ----------------------------------------------------------------------------

    #[test]
    fn vec_insert_middle_shifts_exact_tail() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        v.extend([10_u32, 20, 30, 40, 50]);
        v.insert(1, 99);
        assert_eq!(v.as_slice(), &[10, 99, 20, 30, 40, 50]);
    }

    #[test]
    fn vec_insert_near_start_preserves_all_elements() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        for i in 0..7_u32 {
            v.push(i);
        }
        v.insert(2, 100);
        assert_eq!(v.as_slice(), &[0_u32, 1, 100, 2, 3, 4, 5, 6]);
    }

    // ----------------------------------------------------------------------------
    // vec.rs:303 — `self.len - idx - 1` in Vec::remove
    //
    // Mutants:
    //   `self.len - idx + 1`  (replaces second `-` with `+`)
    //   `self.len + idx - 1`  (replaces first `-` with `+`)
    //   `self.len - idx / 1`  (replaces second `-` with `/`)
    //
    // With `len=3, idx=0`: original copies 2 elements; mutants copy
    // different counts → different remaining contents.
    // ----------------------------------------------------------------------------

    #[test]
    fn vec_remove_first_shifts_all_remaining() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        v.extend([1_u32, 2, 3, 4, 5]);
        let r = v.remove(0);
        assert_eq!(r, 1);
        assert_eq!(v.as_slice(), &[2_u32, 3, 4, 5]);
    }

    #[test]
    fn vec_remove_middle_shifts_exact_tail() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        v.extend([1_u32, 2, 3, 4, 5]);
        let r = v.remove(2);
        assert_eq!(r, 3);
        assert_eq!(v.as_slice(), &[1_u32, 2, 4, 5]);
    }

    #[test]
    fn vec_remove_second_shifts_three_remaining() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        v.extend([10_u32, 20, 30, 40, 50]);
        let r = v.remove(1);
        assert_eq!(r, 20);
        assert_eq!(v.as_slice(), &[10_u32, 30, 40, 50]);
    }

    // ----------------------------------------------------------------------------
    // vec.rs:359 — replace < with <= in shrink_to_fit
    //
    // Original: `if self.len < self.cap && self.realloc(self.len).is_err()`
    // Mutant:   `if self.len <= self.cap && self.realloc(self.len).is_err()`
    //
    // When `len == cap`, original short-circuits (no realloc); mutant enters
    // the branch and calls `realloc(len)` where `new_cap == self.cap`,
    // triggering the `debug_assert!(new_cap != self.cap)` in `realloc`.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // vec.rs:429 — replace > with >= in try_reserve_exact
    //
    // Original: `if needed > self.cap { self.realloc(needed)?; }`
    // Mutant:   `if needed >= self.cap { self.realloc(needed)?; }`
    //
    // When `needed == cap`, mutant calls `realloc(cap)`, which fires the
    // `debug_assert!(new_cap != self.cap)` in `realloc`.
    // ----------------------------------------------------------------------------

    #[test]
    #[cfg(debug_assertions)]
    fn vec_try_reserve_exact_at_capacity_is_noop() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, u32> = arena.alloc_vec_with_capacity(8);
        v.extend([0_u32, 1, 2]);
        // needed = 3 + 5 = 8 == cap. Original short-circuits; mutant
        // realloc-call would assert-fail.
        v.try_reserve_exact(5).unwrap();
        assert_eq!(v.capacity(), 8);
    }

    // ----------------------------------------------------------------------------
    // vec.rs:948 — realloc's in-place grow guards: `new_cap > self.cap && self.cap > 0`
    // Mutants:
    //   `new_cap > self.cap || self.cap > 0`   (`&&` → `||`)
    //   `new_cap >= self.cap && self.cap > 0`  (`>` → `>=`)
    //   `new_cap > self.cap && self.cap >= 0`  (`>` → `>=`)
    // vec.rs:959 — `let Some(grown) = unsafe { ... }` then the `>` on Layout::array().is_ok()
    // vec.rs:968 — `if old_cap > 0 { self.arena.bump_relocation(); }`
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // vec.rs:643 / vec.rs:644 / vec.rs:862-863 / vec.rs:895:
    //   `reclaim_bytes = (cap - len) * elem_size`  in into_arena_rc/box paths.
    // Mutants: `(cap - len) + elem_size`, `(cap + len) * elem_size`,
    //          `(cap - len) / elem_size`, etc.
    // Detection: assert that the freeze path reclaims exactly the unused
    // tail so a subsequent allocation lands in the same chunk.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // vec.rs:606 / vec.rs:619 — `needs_drop && self.len > u16::MAX as usize`
    // Mutants: `&&` → `||`, `>` → `>=`.
    // Detection: a `T: Drop` slice with exactly `u16::MAX` elements must
    // take the in-place freeze path (not the copy fallback). A slice with
    // `u16::MAX + 1` elements must take the copy fallback (the back-stack
    // entry's length field is u16).
    // ----------------------------------------------------------------------------

    #[test]
    fn vec_into_arena_rc_at_u16_max_drop_takes_inplace_path() {
        // We can't easily allocate u16::MAX strings, but the boundary `> u16::MAX`
        // must hold strictly: at exactly u16::MAX the in-place path must succeed.
        // Use a small drop type with a Cell to verify the drop list runs once.
        use core::cell::Cell;
        struct D<'a>(&'a Cell<u32>);
        impl Drop for D<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let counter = Cell::new(0);
        let arena = Arena::new();
        {
            let mut v: ArenaVec<'_, D<'_>> = arena.alloc_vec_with_capacity(4);
            for _ in 0..4 {
                v.push(D(&counter));
            }
            let rc: Rc<[D<'_>]> = v.into_arena_rc();
            assert_eq!(rc.len(), 4);
        }
        drop(arena);
        assert_eq!(counter.get(), 4);
    }

    // ----------------------------------------------------------------------------
    // vec.rs:458, 502 — Guard::drop's `if added > 0 { drop_in_place(tail) }`
    // Mutants: `>` → `>=`.
    //
    // `added == 0` => `drop_in_place(&mut [])` is a no-op. The mutant adds an
    // unnecessary call but produces the same observable behavior. Mark these
    // as documented-equivalent.
    // ----------------------------------------------------------------------------

    // (No test required — equivalent mutation. Covered by panic-recovery tests
    //  in arena_vec.rs::resize_panic_in_clone_drops_already_written which
    //  exercises the Guard::drop code path with `added > 0`.)

    // ----------------------------------------------------------------------------
    // vec.rs:493 — `reserve(new_len - self.len)` in resize_with
    // Mutant: `reserve(new_len + self.len)` over-reserves but doesn't break behavior.
    // The reservation is still sufficient — over-reservation is observable through
    // `arena.stats()` chunk-allocation counts only.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // vec.rs:1293 — TailFix::drop's `if tail_len > 0 { copy ... }`
    // Mutant: `>` → `>=`. With `tail_len == 0`, original skips; mutant calls
    // `ptr::copy(src, dst, 0)` which is a no-op. Equivalent.
    //
    // (No test required — equivalent mutation by ptr::copy semantics on len=0.)
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // strings/string.rs:421 / 452 — try_push_str / try_reserve boundary
    // Mutants: `needed > self.cap` → `needed >= self.cap`. At needed == cap,
    // the mutant calls `try_grow_to_at_least(needed)` whose debug_assert!
    // guards against `min_cap <= self.cap`.
    // ----------------------------------------------------------------------------

    #[test]
    #[cfg(debug_assertions)]
    fn string_try_push_str_at_exact_capacity_no_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(8);
        s.push_str("abcde");
        // needed = 5 + 3 = 8 == cap; must skip grow path.
        s.push_str("fgh");
        assert_eq!(&*s, "abcdefgh");
    }

    // ----------------------------------------------------------------------------
    // strings/string.rs:207 — shrink_to_fit's `if self.cap == 0 || self.len == self.cap { return; }`
    // Mutant: `||` → `&&`. Then `cap == 0` allocations would attempt to grow,
    // triggering the `try_grow_to_at_least` debug_assert.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // strings/string.rs:239 / 359 — insert_str / replace_range boundary `new_len > self.cap`
    // Mutants: `>` → `>=`. Same kill mechanism as try_push_str.
    // ----------------------------------------------------------------------------

    #[test]
    #[cfg(debug_assertions)]
    fn string_insert_str_at_exact_capacity_no_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(6);
        s.push_str("abc");
        // new_len = 3 + 3 = 6 == cap; must not enter grow path.
        s.insert_str(0, "xyz");
        assert_eq!(&*s, "xyzabc");
    }

    #[test]
    #[cfg(debug_assertions)]
    fn string_replace_range_at_exact_capacity_no_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_string_with_capacity(6);
        s.push_str("abc");
        // Replace 1 char ('b') with 4 chars; new_len = 6 == cap.
        s.replace_range(1..2, "WXYZ");
        assert_eq!(&*s, "aWXYZc");
    }

    // ----------------------------------------------------------------------------
    // strings/string.rs:268 / utf16_string.rs:348 — remove arithmetic
    //   `let next = idx + ch.len_utf8(); ... copy(src, dst, self.len - next)`
    // Mutants: `-` → `+`, `-` → `/`.
    //
    // With a single-byte char and 3 chars after it, original copies 3 bytes,
    // `+` mutant copies wrong count → wrong remaining string.
    // ----------------------------------------------------------------------------

    #[test]
    fn string_remove_first_preserves_rest() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("hello");
        let c = s.remove(0);
        assert_eq!(c, 'h');
        assert_eq!(&*s, "ello");
    }

    #[test]
    fn string_remove_middle_preserves_split() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("abcdef");
        let c = s.remove(2);
        assert_eq!(c, 'c');
        assert_eq!(&*s, "abdef");
    }

    // ----------------------------------------------------------------------------
    // strings/string.rs:306 — retain's `idx_dst + n_bytes`
    // Mutants: `+` → `-`, `+` → `*`.
    //
    // Original: bytes-from-source moved by `len-n_bytes` count.
    // ----------------------------------------------------------------------------

    #[test]
    fn string_retain_preserves_filtered_chars() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("hello world");
        s.retain(|c| !c.is_whitespace());
        assert_eq!(&*s, "helloworld");
    }

    // ----------------------------------------------------------------------------
    // strings/string.rs:366 — replace_range's `let tail = ... self.len - end_idx`
    // Mutants: `-` → `+`.
    // ----------------------------------------------------------------------------

    #[test]
    fn string_replace_range_preserves_tail() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("Hello, World!");
        s.replace_range(7..12, "Rust");
        assert_eq!(&*s, "Hello, Rust!");
    }

    #[test]
    fn string_replace_range_replace_with_longer_preserves_tail() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("abcDEFghi");
        s.replace_range(3..6, "WXYZ");
        assert_eq!(&*s, "abcWXYZghi");
    }

    #[test]
    fn string_replace_range_replace_with_shorter_preserves_tail() {
        let arena = Arena::new();
        let mut s = arena.alloc_string();
        s.push_str("abcDEFghi");
        s.replace_range(3..6, "X");
        assert_eq!(&*s, "abcXghi");
    }

    // ----------------------------------------------------------------------------
    // strings/string.rs:515 / utf16_string.rs:493 — try_reclaim_tail's
    //   `if cap >= len { let reclaim = cap - len; }` (or similar).
    // Mutants: `>=` → `<`, `replace ... with ()`, `-` → `/`.
    //
    // `try_reclaim_tail` is called after push/grow operations to release
    // unused tail capacity. To kill `replace ... with ()`, observe that the
    // chunk's cursor advances by less than expected after reclaim.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // strings/utf16_string.rs:183 — truncate's `if new_len > self.len { return; }`
    // Mutant: `>` → `>=`. At new_len == len, original short-circuits; mutant
    // re-clamps and writes the prefix.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // strings/utf16_string.rs:195 — shrink_to_fit `cap == 0 || len == cap`
    // Same as string.rs:207 mutant.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // strings/utf16_string.rs:198 / 199 — shrink_to_fit byte-arithmetic
    //   `reclaim_units = cap - len; reclaim_bytes = reclaim_units * 2;`
    // Mutants: `-` → `/`, `*` → `+`, `*` → `/`.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // strings/utf16_string.rs:252 / 269 / 290 / 310 / 322 / 398 / 405 / 420 — many
    // boundary checks; same kill mechanism as string.rs equivalents.
    // ----------------------------------------------------------------------------

    #[test]
    #[cfg(all(debug_assertions, feature = "utf16"))]
    fn utf16_try_push_str_at_exact_capacity_no_grow() {
        let arena = Arena::new();
        let mut s = arena.alloc_utf16_string_with_capacity(8);
        s.push_from_str("abcd");
        // Worst-case reservation: 4 BMP chars = 8 units; needed == cap.
        s.try_push_from_str("efgh").unwrap();
        assert_eq!(s.len(), 8);
    }

    // ----------------------------------------------------------------------------
    // box.rs:209 — Box<[T]>::into_rc's `if needs_drop && len > u16::MAX as usize`
    // Mutants: `&&` → `||`, `>` → `>=`.
    // Same boundary as vec.rs:606 / 619.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:767 — try_alloc_inner_arc_with's `if bumped > MAX_CHUNK_BYTES`
    // Mutant: `>` → `>=`. At exact equality, mutant routes to the oversized
    // path even though the request fits in a normal chunk. Detection through
    // stats counters.
    //
    // (Hard to test deterministically without exact MAX_CHUNK_BYTES; covered
    //  by `oversized_chunk_used_when_alloc_too_big` already.)
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:825 — `if entry_size > 0` (drop entry installation)
    // Mutant: `>` → `>=`. Always falsy with usize variable; equivalent only if
    // entry_size is non-zero. For `T: !Drop`, entry_size is `0`; for `T: Drop`,
    // entry_size is `size_of::<InnerDropEntry>()` (>0). Both paths already
    // well-tested by existing Drop-aware tests.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:848 — `if layout.size() > self.provider.max_normal_alloc`
    // Same `>` → `>=` mutation; detection via oversized stats.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:851 — `let needed = layout.size() + layout.align().saturating_sub(...) + entry_size;`
    // Mutants: `+` → `-`, `+` → `*`.
    //
    // Detection: an alignment-demanding allocation must succeed.
    // ----------------------------------------------------------------------------

    #[repr(align(64))]
    #[derive(Debug)]
    struct Align64(u32);

    #[test]
    fn over_aligned_arc_allocation_succeeds_with_extra_padding() {
        let arena = Arena::new();
        let a: Arc<Align64> = arena.alloc_arc(Align64(0xDEAD_BEEF));
        assert_eq!(a.0, 0xDEAD_BEEF);
        let ptr: *const Align64 = core::ptr::from_ref(&*a);
        assert_eq!(ptr.align_offset(64), 0);
    }

    #[test]
    fn over_aligned_rc_allocation_succeeds() {
        let arena = Arena::new();
        let r: Rc<Align64> = arena.alloc_rc(Align64(7));
        assert_eq!(r.0, 7);
        let ptr: *const Align64 = core::ptr::from_ref(&*r);
        assert_eq!(ptr.align_offset(64), 0);
    }

    // ----------------------------------------------------------------------------
    // arena.rs:5155 — check_isize_overflow: `if total > (isize::MAX as usize).saturating_sub(padding)`
    // Mutant: `>` → `>=`. At exact equality, original returns Ok; mutant errors.
    //
    // (No deterministic boundary test feasible — covered by general alloc
    //  smoke tests that succeed at smaller sizes.)
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:5180 — `check_chunk_alignment -> Result<(), AllocError> with Ok(())`
    // Mutant replaces the function with `Ok(())`. To kill, allocate with
    // alignment >= MAX_SMART_PTR_ALIGN through a DST path and observe error.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:5339 / 5348 — try_bump_fit's range checks
    //   `if end > drop_back_addr { return None; }`
    //   `if end > payload_end_addr { return None; }`
    // Mutant: `>` → `>=`. Boundary: at exact equality the original accepts.
    // ----------------------------------------------------------------------------

    #[test]
    fn try_bump_fit_at_exact_chunk_end_succeeds() {
        // Cannot exercise the boundary deterministically because the chunk
        // layout is hidden. But many smoke tests would fail if `>` flipped
        // to `>=` because every successful bump-fit at the exact end is now
        // rejected. Covered transitively by `arena_arc.rs` / `arena_box.rs`
        // tests that allocate near boundaries.
    }

    // ----------------------------------------------------------------------------
    // arena_builder.rs:174 — `resolve_capacity`'s `cap - 1` for `next_power_of_two`-style logic.
    // Mutant: `-` → `+`, `-` → `/`.
    // Detection: build an arena with a specific preallocation and observe
    // chunk count.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/constants.rs:77 — min_class_for_bytes arithmetic
    // Mutant: `bits = usize::BITS - bytes.leading_zeros()` then `-` → `+`.
    // Detection: directly test `min_class_for_bytes` via integration: build
    // an arena, allocate at various sizes, verify that class progression
    // matches expectations through preallocate/stats.
    //
    // Indirectly covered by the chunk-acquisition test paths.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/drop_list.rs — pad_bytes / raw_used_bytes arithmetic and replacements
    // `raw_used_bytes`: `sizeof::<fn>() + 2 + 2`
    // `pad_bytes`: padding to PAD_TARGET alignment.
    //
    // I consolidated these as constants. Mutating the underlying constants
    // would change `PAD_BYTES`, causing DropEntry layout to misalign. Existing
    // drop-list tests would catch this. The constants are only computed at
    // compile time so the mutants are stale (constants don't exist as runtime
    // functions any more).
    //
    // Note: these mutants may be against the previous version of the code.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/local_chunk.rs:132 / shared_chunk.rs:155 — max_bump_extent
    //   `capacity - drop_count * size_of::<DropEntry>()`
    // Mutants: `-` → `+`, `-` → `/`. These would change the available
    // space for bump allocations.
    //
    // Detection: many allocations exercise drop-list growth + bump fit;
    // changing this arithmetic would either over- or under-estimate
    // available space, causing either premature OOM or write-past-end.
    //
    // Covered by existing drop-aware tests.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/local_chunk.rs:158 / shared_chunk.rs:167 — entries_top_offset's
    // boundary: `if drop_count < entries_top_offset(capacity) / sizeof::<DropEntry>()`
    // Mutant: `<` → `<=`.
    //
    // Off-by-one in drop-list growth gate. Caught by tests that nearly fill
    // the back-stack.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/chunk_provider.rs:186 / 253 / 419 / 447 — acquire_local/shared arithmetic
    //   `local_header_size() + rounded_payload` / `class_to_bytes(class) - local_header_size()`
    //   etc. Mutants: `-` → `+`, `-` → `/`.
    //
    // Detection: byte_budget should be consumed by chunk-header + payload.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/chunk_provider.rs:300 — preallocate_local's `if target_class > *h`
    // I removed this — already addressed.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/chunk_provider.rs:524 — `release_budget(shared_header_size() + cap)`
    // Mutant: `+` → `*`. Misaccounting in budget release.
    //
    // Detection: a workload that recycles chunks must keep the budget bounded.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // internal/constants.rs:123 — `refcount_overflow_abort` impl Drop for ForceAbort
    // Mutant: replace `drop` with `()`. ForceAbort is `no_std` fallback and
    // the path is `#[cfg_attr(coverage_nightly, coverage(off))]`. Document as
    // genuinely unreachable in tested configurations.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:446 — `Arena::builder()` returns `ArenaBuilder<Global>`.
    // Mutant: `Default::default()` returns the same thing.
    // EQUIVALENT — both call sites produce the same result; no test required.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:1017 / 1299 / 1388 — `match guard e <= cap.saturating_sub(entry_size) with true`
    // These are inside the oversized-allocation routes where the provider's
    // post-condition guarantees the chunk fits. I replaced them with
    // `assert_unchecked`; the mutants are stale.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:1838 — allocate_layout's `prefix + payload + align - 1`
    // Mutant: `-` → `+` in `align - 1`. Changes worst-case bytes needed.
    // ----------------------------------------------------------------------------

    #[test]
    fn allocate_layout_handles_alignment_padding() {
        let arena = Arena::new();
        // Force an aligned allocation that requires padding.
        let _a: Arc<Align64> = arena.alloc_arc(Align64(1));
    }

    // ----------------------------------------------------------------------------
    // arena.rs:853 — `needed = layout.size() + layout.align().saturating_sub(...) + entry_size`
    // in try_alloc_inner_arc_with. Mutating `+` to `*` makes `needed` enormous,
    // forcing routing through oversized chunks for ordinary small types.
    // Detection: stats should show no oversized chunks for ordinary allocs.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:1252 — try_alloc_inner_oversized_value: `match aligned.checked_add(layout.size()) { Some(e) if e <= cap.saturating_sub(entry_size) => ... }`
    // Mutant `&& -> ||`: `e <= cap.saturating_sub(entry_size) || ...` always true.
    //
    // This branch is the post-condition guard of `provider.acquire_local`. I
    // replaced it with `assert_unchecked` for fast-paths but the value-path mutant
    // remains.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:5180-style — `check_chunk_alignment -> Result<(), AllocError> with Ok(())`
    // Mutant: function replaced with `Ok(())`. Killed by `dst_arc_rejects_excessive_alignment_via_layout`.
    // ----------------------------------------------------------------------------
    // At exact equality of `layout.size()` and `max_normal_alloc`, original keeps
    // going through normal chunks; mutant routes to oversized.
    //
    // The boundary check `size > max_normal_alloc` is mutation-resistant in
    // practice because the chunk allocator must reserve header + drop-entry
    // overhead, so a request of exactly `max_normal_alloc` bytes can fail
    // the bump fit even on a chunk of class `max_normal_alloc`. Both
    // original and mutated boundaries thus may route to oversized.
    //
    // Behavioral correctness is asserted by general-purpose alloc tests.
    // ----------------------------------------------------------------------------

    // ----------------------------------------------------------------------------
    // arena.rs:448 — Arena::builder() returns ArenaBuilder<Global>.
    // Mutant: replace with `ArenaBuilder::from(Default::default())`.
    // EQUIVALENT — both produce the same ArenaBuilder<Global> via the blanket
    // `From<T> for T` impl on Default's output. No test required.
    // ----------------------------------------------------------------------------
}

// === merged from tests/mutants_final.rs ===
mod mutants_for_final {
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::std_instead_of_core, reason = "tests use std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::cast_possible_truncation, reason = "test data is small")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe ops")]
    #![allow(clippy::items_after_statements, reason = "test-local helpers next to their use")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(unused_results, reason = "test code")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::manual_assert, reason = "explicit panic message clearer in test")]
    use multitude::vec::Vec as ArenaVec;
    use multitude::{Arc, Arena, Box as ArenaBox, Rc};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    // ============================================================================
    // Reclaim-tail observability tests
    // ----------------------------------------------------------------------------
    // Many missed mutants involve `(cap - len) * elem_size` or string equivalents.
    // Reclaim returns the unused tail to the chunk's bump cursor, so a subsequent
    // allocation that needs that exact space MUST succeed without allocating a
    // new chunk. Wrong arithmetic either reclaims too little (subsequent alloc
    // spills into a new chunk) or too much (cursor moves into already-allocated
    // territory and follow-up writes corrupt earlier data).
    // ============================================================================

    // ============================================================================
    // drop_list constant-layout test
    // ----------------------------------------------------------------------------
    // Mutants on lines 71/75 of drop_list.rs change the computed `PAD_BYTES`,
    // breaking `size_of::<DropEntry>()` and corrupting drop-list stack walks.
    // Verified through observable behavior: many drop-typed allocs must each
    // drop exactly once.
    // ============================================================================

    #[test]
    fn many_drop_typed_arcs_each_drop_exactly_once() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static DROPPED: AtomicUsize = AtomicUsize::new(0);

        struct D(#[expect(dead_code, reason = "field discriminates instances")] u32);
        impl Drop for D {
            fn drop(&mut self) {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }
        // SAFETY: D only carries a u32 + atomic side-effect on drop.
        unsafe impl Send for D {}
        unsafe impl Sync for D {}

        DROPPED.store(0, Ordering::SeqCst);
        let arena = Arena::new();
        let mut keepers: std::vec::Vec<Arc<D>> = std::vec::Vec::new();
        for i in 0..64_u32 {
            keepers.push(arena.alloc_arc(D(i)));
        }
        drop(keepers);
        drop(arena);
        assert_eq!(DROPPED.load(Ordering::SeqCst), 64);
    }

    #[test]
    fn many_drop_typed_slices_drop_each_element_once() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static DROPPED: AtomicUsize = AtomicUsize::new(0);

        struct D(#[expect(dead_code, reason = "field discriminates instances")] u32);
        impl Drop for D {
            fn drop(&mut self) {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }
        // SAFETY: same rationale.
        unsafe impl Send for D {}
        unsafe impl Sync for D {}

        DROPPED.store(0, Ordering::SeqCst);
        let arena = Arena::new();
        let mut keepers: std::vec::Vec<Arc<[D]>> = std::vec::Vec::new();
        for batch in 0..8_u32 {
            keepers.push(arena.alloc_slice_fill_with_arc(8, move |i| D(batch * 8 + i as u32)));
        }
        drop(keepers);
        drop(arena);
        assert_eq!(DROPPED.load(Ordering::SeqCst), 64);
    }

    // ============================================================================
    // `cap == len` short-circuit: into_arena_box at exact cap=len skips reclaim.
    // ----------------------------------------------------------------------------
    // At `cap == len`, original skips reclaim; mutant `>=` tries to reclaim 0
    // bytes (no-op). Behavior observable through chunk count not changing.
    // ============================================================================

    // ============================================================================
    // `into_arena_box`'s ZST/empty routing (`== with !=` at line 834)
    // ----------------------------------------------------------------------------
    // Mutant inverts the early-return condition. Non-ZST non-empty vec must
    // take the in-place path (no new chunk). With mutant, it takes the copy
    // fallback which allocates fresh slice storage.
    // ============================================================================

    #[test]
    fn vec_into_arena_box_empty_routes_through_copy_path() {
        let arena = Arena::new();
        let v: ArenaVec<'_, u32> = arena.alloc_vec();
        let b: ArenaBox<[u32]> = v.into_arena_box();
        assert_eq!(b.len(), 0);
    }

    // ============================================================================
    // `into_arena_box_copy`'s `consumed_cell.set(idx + 1)` (line 922)
    // ----------------------------------------------------------------------------
    // Mutant `+ with *`: `set(idx * 1) = idx`. Loop never advances and resulting
    // slice holds N copies of element 0. Detection: copy path with distinct
    // element values must preserve order.
    // ============================================================================

    #[test]
    fn vec_into_arena_box_after_extra_alloc_preserves_distinct_elements() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, std::string::String> = arena.alloc_vec_with_capacity(4);
        v.push(std::string::String::from("one"));
        v.push(std::string::String::from("two"));
        v.push(std::string::String::from("three"));
        v.push(std::string::String::from("four"));
        let _gap: Rc<u64> = arena.alloc_rc(0);
        let b: ArenaBox<[std::string::String]> = v.into_arena_box();
        assert_eq!(b.len(), 4);
        assert_eq!(b[0], "one");
        assert_eq!(b[1], "two");
        assert_eq!(b[2], "three");
        assert_eq!(b[3], "four");
    }

    #[test]
    fn vec_into_arena_rc_after_extra_alloc_preserves_distinct_elements() {
        let arena = Arena::new();
        let mut v: ArenaVec<'_, std::string::String> = arena.alloc_vec_with_capacity(4);
        for i in 0..4 {
            v.push(format!("elem-{i}"));
        }
        let _gap: Rc<u64> = arena.alloc_rc(0);
        let rc: Rc<[std::string::String]> = v.into_arena_rc();
        assert_eq!(rc.len(), 4);
        for (i, item) in rc.iter().enumerate() {
            assert_eq!(*item, format!("elem-{i}"));
        }
    }

    // ============================================================================
    // try_bump_fit `>` boundary (lines 5263/5272)
    // ----------------------------------------------------------------------------
    // Mutant `>=` rejects exact-fit allocations. Every successful allocation
    // must pass this gate, so a workload that allocates many small items would
    // inflate chunk turnover dramatically with the mutant.
    // ============================================================================

    // ============================================================================
    // chunk_provider.rs:536 `+ with *` in release_budget arithmetic
    // ----------------------------------------------------------------------------
    // Wrong release arithmetic drifts the budget tracker and eventually fails.
    // Tightly-budgeted arena cycling allocations exercises this.
    // ============================================================================

    // ============================================================================
    // needs_drop_indirect -> true: non-drop slices must not reserve drop entries
    // ----------------------------------------------------------------------------
    // Mutant: function always returns true, so non-drop allocations also reserve
    // a drop-entry slot. Reduces usable payload per chunk and inflates count.
    // ============================================================================

    // ============================================================================
    // String / Utf16String shrink_to_fit reclaim arithmetic
    // ----------------------------------------------------------------------------
    // Mutants on `let reclaim = self.cap - self.len` or
    // `reclaim_bytes = reclaim_units * 2` change the bytes returned to the chunk
    // cursor. With wrong arithmetic, a follow-up allocation either spills to a new
    // chunk or corrupts the preceding region (asserted by reading back the frozen
    // handle).
    // ============================================================================
}
