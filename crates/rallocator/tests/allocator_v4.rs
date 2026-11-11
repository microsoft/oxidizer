// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Whole-process contracts for the default snmalloc backend.

use GLOBAL as Allocator;

rallocator::rallocator!();

const _: &rallocator::Rallocator = &GLOBAL;

#[path = "common/allocator.rs"]
mod contracts;
