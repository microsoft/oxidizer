// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases how a type can support an "optimal path" if an optimized memory
//! configuration is used, reverting to a less optimal "fallback path" if
//! non-optimized memory is used (e.g. from `GlobalPool`).
//!
//! The most common use case for this is when performing I/O operations using operating
//! system provided I/O APIs. These often have preferences on what sort of memory they
//! want to accept, with the optimal APIs only being usable if you use the right kind of
//! memory in the right ways (e.g. pre-registering it, aligning it and so on).
//!
//! More advanced scenarios (not showcased here) also include memory that is mapped to specific
//! physical memory modules (e.g. memory on a specific GPU or other accelerator device).

use std::num::NonZero;

use bytesbuf::mem::{BlockSize, CallbackMemory, GlobalPool, HasMemory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};

fn main() {
    // In real-world code, both of these would be provided by the application framework.
    let global_memory_pool = GlobalPool::new();
    let io_context = IoContext::new();

    let mut connection = Connection::new(io_context);
    let connection_memory = connection.memory();

    // This message uses the connection's memory provider, which provides optimal memory.
    let message1 = BytesView::copied_from_slice(b"Example message 1: hello, world!", &connection_memory);
    connection.write(message1.clone());

    // This message uses the global memory pool, which does not provide optimal memory.
    let message2 = BytesView::copied_from_slice(b"Message 2: goodbye", &global_memory_pool);
    connection.write(message2.clone());

    // This message uses a combination of both memory providers. This will not use the optimal
    // I/O path because the optimal I/O path requires all memory in a byte sequence to be optimal.
    let message3 = BytesView::from_views([message1, message2]);
    connection.write(message3);
}

/// We implement a network connection that can send data to a remote endpoint.
///
/// This type offers two implementations for sending data:
///
/// 1. If the memory comes from the I/O pool and is pre-registered with the operating system,
///    we use an optimal path for maximum performance.
/// 2. Otherwise, we use a fallback path that is somewhat slower.
///
/// The choice of implementation is made transparently - the caller writing bytes to the
/// connection does not need to know or understand which implementation is used.
#[derive(Debug)]
struct Connection {
    io_context: IoContext,
}

impl Connection {
    pub const fn new(io_context: IoContext) -> Self {
        Self { io_context }
    }

    pub fn write(&mut self, message: BytesView) {
        // We now need to identify whether the message actually uses memory that allows us to
        // use the optimal I/O path. There is no requirement that the data passed to us contains
        // only memory with our preferred configuration.

        let use_optimal_path = message.slices().all(|(_, meta)| {
            // If there is no metadata, the memory is not I/O memory.
            meta.is_some_and(|meta| {
                // If the type of metadata does not match the metadata
                // exposed by the I/O memory provider, the memory is not I/O memory.
                let Some(io_memory_configuration) = meta.downcast_ref::<MemoryConfiguration>() else {
                    return false;
                };

                // If the memory is I/O memory but is not not pre-registered
                // with the operating system, we cannot use the optimal path.
                io_memory_configuration.requires_registered_memory
            })
        });

        if use_optimal_path {
            self.write_optimal(message);
        } else {
            self.write_fallback(message);
        }
    }

    #[expect(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_self,
        clippy::needless_pass_by_value,
        reason = "for example realism"
    )]
    fn write_optimal(&mut self, message: BytesView) {
        println!("Sending message of {} bytes using optimal path.", message.len());
    }

    #[expect(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_self,
        clippy::needless_pass_by_value,
        reason = "for example realism"
    )]
    fn write_fallback(&mut self, message: BytesView) {
        println!("Sending message of {} bytes using fallback path.", message.len());
    }
}

/// Represents the optimal memory configuration for a network connection when reserving I/O memory.
const CONNECTION_OPTIMAL_MEMORY_CONFIGURATION: MemoryConfiguration = MemoryConfiguration {
    requires_page_alignment: false,
    zero_memory_on_release: false,
    requires_registered_memory: true,
};

impl HasMemory for Connection {
    fn memory(&self) -> impl MemoryShared {
        CallbackMemory::new({
            // Cloning is cheap, as it is a service that shares resources between clones.
            let io_context = self.io_context.clone();

            move |min_len| io_context.reserve_io_memory(min_len, CONNECTION_OPTIMAL_MEMORY_CONFIGURATION)
        })
    }
}

// ###########################################################################
// Everything below this comment is dummy logic to make the example compile.
// The useful content of the example is the code above.
// ###########################################################################

#[derive(Clone, Debug)]
struct IoContext;

impl IoContext {
    pub const fn new() -> Self {
        Self {}
    }

    #[expect(clippy::unused_self, reason = "for example realism")]
    pub fn reserve_io_memory(&self, min_len: usize, memory_configuration: MemoryConfiguration) -> BytesBuf {
        let min_len: BlockSize = min_len
            .try_into()
            .expect("this example is limited to max allocation size of BlockSize, just to keep it simple");

        let Some(min_len) = NonZero::new(min_len) else {
            return BytesBuf::new();
        };

        let block = io_memory::allocate(min_len, memory_configuration);

        BytesBuf::from_blocks([block])
    }
}

#[expect(dead_code, reason = "unused fields just for example realism")]
struct MemoryConfiguration {
    requires_page_alignment: bool,
    zero_memory_on_release: bool,
    requires_registered_memory: bool,
}

/// Minimal dummy implementation of an I/O memory manager. All it does is
/// remember the memory configuration that was requested in the memory block metadata,
/// thereby pretending the memory has that configuration.
mod io_memory {
    use std::alloc::{Layout, alloc, dealloc};
    use std::any::Any;
    use std::mem::MaybeUninit;
    use std::num::NonZero;
    use std::ptr::{self, NonNull};
    use std::sync::atomic::{self, AtomicUsize};

    use bytesbuf::mem::{Block, BlockRef, BlockRefDynamic, BlockRefDynamicWithMeta, BlockRefVTable, BlockSize};

    use super::MemoryConfiguration;

    /// Allocates a new memory block of the given length and returns a `BlockRef` to it.
    #[must_use]
    pub fn allocate(len: NonZero<BlockSize>, memory_configuration: MemoryConfiguration) -> Block {
        let block_ptr = new_block(len, memory_configuration);

        // SAFETY: We just created that memory block, so it is valid for reads.
        let block = unsafe { block_ptr.as_ref() };

        // SAFETY: block_ptr must remain valid for reads and writes until drop()
        // is called via the dynamic fns. Yep, it does - the dynamic impl type takes ownership.
        let block_ref = unsafe { BlockRef::new(block_ptr, &BLOCK_REF_FUNCTION_TABLE) };

        // SAFETY: We guarantee exclusive access to the memory capacity - nobody else gets it.
        unsafe { Block::new(block.ptr, block.len, block_ref) }
    }

    struct IoMemoryBlock {
        memory_configuration: MemoryConfiguration,
        ptr: NonNull<MaybeUninit<u8>>,
        len: NonZero<BlockSize>,
        ref_count: AtomicUsize,
    }

    // SAFETY: We must guarantee thread-safety. We do.
    unsafe impl BlockRefDynamic for IoMemoryBlock {
        type State = Self;

        fn clone(state_ptr: NonNull<Self::State>) -> NonNull<Self::State> {
            // SAFETY: The state pointer is always valid for reads.
            let state = unsafe { state_ptr.as_ref() };

            state.ref_count.fetch_add(1, atomic::Ordering::Relaxed);

            // We reuse the state for all clones.
            state_ptr
        }

        fn drop(state_ptr: NonNull<Self::State>) {
            // SAFETY: The state pointer is always valid for reads.
            let state = unsafe { state_ptr.as_ref() };

            if state.ref_count.fetch_sub(1, atomic::Ordering::Release) != 1 {
                return;
            }

            // This was the last reference, so we can deallocate the block.

            // Ensure that we have observed all writes into the block from other threads.
            // On x86 this does nothing but on weaker memory models writes could be delayed.
            atomic::fence(atomic::Ordering::Acquire);

            // First we deallocate the block's capacity.
            // SAFETY: Layout must match between allocation and deallocation. It does.
            unsafe { dealloc(state.ptr.as_ptr().cast(), byte_array_layout(state.len)) };

            // Then we deallocate the block object itself.
            // SAFETY: Layout must match between allocation and deallocation. It does.
            unsafe {
                dealloc(state_ptr.as_ptr().cast(), BLOCK_LAYOUT);
            }
        }
    }

    // SAFETY: We must guarantee thread-safety. We do.
    unsafe impl BlockRefDynamicWithMeta for IoMemoryBlock {
        fn meta(state_ptr: NonNull<Self::State>) -> NonNull<dyn Any> {
            // SAFETY: The state pointer is always valid for reads.
            let state = unsafe { state_ptr.as_ref() };

            // Safe to pointerize because the parent API contract requires that
            // this has a lifetime that is a subset of the lifetime of `data`.
            let as_any: &dyn Any = &state.memory_configuration;

            NonNull::new(ptr::from_ref(as_any).cast_mut()).expect("field of non-null is non-null")
        }
    }

    const BLOCK_REF_FUNCTION_TABLE: BlockRefVTable<IoMemoryBlock> = BlockRefVTable::from_trait_with_meta();

    fn byte_array_layout(len: NonZero<BlockSize>) -> Layout {
        Layout::array::<u8>(len.get() as usize).expect("the layout of a byte array can always be determined")
    }

    // SAFETY: We are asking for the layout of a valid Rust type and passing its natural size
    // and alignment - nothing can go wrong.
    const BLOCK_LAYOUT: Layout = unsafe { Layout::from_size_align_unchecked(size_of::<IoMemoryBlock>(), align_of::<IoMemoryBlock>()) };

    fn new_block(len: NonZero<BlockSize>, memory_configuration: MemoryConfiguration) -> NonNull<IoMemoryBlock> {
        // SAFETY: Layout must be non-zero and otherwise sane.
        // It is - we use NonZero for len to ensure non-zero size.
        let capacity_ptr = NonNull::new(unsafe { alloc(byte_array_layout(len)) })
            .expect("we do not intend to handle failed allocations - they are fatal")
            .cast::<MaybeUninit<u8>>();

        // SAFETY: Layout must be non-zero and otherwise sane.
        // It is - we know that Block is a normal type and we have a normal layout for it.
        let block_ptr = NonNull::new(unsafe { alloc(BLOCK_LAYOUT) })
            .expect("we do not intend to handle failed allocations - they are fatal")
            .cast::<IoMemoryBlock>();

        let block = IoMemoryBlock {
            ptr: capacity_ptr,
            len,
            ref_count: AtomicUsize::new(1),
            memory_configuration,
        };

        // SAFETY: We just allocated that memory with a proper layout, so it is both valid
        // for writes and properly aligned.
        unsafe { block_ptr.write(block) };

        block_ptr
    }
}
