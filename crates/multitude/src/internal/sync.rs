// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Atomic shim that switches between `core::sync::atomic` and
//! `loom::sync::atomic` under `cfg(loom)`.

#[cfg(not(loom))]
pub(crate) use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicU16, AtomicUsize, Ordering, fence};

#[cfg(loom)]
pub(crate) use loom::sync::atomic::{AtomicPtr, AtomicU8, AtomicU16, AtomicUsize, Ordering, fence};
