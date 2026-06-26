// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Atomic primitives, routed through this module so the concurrency tests
//! can swap in [`loom`](https://docs.rs/loom)'s instrumented atomics under
//! `--cfg loom` to exhaustively explore thread interleavings. In normal builds
//! these are the `core::sync::atomic` types with zero overhead.

pub(crate) use core::sync::atomic::Ordering;
#[cfg(not(loom))]
pub(crate) use core::sync::atomic::{AtomicU32, AtomicUsize, fence};

#[cfg(loom)]
pub(crate) use loom::sync::atomic::{AtomicU32, AtomicUsize, fence};
