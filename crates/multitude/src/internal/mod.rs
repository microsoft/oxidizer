// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Internal building blocks of the `multitude` arena allocator.
//!
//! Everything here is `pub(crate)`. The public API lives one level above.

pub(crate) mod arena_buf;
pub(crate) mod chunk;
pub(in crate::internal) mod chunk_alloc;
pub(crate) mod chunk_mutator;
pub(crate) mod chunk_provider;
pub(crate) mod chunk_ref;
pub(crate) mod constants;
pub(crate) mod current_chunk;
pub(crate) mod drop_entry;
pub(in crate::internal) mod in_chunk;
pub(crate) mod thin_dst;
pub(crate) mod uninit;

pub(crate) use chunk::Chunk;
