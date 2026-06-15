// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the bytesbuf integration module.

#![cfg(feature = "bytesbuf")]
#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use bytesbuf::mem::Memory;
use multitude::Arena;

#[test]
fn reserve_and_consume_basic() {
    let arena = Arena::new();
    let mut buf = arena.reserve(64);
    buf.put_slice(*b"hello");
    let view = buf.consume_all();
    assert_eq!(view.len(), 5);
    assert_eq!(view, b"hello");
}

/// Fast-fail micro-test for the `BlockRef::drop` last-drop cleanup
/// gate (`bytesbuf.rs::ArenaBlockState::drop`'s `if prev == 1`).
/// Under the `==` → `!=` mutation the cleanup runs on every drop
/// *except* the last, so the chunk hold is never released and the
/// arena chunk leaks to the tracking allocator. The targeted shape
/// (single reserve, immediate drop, then arena drop) makes this
/// observation roughly 1ms instead of ~78s through the stress suite.
#[test]
fn reserve_drop_releases_arena_chunk_hold() {
    let alloc = common::SendTrackingAllocator::new();
    {
        let arena = Arena::builder_in(alloc.clone()).build();
        let buf = arena.reserve(64);
        drop(buf);
    }
    assert_eq!(
        alloc.live_chunks(),
        0,
        "BlockRef::drop last-drop branch must release the chunk hold"
    );
    assert_eq!(alloc.live_bytes(), 0);
}

#[test]
fn reserve_exact_size() {
    let arena = Arena::new();
    let mut buf = arena.reserve(5);
    buf.put_slice(*b"exact");
    let view = buf.consume_all();
    assert_eq!(view, b"exact");
}

#[test]
fn reserve_large() {
    let arena = Arena::new();
    let mut buf = arena.reserve(4096);
    // Write 4096 bytes in chunks of 256
    for chunk_idx in 0..16u8 {
        let mut chunk = [0u8; 256];
        for (i, byte) in chunk.iter_mut().enumerate() {
            *byte = u8::try_from((usize::from(chunk_idx) * 256 + i) % 256).unwrap();
        }
        buf.put_slice(chunk);
    }
    let view = buf.consume_all();
    assert_eq!(view.len(), 4096);
    let slice = view.first_slice();
    assert!(slice.len() >= 256);
    assert_eq!(slice[0], 0);
    assert_eq!(slice[255], 255);
}

#[test]
fn reserve_zero_returns_empty_buf() {
    let arena = Arena::new();
    let mut buf = arena.reserve(0);
    let view = buf.consume_all();
    assert!(view.is_empty());
}

#[test]
fn multiple_reserves_independent() {
    let arena = Arena::new();
    let mut buf1 = arena.reserve(32);
    let mut buf2 = arena.reserve(32);

    buf1.put_slice(*b"first");
    buf2.put_slice(*b"second");

    let v1 = buf1.consume_all();
    let v2 = buf2.consume_all();
    assert_eq!(v1, b"first");
    assert_eq!(v2, b"second");
}

#[test]
fn view_outlives_arena() {
    let view;
    {
        let arena = Arena::new();

        let mut buf = arena.reserve(32);
        buf.put_slice(*b"persist");
        view = buf.consume_all();
    }
    // Arena dropped; the BytesView data should still be valid.
    assert_eq!(view, b"persist");
}

#[test]
fn view_clone_shares_data() {
    let arena = Arena::new();
    let mut buf = arena.reserve(32);
    buf.put_slice(*b"shared");
    let v1 = buf.consume_all();
    let v2 = v1.clone();
    assert_eq!(v1.len(), v2.len());
    assert_eq!(v1.first_slice(), v2.first_slice());
}

#[test]
fn view_clone_outlives_original() {
    let arena = Arena::new();
    let mut buf = arena.reserve(32);
    buf.put_slice(*b"cloned");
    let v1 = buf.consume_all();
    let v2 = v1.clone();
    drop(v1);
    assert_eq!(v2, b"cloned");
}

#[test]
fn stress_many_reserves() {
    let arena = Arena::new();
    let mut views = Vec::new();
    for i in 0u32..100 {
        let mut buf = arena.reserve(16);
        buf.put_slice(i.to_le_bytes());
        views.push(buf.consume_all());
    }
    for (i, view) in views.iter().enumerate() {
        let expected = u32::try_from(i).unwrap().to_le_bytes();
        assert_eq!(view.first_slice(), &expected[..]);
    }
}

#[test]
fn view_send_across_thread() {
    let arena = Arena::new();
    let mut buf = arena.reserve(32);
    buf.put_slice(*b"thread-safe");
    let view = buf.consume_all();

    let h = std::thread::spawn(move || {
        assert_eq!(view, b"thread-safe");
        view.len()
    });
    assert_eq!(h.join().unwrap(), 11);
}

#[test]
fn arena_implements_memory_trait() {
    let arena = Arena::new();
    // Use arena directly as a Memory impl
    let mut buf = arena.reserve(16);
    buf.put_slice(*b"trait");
    let view = buf.consume_all();
    assert_eq!(view, b"trait");
}

#[test]
fn single_ref_drop_cleanup() {
    // Create a view and ensure exactly one reference exists, then drop it.
    // This exercises the `was == 1` branch in ArenaBlockState::drop.
    let arena = Arena::new();
    let mut buf = arena.reserve(16);
    buf.put_slice(*b"drop me");
    let view = buf.consume_all();
    // No clone — single reference
    drop(view);
}

#[test]
fn single_ref_drop_after_arena_dropped() {
    let view;
    {
        let arena = Arena::new();

        let mut buf = arena.reserve(16);
        buf.put_slice(*b"outlive");
        view = buf.consume_all();
    }
    // Arena dropped, view holds sole reference
    assert_eq!(view, b"outlive");
    drop(view); // exercises full cleanup with arena gone
}

mod from_coverage_extras_bytesbuf {
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
    use multitude::Arena;

    #[expect(unused_imports, reason = "relocated tests may reference common helpers")]
    use crate::common;

    #[test]
    fn bytesbuf_drop_releases_arena_chunk() {
        use core::sync::atomic::{AtomicIsize, Ordering};
        use std::sync::Arc as StdArc;

        use allocator_api2::alloc::{AllocError, Allocator, Global, Layout};
        use bytesbuf::mem::Memory;

        #[derive(Clone)]
        struct CountingAllocator {
            live: StdArc<AtomicIsize>,
        }
        // SAFETY: forwards to Global; counter is atomic.
        unsafe impl Allocator for CountingAllocator {
            fn allocate(&self, layout: Layout) -> Result<core::ptr::NonNull<[u8]>, AllocError> {
                let p = Global.allocate(layout)?;
                self.live.fetch_add(1, Ordering::Relaxed);
                Ok(p)
            }
            unsafe fn deallocate(&self, ptr: core::ptr::NonNull<u8>, layout: Layout) {
                // SAFETY: caller's contract.
                unsafe { Global.deallocate(ptr, layout) };
                self.live.fetch_sub(1, Ordering::Relaxed);
            }
        }

        let live = StdArc::new(AtomicIsize::new(0));
        let alloc = CountingAllocator {
            live: StdArc::clone(&live),
        };
        let arena = Arena::builder().allocator_in(alloc).build();

        {
            let mut buf = arena.reserve(4 * 1024);
            buf.put_slice([0_u8; 4 * 1024]);
            let view = buf.consume_all();
            assert_eq!(view.len(), 4 * 1024);
            drop(view);
        }
        drop(arena);

        assert_eq!(live.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn bytesbuf_reserve_refill_failure_through_allocate_shared_layout() {
        // Drive `allocate_shared_layout` (3112)'s refill failure path via bytesbuf.
        use bytesbuf::mem::Memory;
        let alloc = common::SendFailingAllocator::new(1);
        let arena = Arena::builder_in(alloc).max_normal_alloc(4096).try_build().unwrap();
        // First few `reserve` calls succeed (within the initial chunk). Eventually
        // a `reserve` will need a fresh chunk and fail; we catch the panic from
        // bytesbuf's `expect("arena allocation failed")` wrapper.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for _ in 0..32 {
                let _ = arena.reserve(2048);
            }
        }));
        assert!(result.is_err(), "expected bytesbuf reserve to fail eventually");
    }
}
