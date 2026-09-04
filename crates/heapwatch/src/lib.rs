// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! A global allocator wrapper that continuously accounts heap usage.
//!
//! Heapwatch wraps another allocator and, installed as a binary's
//! `#[global_allocator]`, reports how many bytes that binary's Rust heap holds
//! and how much has moved through it. It answers *how much*, never *who* — no
//! backtraces, no per-allocation metadata, no pointer-to-size shadow map —
//! which is what keeps the per-allocation cost to a few non-atomic adds, low
//! enough to leave enabled in production.
//!
//! # Status
//!
//! **The API is not implemented yet — this crate is currently scaffolding.**
//! The architecture is settled and written up in [`DESIGN.md`], and the ideas
//! deliberately left out of it are recorded in [`TODO.md`]. Both land ahead of
//! the code so the design can be reviewed on its own terms.
//!
//! # The mechanism
//!
//! Each thread accumulates its own totals with plain, non-atomic arithmetic and
//! publishes them into the allocator instance's atomic totals in batches — once
//! *bytes allocated plus bytes freed* since its last publication crosses a
//! compile-time threshold, and again when the thread exits. That removes the
//! atomic read-modify-write per allocation that an exact accounting wrapper
//! pays, which is the cost that scales badly with core count.
//!
//! The trade is a small, bounded, stated inaccuracy: a reading omits whatever
//! each live thread has not yet published, at most the threshold times the
//! number of live threads. That bound depends on neither the allocation rate
//! nor how long the process has been running. Reading is O(1) in the thread
//! count — a handful of relaxed loads through a handle obtained from the
//! allocator, with no registry to walk.
//!
//! # Measurement boundary
//!
//! Heapwatch counts successful calls through the registered `GlobalAlloc`, and
//! counts them as *requested* rather than as reserved, since the trait has no
//! hook to ask the inner allocator what it actually rounded up to. Allocations
//! that never reach the global allocator — native and FFI heaps, direct OS
//! mappings, anything routed to `std::alloc::System` — are outside the
//! boundary, as are thread stacks, static data, and the inner allocator's own
//! metadata and fragmentation. It therefore complements, rather than replaces,
//! allocator-native statistics and process-level metrics such as resident set
//! size.
//!
//! [`DESIGN.md`]: https://github.com/microsoft/oxidizer/blob/main/crates/heapwatch/docs/DESIGN.md
//! [`TODO.md`]: https://github.com/microsoft/oxidizer/blob/main/crates/heapwatch/docs/TODO.md
