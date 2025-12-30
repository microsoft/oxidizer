// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Types for using and implementing memory providers.

mod block;
mod block_ref;
mod callback_memory;

mod global;
mod has_memory;
mod memory;
mod memory_shared;
mod opaque_memory;

pub use block::{Block, BlockSize};
pub use block_ref::{BlockRef, BlockRefDynamic, BlockRefDynamicWithMeta, BlockRefVTable};
pub use callback_memory::CallbackMemory;

pub use global::GlobalPool;
pub use has_memory::HasMemory;
pub use memory::Memory;
pub use memory_shared::MemoryShared;
pub use opaque_memory::OpaqueMemory;

#[cfg(any(test, feature = "test-util"))]
pub mod testing;
