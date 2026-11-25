// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Whole-process contracts for the Rust snmalloc backend.

use GLOBAL as Allocator;

rallocator::rallocator!(backend = snmalloc);

const _: &rallocator::Rallocator<rallocator::backend::Snmalloc> = &GLOBAL;

#[path = "common/allocator.rs"]
mod contracts;
