// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consolidated bolero property tests (lifecycle + panic safety).
//!
//! The whole file is gated out of Miri because:
//!  * `bolero::check!` corpus replay needs filesystem isolation that
//!    Miri does not provide;
//!  * even the four `#[cfg_attr(miri, ignore)]` tests would otherwise
//!    pull `bolero`, `bolero-engine`, and their generated `TypeGenerator`
//!    code through Miri's MIR translation, which is the dominant cost
//!    when the tests themselves are skipped.
//!
//! The unsafe lifecycle/drop paths exercised here are independently
//! covered under Miri by `arena_arc.rs`, `arena_box.rs`,
//! `arena_string.rs`, `arena_vec.rs`, and `drop_reentrancy.rs`.
#![cfg(not(miri))]

mod common;

// === lifecycle property tests ===
mod bolero_lifecycle {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::std_instead_of_alloc, reason = "test code uses std")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::missing_assert_message, reason = "test assertions are self-explanatory")]
    #![allow(clippy::min_ident_chars, reason = "short names in test loops")]
    #![allow(clippy::single_match_else, reason = "test code")]
    #![allow(clippy::cast_possible_truncation, reason = "test indices are bounded")]
    #![allow(clippy::similar_names, reason = "str_boxes / str_arcs are parallel container names")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::too_many_lines, reason = "fuzz driver dispatches over a wide Op enum")]
    #![allow(
        clippy::large_stack_arrays,
        reason = "oversized payload deliberately stresses the oversized-chunk path"
    )]
    #![allow(clippy::large_enum_variant, reason = "Op variants vary deliberately to stress allocation paths")]
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use bolero::TypeGenerator;
    use multitude::{Arc, Arena, Box as ArenaBox};

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    /// Counter shared between the test driver and every [`Tracker`].
    /// A fresh pair is allocated per iteration so concurrent runs don't
    /// contaminate each other.
    type Counter = StdArc<AtomicUsize>;

    /// Payload that increments `created` on construction and `dropped` on
    /// drop. After a full sequence + arena drop, `created == dropped` must
    /// hold.
    struct Tracker {
        dropped: Counter,
        _payload: u64,
    }

    impl Tracker {
        fn new(created: &Counter, dropped: &Counter, payload: u64) -> Self {
            let _ = created.fetch_add(1, Ordering::Relaxed);
            Self {
                dropped: dropped.clone(),
                _payload: payload,
            }
        }
    }

    impl Drop for Tracker {
        fn drop(&mut self) {
            let _ = self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Zero-sized payload that still records create/drop events. Exercises the
    /// ZST allocation path (no backing bytes, but `DropEntry` still required).
    struct ZstTracker {
        dropped: Counter,
    }

    impl ZstTracker {
        fn new(created: &Counter, dropped: &Counter) -> Self {
            let _ = created.fetch_add(1, Ordering::Relaxed);
            Self { dropped: dropped.clone() }
        }
    }

    impl Drop for ZstTracker {
        fn drop(&mut self) {
            let _ = self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 256-byte aligned payload. Stresses alignment padding inside chunks.
    #[repr(align(256))]
    struct AlignedTracker {
        dropped: Counter,
        _payload: [u8; 64],
    }

    impl AlignedTracker {
        fn new(created: &Counter, dropped: &Counter) -> Self {
            let _ = created.fetch_add(1, Ordering::Relaxed);
            Self {
                dropped: dropped.clone(),
                _payload: [0; 64],
            }
        }
    }

    impl Drop for AlignedTracker {
        fn drop(&mut self) {
            let _ = self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 32 KiB payload — exceeds the default `max_normal_alloc`,
    /// forcing routing through the oversized-chunk path.
    struct LargeTracker {
        dropped: Counter,
        _payload: [u8; 32 * 1024],
    }

    impl LargeTracker {
        fn new(created: &Counter, dropped: &Counter) -> Self {
            let _ = created.fetch_add(1, Ordering::Relaxed);
            Self {
                dropped: dropped.clone(),
                _payload: [0; 32 * 1024],
            }
        }
    }

    impl Drop for LargeTracker {
        fn drop(&mut self) {
            let _ = self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Operations the property test can issue against the arena. Bolero
    /// generates random `Vec<Op>` and runs them in order; the test
    /// asserts the lifecycle invariants at the end.
    ///
    /// The `idx` fields are interpreted modulo the relevant container
    /// length, so any value is meaningful (no rejection sampling needed).
    #[derive(Clone, Copy, Debug, TypeGenerator)]
    enum Op {
        /// `alloc_box(Tracker)` — Box flavor, has a [`DropEntry`].
        AllocBox(u64),
        /// `alloc_arc(Tracker)` — Shared-flavor, deferred-reconciliation
        /// refcount.
        AllocArc(u64),
        /// `Arc::clone` — atomic increment on a `Shared` chunk.
        CloneArc {
            idx: u8,
        },
        /// Drop the `Box<Tracker>` at this index.
        DropBox {
            idx: u8,
        },
        /// Drop the `Arc<Tracker>` at this index.
        DropArc {
            idx: u8,
        },
        /// `Arena::reset` — drains slots + pinned list. Smart pointers
        /// outlive reset (their +1 keeps the chunk alive).
        Reset,
        /// `alloc_str_box` — exercises the str path.
        AllocStrBox {
            len: u8,
        },
        /// `alloc_str_arc` — Shared str refcount.
        AllocStrArc {
            len: u8,
        },
        /// Drop a `Box<str>` at this index.
        DropStrBox {
            idx: u8,
        },
        /// Drop an `Arc<str>` at this index.
        DropStrArc {
            idx: u8,
        },

        AllocZstBox,
        AllocZstArc,
        DropZstBox {
            idx: u8,
        },
        DropZstArc {
            idx: u8,
        },

        AllocAlignedBox,
        AllocAlignedArc,
        DropAlignedBox {
            idx: u8,
        },
        DropAlignedArc {
            idx: u8,
        },

        AllocLargeBox,
        AllocLargeArc,
        DropLargeBox {
            idx: u8,
        },
        DropLargeArc {
            idx: u8,
        },

        /// `arena.alloc_vec()` then push a sequence of trackers, then
        /// freeze into `Arc<[Tracker]>`. `count` controls how many
        /// elements to push (capped to keep state bounded).
        BuildVecArc {
            count: u8,
            payload: u64,
        },
        /// Drop an `Arc<[Tracker]>` previously built.
        DropVecArc {
            idx: u8,
        },

        /// `arena.alloc_vec()` then push, then freeze into
        /// `Box<[Tracker]>`.
        BuildVecBox {
            count: u8,
            payload: u64,
        },
        /// Drop a `Box<[Tracker]>` previously built.
        DropVecBox {
            idx: u8,
        },

        /// `arena.alloc_string()`, push bytes, freeze to `Box<str>`.
        BuildStringBox {
            len: u8,
        },
        DropBuiltStringBox {
            idx: u8,
        },

        #[cfg(feature = "utf16")]
        AllocUtf16StrBox {
            len: u8,
        },
        #[cfg(feature = "utf16")]
        AllocUtf16StrArc {
            len: u8,
        },
        #[cfg(feature = "utf16")]
        DropUtf16StrBox {
            idx: u8,
        },
        #[cfg(feature = "utf16")]
        DropUtf16StrArc {
            idx: u8,
        },
        #[cfg(feature = "utf16")]
        BuildUtf16StringBox {
            len: u8,
        },
        #[cfg(feature = "utf16")]
        DropBuiltUtf16StringBox {
            idx: u8,
        },
    }

    fn run_ops(ops: &[Op]) {
        let created: Counter = StdArc::new(AtomicUsize::new(0));
        let dropped: Counter = StdArc::new(AtomicUsize::new(0));

        let mut arena = Arena::new();
        let mut boxes: Vec<ArenaBox<Tracker>> = Vec::new();
        let mut arcs: Vec<Arc<Tracker>> = Vec::new();
        let mut str_boxes: Vec<multitude::Box<str>> = Vec::new();
        let mut str_arcs: Vec<multitude::Arc<str>> = Vec::new();

        let mut zst_boxes: Vec<ArenaBox<ZstTracker>> = Vec::new();
        let mut zst_arcs: Vec<Arc<ZstTracker>> = Vec::new();

        let mut aligned_boxes: Vec<ArenaBox<AlignedTracker>> = Vec::new();
        let mut aligned_arcs: Vec<Arc<AlignedTracker>> = Vec::new();

        let mut large_boxes: Vec<ArenaBox<LargeTracker>> = Vec::new();
        let mut large_arcs: Vec<Arc<LargeTracker>> = Vec::new();

        let mut vec_arcs: Vec<Arc<[Tracker]>> = Vec::new();
        let mut vec_boxes: Vec<ArenaBox<[Tracker]>> = Vec::new();
        let mut built_str_boxes: Vec<multitude::Box<str>> = Vec::new();

        #[cfg(feature = "utf16")]
        let mut utf16_str_boxes: Vec<multitude::strings::BoxUtf16Str> = Vec::new();
        #[cfg(feature = "utf16")]
        let mut utf16_str_arcs: Vec<multitude::strings::ArcUtf16Str> = Vec::new();
        #[cfg(feature = "utf16")]
        let mut built_utf16_str_boxes: Vec<multitude::strings::BoxUtf16Str> = Vec::new();

        for op in ops {
            match *op {
                Op::AllocBox(payload) => {
                    boxes.push(arena.alloc_box(Tracker::new(&created, &dropped, payload)));
                }
                Op::AllocArc(payload) => {
                    arcs.push(arena.alloc_arc(Tracker::new(&created, &dropped, payload)));
                }
                Op::CloneArc { idx } => {
                    if !arcs.is_empty() {
                        let i = (idx as usize) % arcs.len();
                        let cloned = arcs[i].clone();
                        arcs.push(cloned);
                    }
                }
                Op::DropBox { idx } => {
                    if !boxes.is_empty() {
                        let i = (idx as usize) % boxes.len();
                        drop(boxes.swap_remove(i));
                    }
                }
                Op::DropArc { idx } => {
                    if !arcs.is_empty() {
                        let i = (idx as usize) % arcs.len();
                        drop(arcs.swap_remove(i));
                    }
                }
                Op::Reset => {
                    arena.reset();
                }
                Op::AllocStrBox { len } => {
                    let s = make_str(usize::from(len));
                    str_boxes.push(arena.alloc_str_box(&s));
                }
                Op::AllocStrArc { len } => {
                    let s = make_str(usize::from(len));
                    str_arcs.push(arena.alloc_str_arc(&s));
                }
                Op::DropStrBox { idx } => {
                    if !str_boxes.is_empty() {
                        let i = (idx as usize) % str_boxes.len();
                        drop(str_boxes.swap_remove(i));
                    }
                }
                Op::DropStrArc { idx } => {
                    if !str_arcs.is_empty() {
                        let i = (idx as usize) % str_arcs.len();
                        drop(str_arcs.swap_remove(i));
                    }
                }

                Op::AllocZstBox => {
                    zst_boxes.push(arena.alloc_box(ZstTracker::new(&created, &dropped)));
                }
                Op::AllocZstArc => {
                    zst_arcs.push(arena.alloc_arc(ZstTracker::new(&created, &dropped)));
                }
                Op::DropZstBox { idx } => {
                    if !zst_boxes.is_empty() {
                        let i = (idx as usize) % zst_boxes.len();
                        drop(zst_boxes.swap_remove(i));
                    }
                }
                Op::DropZstArc { idx } => {
                    if !zst_arcs.is_empty() {
                        let i = (idx as usize) % zst_arcs.len();
                        drop(zst_arcs.swap_remove(i));
                    }
                }

                Op::AllocAlignedBox => {
                    aligned_boxes.push(arena.alloc_box(AlignedTracker::new(&created, &dropped)));
                }
                Op::AllocAlignedArc => {
                    aligned_arcs.push(arena.alloc_arc(AlignedTracker::new(&created, &dropped)));
                }
                Op::DropAlignedBox { idx } => {
                    if !aligned_boxes.is_empty() {
                        let i = (idx as usize) % aligned_boxes.len();
                        drop(aligned_boxes.swap_remove(i));
                    }
                }
                Op::DropAlignedArc { idx } => {
                    if !aligned_arcs.is_empty() {
                        let i = (idx as usize) % aligned_arcs.len();
                        drop(aligned_arcs.swap_remove(i));
                    }
                }

                Op::AllocLargeBox => {
                    large_boxes.push(arena.alloc_box(LargeTracker::new(&created, &dropped)));
                }
                Op::AllocLargeArc => {
                    large_arcs.push(arena.alloc_arc(LargeTracker::new(&created, &dropped)));
                }
                Op::DropLargeBox { idx } => {
                    if !large_boxes.is_empty() {
                        let i = (idx as usize) % large_boxes.len();
                        drop(large_boxes.swap_remove(i));
                    }
                }
                Op::DropLargeArc { idx } => {
                    if !large_arcs.is_empty() {
                        let i = (idx as usize) % large_arcs.len();
                        drop(large_arcs.swap_remove(i));
                    }
                }

                Op::BuildVecArc { count, payload } => {
                    let n = (count as usize).min(16);
                    let mut v = arena.alloc_vec::<Tracker>();
                    for i in 0..n {
                        v.push(Tracker::new(&created, &dropped, payload.wrapping_add(i as u64)));
                    }
                    vec_arcs.push(v.into_arena_arc());
                }
                Op::DropVecArc { idx } => {
                    if !vec_arcs.is_empty() {
                        let i = (idx as usize) % vec_arcs.len();
                        drop(vec_arcs.swap_remove(i));
                    }
                }

                Op::BuildVecBox { count, payload } => {
                    let n = (count as usize).min(16);
                    let mut v = arena.alloc_vec::<Tracker>();
                    for i in 0..n {
                        v.push(Tracker::new(&created, &dropped, payload.wrapping_add(i as u64)));
                    }
                    vec_boxes.push(v.into_arena_box());
                }
                Op::DropVecBox { idx } => {
                    if !vec_boxes.is_empty() {
                        let i = (idx as usize) % vec_boxes.len();
                        drop(vec_boxes.swap_remove(i));
                    }
                }

                Op::BuildStringBox { len } => {
                    let mut s = arena.alloc_string();
                    let target = usize::from(len).min(1024);
                    for i in 0..target {
                        s.push(char::from(b'a' + ((i % 26) as u8)));
                    }
                    if target > 4 {
                        s.truncate(target - 1);
                        s.push('!');
                        s.shrink_to_fit();
                    }
                    built_str_boxes.push(s.into_arena_box_str());
                }
                Op::DropBuiltStringBox { idx } => {
                    if !built_str_boxes.is_empty() {
                        let i = (idx as usize) % built_str_boxes.len();
                        drop(built_str_boxes.swap_remove(i));
                    }
                }

                #[cfg(feature = "utf16")]
                Op::AllocUtf16StrBox { len } => {
                    let s = make_utf16_str(usize::from(len));
                    utf16_str_boxes.push(arena.alloc_utf16_str_box(&s));
                }
                #[cfg(feature = "utf16")]
                Op::AllocUtf16StrArc { len } => {
                    let s = make_utf16_str(usize::from(len));
                    utf16_str_arcs.push(arena.alloc_utf16_str_arc(&s));
                }
                #[cfg(feature = "utf16")]
                Op::DropUtf16StrBox { idx } => {
                    if !utf16_str_boxes.is_empty() {
                        let i = (idx as usize) % utf16_str_boxes.len();
                        drop(utf16_str_boxes.swap_remove(i));
                    }
                }
                #[cfg(feature = "utf16")]
                Op::DropUtf16StrArc { idx } => {
                    if !utf16_str_arcs.is_empty() {
                        let i = (idx as usize) % utf16_str_arcs.len();
                        drop(utf16_str_arcs.swap_remove(i));
                    }
                }
                #[cfg(feature = "utf16")]
                Op::BuildUtf16StringBox { len } => {
                    let mut s = arena.alloc_utf16_string();
                    let target = usize::from(len).min(1024);
                    for i in 0..target {
                        s.push(char::from(b'a' + ((i % 26) as u8)));
                    }
                    if target > 4 {
                        s.truncate(target - 1);
                        s.push('!');
                        s.shrink_to_fit();
                    }
                    built_utf16_str_boxes.push(s.into_arena_box_utf16_str());
                }
                #[cfg(feature = "utf16")]
                Op::DropBuiltUtf16StringBox { idx } => {
                    if !built_utf16_str_boxes.is_empty() {
                        let i = (idx as usize) % built_utf16_str_boxes.len();
                        drop(built_utf16_str_boxes.swap_remove(i));
                    }
                }
            }
        }

        drop(boxes);
        drop(str_boxes);
        drop(arcs);
        drop(str_arcs);

        drop(zst_boxes);
        drop(zst_arcs);
        drop(aligned_boxes);
        drop(aligned_arcs);
        drop(large_boxes);
        drop(large_arcs);
        drop(vec_arcs);
        drop(vec_boxes);
        drop(built_str_boxes);
        #[cfg(feature = "utf16")]
        {
            drop(utf16_str_boxes);
            drop(utf16_str_arcs);
            drop(built_utf16_str_boxes);
        }

        #[cfg(feature = "stats")]
        let stats_before_drop = arena.stats();

        drop(arena);

        let created_n = created.load(Ordering::Relaxed);
        let dropped_n = dropped.load(Ordering::Relaxed);
        assert_eq!(
            created_n, dropped_n,
            "every Tracker created must be dropped exactly once (created={created_n}, dropped={dropped_n})"
        );

        #[cfg(feature = "stats")]
        {
            let s = stats_before_drop;
            if created_n > 0 {
                let total_chunks = s.normal_local_chunks_allocated
                    + s.oversized_local_chunks_allocated
                    + s.normal_shared_chunks_allocated
                    + s.oversized_shared_chunks_allocated;
                assert!(
                    total_chunks >= 1,
                    "at least one chunk must have been allocated (created={created_n}, stats={s:?})",
                );
            }
            assert!(
                s.total_bytes_allocated >= (created_n as u64).saturating_mul(8),
                "total_bytes_allocated under-reported (created={created_n}, stats={s:?})",
            );
        }
    }

    fn make_str(len: usize) -> std::string::String {
        let len = len.min(1024);
        let mut s = std::string::String::with_capacity(len);
        for i in 0..len {
            s.push(char::from(b'a' + ((i % 26) as u8)));
        }
        s
    }

    #[cfg(feature = "utf16")]
    fn make_utf16_str(len: usize) -> widestring::Utf16String {
        let len = len.min(1024);
        let mut s = widestring::Utf16String::with_capacity(len);
        for i in 0..len {
            s.push(char::from(b'a' + ((i % 26) as u8)));
        }
        s
    }

    #[test]
    fn lifecycle_invariants() {
        bolero::check!().with_type::<Vec<Op>>().for_each(|ops: &Vec<Op>| {
            run_ops(ops);
        });
    }

    /// LIFO drop pattern: rewrite every `idx` to 0 so drops always pop
    /// the head, exercising stack-discipline ordering.
    #[test]
    fn lifecycle_invariants_lifo_drops() {
        bolero::check!().with_type::<Vec<Op>>().for_each(|ops: &Vec<Op>| {
            let normalized: Vec<Op> = ops
                .iter()
                .map(|op| match *op {
                    Op::CloneArc { .. } => Op::CloneArc { idx: 0 },
                    Op::DropBox { .. } => Op::DropBox { idx: 0 },
                    Op::DropArc { .. } => Op::DropArc { idx: 0 },
                    Op::DropStrBox { .. } => Op::DropStrBox { idx: 0 },
                    Op::DropStrArc { .. } => Op::DropStrArc { idx: 0 },
                    Op::DropZstBox { .. } => Op::DropZstBox { idx: 0 },
                    Op::DropZstArc { .. } => Op::DropZstArc { idx: 0 },
                    Op::DropAlignedBox { .. } => Op::DropAlignedBox { idx: 0 },
                    Op::DropAlignedArc { .. } => Op::DropAlignedArc { idx: 0 },
                    Op::DropLargeBox { .. } => Op::DropLargeBox { idx: 0 },
                    Op::DropLargeArc { .. } => Op::DropLargeArc { idx: 0 },
                    Op::DropVecArc { .. } => Op::DropVecArc { idx: 0 },
                    Op::DropVecBox { .. } => Op::DropVecBox { idx: 0 },
                    Op::DropBuiltStringBox { .. } => Op::DropBuiltStringBox { idx: 0 },
                    #[cfg(feature = "utf16")]
                    Op::DropUtf16StrBox { .. } => Op::DropUtf16StrBox { idx: 0 },
                    #[cfg(feature = "utf16")]
                    Op::DropUtf16StrArc { .. } => Op::DropUtf16StrArc { idx: 0 },
                    #[cfg(feature = "utf16")]
                    Op::DropBuiltUtf16StringBox { .. } => Op::DropBuiltUtf16StringBox { idx: 0 },
                    other => other,
                })
                .collect();
            run_ops(&normalized);
        });
    }

    /// Inject `Op::Reset` between every operation to stress the
    /// eviction + cache-pop interleavings.
    #[test]
    fn lifecycle_invariants_interleaved_reset() {
        bolero::check!().with_type::<Vec<Op>>().for_each(|ops: &Vec<Op>| {
            let mut interleaved = Vec::with_capacity(ops.len() * 2);
            for op in ops {
                interleaved.push(*op);
                interleaved.push(Op::Reset);
            }
            run_ops(&interleaved);
        });
    }
}

// === panic safety property tests ===
mod bolero_panic_safety {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::std_instead_of_alloc, reason = "test code uses std")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_assert_message, reason = "test assertions are self-explanatory")]
    #![allow(clippy::min_ident_chars, reason = "short names in test loops")]
    #![allow(clippy::cast_possible_truncation, reason = "test indices are bounded")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::panic, reason = "test deliberately injects panics to verify recovery")]
    #![allow(clippy::manual_assert, reason = "panic-injection sites are clearer with explicit panic!")]
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc as StdRc;
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use bolero::TypeGenerator;
    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    type Counter = StdArc<AtomicUsize>;

    struct Tracker {
        dropped: Counter,
        _payload: u64,
    }

    impl Tracker {
        fn new(created: &Counter, dropped: &Counter, payload: u64) -> Self {
            let _ = created.fetch_add(1, Ordering::Relaxed);
            Self {
                dropped: dropped.clone(),
                _payload: payload,
            }
        }
    }

    impl Drop for Tracker {
        fn drop(&mut self) {
            let _ = self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct ClonePanicTracker {
        created: Counter,
        dropped: Counter,
        remaining_before_panic: StdRc<Cell<i32>>,
        payload: u64,
        armed: bool,
    }

    impl Clone for ClonePanicTracker {
        fn clone(&self) -> Self {
            let remaining = self.remaining_before_panic.get();
            assert!(remaining > 0, "clone-panic-injection");
            self.remaining_before_panic.set(remaining - 1);
            let _ = self.created.fetch_add(1, Ordering::Relaxed);
            Self {
                created: StdArc::clone(&self.created),
                dropped: StdArc::clone(&self.dropped),
                remaining_before_panic: StdRc::clone(&self.remaining_before_panic),
                payload: self.payload,
                armed: true,
            }
        }
    }

    impl Drop for ClonePanicTracker {
        fn drop(&mut self) {
            if self.armed {
                let _ = self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    impl ClonePanicTracker {
        fn unarmed_dup(&self) -> Self {
            Self {
                created: StdArc::clone(&self.created),
                dropped: StdArc::clone(&self.dropped),
                remaining_before_panic: StdRc::clone(&self.remaining_before_panic),
                payload: self.payload,
                armed: false,
            }
        }
    }

    #[derive(Clone, Copy, Debug, TypeGenerator)]
    enum Op {
        /// `alloc_slice_fill_with(len, f)` where `f` panics on iteration
        /// `panic_at` (capped to `len`). All trackers initialized before
        /// the panic must drop exactly once when the arena drops.
        FillWithPanic { len: u8, panic_at: u8, payload: u64 },
        /// `alloc_slice_clone(&[seed; len])` where the seed's `Clone`
        /// panics on the `panic_at`-th invocation.
        CloneSlicePanic { len: u8, panic_at: u8, payload: u64 },
        /// Successful `alloc_slice_fill_with`.
        FillWithOk { len: u8, payload: u64 },
        /// Successful `alloc_box`.
        AllocBox(u64),
        /// `arena.reset()`.
        Reset,
    }

    fn run_ops(ops: &[Op]) {
        let created: Counter = StdArc::new(AtomicUsize::new(0));
        let dropped: Counter = StdArc::new(AtomicUsize::new(0));

        let mut arena = Arena::new();
        for op in ops {
            match *op {
                Op::FillWithPanic { len, panic_at, payload } => {
                    let n = usize::from(len).min(32);
                    if n == 0 {
                        continue;
                    }
                    let panic_idx = usize::from(panic_at) % n;
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        let _slice = arena.alloc_slice_fill_with::<Tracker, _>(n, |i| {
                            if i == panic_idx {
                                panic!("fill-panic-injection");
                            }
                            Tracker::new(&created, &dropped, payload.wrapping_add(i as u64))
                        });
                    }));
                    assert!(result.is_err(), "fill_with panic must propagate");
                }
                Op::CloneSlicePanic { len, panic_at, payload } => {
                    let n = usize::from(len).min(32);
                    if n == 0 {
                        continue;
                    }
                    let panic_idx = usize::from(panic_at) % n;
                    let panic_idx_i32 = i32::try_from(panic_idx).unwrap_or(i32::MAX);
                    let seed = ClonePanicTracker {
                        created: StdArc::clone(&created),
                        dropped: StdArc::clone(&dropped),
                        remaining_before_panic: StdRc::new(Cell::new(panic_idx_i32)),
                        payload,
                        armed: false,
                    };
                    let src: std::vec::Vec<ClonePanicTracker> = (0..n).map(|_| seed.unarmed_dup()).collect();
                    let baseline_created = created.load(Ordering::Relaxed);
                    let baseline_dropped = dropped.load(Ordering::Relaxed);
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        let _slice = arena.alloc_slice_clone::<ClonePanicTracker>(&src);
                    }));
                    assert!(result.is_err(), "alloc_slice_clone panic must propagate");
                    drop(src);
                    let after_created = created.load(Ordering::Relaxed);
                    let after_dropped = dropped.load(Ordering::Relaxed);
                    assert_eq!(
                        after_created - baseline_created,
                        after_dropped - baseline_dropped,
                        "arena must drop the partially-cloned prefix on panic",
                    );
                }
                Op::FillWithOk { len, payload } => {
                    let n = usize::from(len).min(32);
                    if n == 0 {
                        continue;
                    }
                    let _slice =
                        arena.alloc_slice_fill_with::<Tracker, _>(n, |i| Tracker::new(&created, &dropped, payload.wrapping_add(i as u64)));
                }
                Op::AllocBox(payload) => {
                    let _b = arena.alloc_box(Tracker::new(&created, &dropped, payload));
                }
                Op::Reset => {
                    arena.reset();
                }
            }
        }

        drop(arena);

        let created_n = created.load(Ordering::Relaxed);
        let dropped_n = dropped.load(Ordering::Relaxed);
        assert_eq!(
            created_n, dropped_n,
            "every Tracker created must be dropped exactly once (created={created_n}, dropped={dropped_n})",
        );
    }

    #[test]
    fn panic_safety_invariants() {
        bolero::check!().with_type::<Vec<Op>>().for_each(|ops: &Vec<Op>| {
            run_ops(ops);
        });
    }
}
