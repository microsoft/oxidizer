// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Rust allocation machinery derived from mimalloc.
//!
//! Reference: mimalloc commit `39448a96aae7eab35840b2a9d19adaf175cf9b34`.
//! Upstream copyright and permission notice: `LICENSE-MIMALLOC`.
//!
//! The allocation paths use the non-hardened 64-bit configuration of the single
//! default heap. Native statistics, profiling, and guarded sampling are excluded.
//! The backend does not call or link the native allocator.
//!
//! A page holds equally sized blocks. `heap` owns the three free lists, page
//! queues, retirement, and the atomic protocol for remote frees. `abandoned`
//! indexes reusable pages with per-arena atomic bitmaps. A claimed bitmap bit
//! protects metadata until the reader acquires page ownership or restores the bit.
//!
//! `memory` owns the shared arenas, commit state, and delayed purge.
//! `hal` contains the platform operations. `pagemap` finds aligned metadata
//! within 256-MiB regions. Small pages keep their metadata before their blocks.
//! Rust determines the metadata size, so its physical layout differs from C.
//!
//! The supported interface is Rust's global allocator. The separate C APIs for
//! explicit heaps, custom arenas, runtime configuration, and block visitation
//! are not exposed.

mod abandoned;
mod allocator;
mod atomic;
mod classes;
mod hal;
mod heap;
mod memory;
mod observation;
mod pagemap;

pub use allocator::Mimalloc;
