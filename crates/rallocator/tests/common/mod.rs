// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::{GlobalAlloc, Layout};
use std::hint::black_box;
use std::ptr::NonNull;

/// Owns one allocation and its independently reproducible expected contents.
#[derive(Debug)]
pub(crate) struct Block<'a, A: GlobalAlloc> {
    allocator: &'a A,
    pointer: NonNull<u8>,
    layout: Layout,
    tag: u64,
}

// SAFETY: A block has unique ownership, exposes no borrowed payload, and transfers
// its allocation together with the allocator and layout needed to release it.
// Sharing the allocator between allocating and freeing threads requires `Sync`.
unsafe impl<A: GlobalAlloc + Sync> Send for Block<'_, A> {}

impl<'a, A: GlobalAlloc> Block<'a, A> {
    pub(crate) fn new(allocator: &'a A, layout: Layout, tag: u64, zeroed: bool) -> Self {
        assert_ne!(layout.size(), 0, "GlobalAlloc requires a nonempty layout");
        let address = if zeroed {
            // SAFETY: The valid, nonempty allocation is uniquely owned by this block.
            unsafe { allocator.alloc_zeroed(layout) }
        } else {
            // SAFETY: The valid, nonempty allocation is uniquely owned by this block.
            unsafe { allocator.alloc(layout) }
        };
        let mut block = Self {
            allocator,
            pointer: NonNull::new(black_box(address)).unwrap(),
            layout,
            tag,
        };
        assert_eq!(block.address() % layout.align(), 0, "misaligned allocation: {layout:?}");
        if zeroed {
            // SAFETY: alloc_zeroed must initialize all requested bytes.
            let bytes = unsafe { std::slice::from_raw_parts(black_box(address), layout.size()) };
            assert!(bytes.iter().all(|byte| *byte == 0), "alloc_zeroed returned dirty bytes: {layout:?}");
        }
        block.paint(tag);
        block
    }

    pub(crate) const fn layout(&self) -> Layout {
        self.layout
    }

    pub(crate) fn address(&self) -> usize {
        self.pointer.as_ptr().addr()
    }

    pub(crate) fn paint(&mut self, tag: u64) {
        self.tag = tag;
        // SAFETY: The block exclusively owns the entire writable allocation.
        let bytes = unsafe { std::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.layout.size()) };
        for (index, chunk) in bytes.chunks_mut(8).enumerate() {
            let pattern = pattern(tag, index);
            chunk.copy_from_slice(&pattern[..chunk.len()]);
        }
        black_box(self.pointer.as_ptr());
    }

    pub(crate) fn check(&self) {
        assert_eq!(self.address() % self.layout.align(), 0, "misaligned live allocation");
        self.check_prefix(self.layout.size());
    }

    fn check_prefix(&self, size: usize) {
        assert!(size <= self.layout.size());
        // SAFETY: Allocation initializes the whole payload. After reallocation,
        // callers inspect only the prefix the GlobalAlloc contract preserves.
        let bytes = unsafe { std::slice::from_raw_parts(black_box(self.pointer.as_ptr()), size) };
        for (index, chunk) in bytes.chunks(8).enumerate() {
            let expected = pattern(self.tag, index);
            assert_eq!(
                chunk,
                &expected[..chunk.len()],
                "payload corruption: tag={}, layout={:?}, offset={}",
                self.tag,
                self.layout,
                index * 8
            );
        }
    }

    pub(crate) fn reallocate(&mut self, new_size: usize) {
        assert_ne!(new_size, 0, "GlobalAlloc::realloc requires a nonzero size");
        self.check();
        let new_layout = Layout::from_size_align(new_size, self.layout.align()).unwrap();
        let preserved = self.layout.size().min(new_size);
        // SAFETY: The pointer and old layout describe this block's live
        // allocation; the new size is nonzero and valid for the same alignment.
        let address = unsafe { self.allocator.realloc(self.pointer.as_ptr(), self.layout, new_size) };
        let pointer = NonNull::new(black_box(address)).unwrap();
        // Update ownership before assertions so unwinding frees the new block,
        // never the old allocation that a successful realloc has consumed.
        self.pointer = pointer;
        self.layout = new_layout;
        assert_eq!(self.address() % self.layout.align(), 0, "realloc lost alignment");
        self.check_prefix(preserved);
        self.paint(self.tag);
    }
}

impl<A: GlobalAlloc> Drop for Block<'_, A> {
    fn drop(&mut self) {
        // SAFETY: This non-Copy owner releases its live allocation exactly once,
        // using the same allocator and the current (possibly resized) layout.
        unsafe { self.allocator.dealloc(self.pointer.as_ptr(), self.layout) };
    }
}

pub(crate) fn check_disjoint<A: GlobalAlloc>(blocks: &[Block<'_, A>]) {
    for (index, block) in blocks.iter().enumerate() {
        let start = block.address();
        let end = start.checked_add(block.layout().size()).unwrap();
        for other in &blocks[..index] {
            let other_start = other.address();
            let other_end = other_start.checked_add(other.layout().size()).unwrap();
            assert!(
                end <= other_start || other_end <= start,
                "live allocations overlap: {start:#x}..{end:#x} and {other_start:#x}..{other_end:#x}"
            );
        }
    }
    for block in blocks {
        block.check();
    }
}

fn pattern(tag: u64, index: usize) -> [u8; 8] {
    tag.wrapping_add(u64::try_from(index).unwrap().wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .rotate_left(23)
        .to_le_bytes()
}
