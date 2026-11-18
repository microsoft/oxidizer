// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(test)]

// Reuse the allocation fixture through the public Block and BlockRef APIs.
#[path = "../../src/mem/testing/std_alloc_block.rs"]
mod std_alloc_block;

mod memory;

pub(crate) use memory::TestMemory;
