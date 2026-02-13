// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Types for using and implementing memory providers.
//!
//! The only production-grade memory provider published by this crate is [`GlobalPool`], which uses
//! memory from the Rust global memory allocator and adds a layer of pooling to reduce the overhead
//! from memory allocation churn.
//!
//! Special-purpose memory providers can be implemented by other crates as needed, providing access
//! to memory with particular characteristics (e.g. page-aligned memory, memory mapped to specific
//! physical devices, ...).
//!
//! # Implementing a memory provider
//!
//! A memory provider must implement the [`Memory`] trait and should implement the [`MemoryShared`] trait.
//! The ultimate purpose of a memory provider is to create a [`BytesBuf`] to which is given the
//! requested number of bytes of memory capacity.
//!
//! The high-level workflow for this is as follows:
//!
//! 1. The memory provider receives a `reserve(N)` call.
//! 1. The memory provider prepares any number of memory blocks (of any size) so that their total capacity
//!    is at least `N` bytes. A memory block is conceptually just a pointer to some usable memory and a length,
//!    though the memory provider may wish to associate it with some additional metadata for its own purposes.
//! 1. For each memory block, the memory provider creates a [`BlockRef`] that references the memory block,
//!    associating it with a custom state object that contains the block metadata (at minimum, a reference count).
//!    Either a [`BlockRefDynamic`] or [`BlockRefDynamicWithMeta`] is used to associate provider-specific
//!    behaviors with the memory block (e.g. what to do on [`BlockRef`] clone or release).
//! 1. The memory provider creates a [`Block`] for each [`BlockRef`], asserting that the caller will be the
//!    exclusive owner of these memory blocks. Only exclusively owned memory can be written into by a [`BytesBuf`].
//! 1. The memory provider passes the [`Block`] objects to [`BytesBuf::from_blocks()`] to create a [`BytesBuf`]
//!    and returns it to the caller.
//!
//! [`BytesBuf`]: crate::BytesBuf
//! [`BytesBuf::from_blocks()`]: crate::BytesBuf::from_blocks

mod block;
mod block_ref;
mod callback_memory;

mod global;
mod has_memory;
mod memory;
mod memory_shared;
mod opaque_memory;

pub use block::{Block, BlockSize};
pub use block_ref::{BlockMeta, BlockRef, BlockRefDynamic, BlockRefDynamicWithMeta, BlockRefVTable};
pub use callback_memory::CallbackMemory;
pub use global::GlobalPool;
pub use has_memory::HasMemory;
pub use memory::Memory;
pub use memory_shared::MemoryShared;
pub use opaque_memory::OpaqueMemory;

#[cfg(any(test, feature = "test-util"))]
pub mod testing;
