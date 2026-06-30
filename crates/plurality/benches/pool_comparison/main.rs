// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cross-crate allocate+free comparison (Callgrind instruction counts).
//!
//! gungraun needs Valgrind (Linux-only); on other targets this bench compiles
//! to a no-op. The benchmark bodies live in `linux.rs`.

#![cfg_attr(
    target_os = "linux",
    allow(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "triggered by the gungraun main! macro expansion"
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
use linux::*;

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = comparison);
