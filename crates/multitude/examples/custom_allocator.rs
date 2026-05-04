// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Using a custom backing allocator with `Arena<A>`.

#![allow(clippy::missing_safety_doc, reason = "example code")]
#![allow(clippy::std_instead_of_core, reason = "example uses std")]

use core::alloc::Layout;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};

use allocator_api2::alloc::{AllocError, Allocator, Global};
use multitude::Arena;

#[derive(Clone)]
struct CountingAllocator {
    bytes: &'static AtomicUsize,
    chunks: &'static AtomicUsize,
}

// SAFETY: forwards to `Global`, which satisfies the Allocator contract.
unsafe impl Allocator for CountingAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let _ = self.bytes.fetch_add(layout.size(), Ordering::Relaxed);
        let _ = self.chunks.fetch_add(1, Ordering::Relaxed);
        Global.allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarding the same layout we received.
        unsafe {
            Global.deallocate(ptr, layout);
        }
    }
}

static BYTES: AtomicUsize = AtomicUsize::new(0);
static CHUNKS: AtomicUsize = AtomicUsize::new(0);

fn main() {
    let alloc = CountingAllocator {
        bytes: &BYTES,
        chunks: &CHUNKS,
    };
    let arena = Arena::new_in(alloc);

    for i in 0..50_000_u64 {
        let _ = arena.alloc_rc(i);
    }

    println!(
        "after 50_000 alloc()s: {} bytes requested across {} chunk allocations",
        BYTES.load(Ordering::Relaxed),
        CHUNKS.load(Ordering::Relaxed),
    );
    drop(arena);
}
