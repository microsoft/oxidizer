// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Internal building blocks of the `multitude` arena allocator.
//!
//! Everything here is `pub(crate)`. The public API lives one level above.

pub(crate) mod arena_buf;
pub(crate) mod chunk;
pub(crate) mod chunk_alloc;
pub(crate) mod chunk_mutator;
pub(crate) mod chunk_ops;
pub(crate) mod chunk_provider;
pub(crate) mod chunk_ref;
pub(crate) mod constants;
pub(crate) mod current_chunk;
pub(crate) mod drop_entry;
pub(crate) mod in_chunk;
pub(crate) mod local_chunk;
pub(crate) mod owner_thread_cell;
pub(crate) mod shared_chunk;
pub(crate) mod thin_dst;
pub(crate) mod uninit;

pub(crate) use chunk::Chunk;
