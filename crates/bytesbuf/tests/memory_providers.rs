// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Memory providers, block references, and their public contracts.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

mod support;

mod memory_providers {
    mod block_ref;
    mod callback;
    mod fixed_block;
    mod global;
    mod memory;
    mod opaque;
    mod transparent;
}
