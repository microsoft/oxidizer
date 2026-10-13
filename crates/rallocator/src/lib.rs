// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! An experimental Rust global allocator for NUMA and mostly thread-local workloads.
//!
//! We use it to experiment with allocation hinting, compile-time configuration,
//! and runtime telemetry through `seismograph`.
//!
//! For now, it is a shameless Rust port of
//! [snmalloc](https://github.com/microsoft/snmalloc), but the implementation has
//! changed before and will probably change again.
//!
//! # Why use it
//!
//! The design targets allocation-heavy multithreaded and thread-per-core
//! workloads: keep ordinary allocations local and amortize cross-thread frees.
//! Built-in Seismograph integration can record allocation events and expose
//! allocator state for diagnosing memory use. Enable recording through
//! [`seismograph::recorder()`](https://docs.rs/seismograph/latest/seismograph/fn.recorder.html)
//! when diagnostics are needed.
//!
//! # Usage
//!
//! Add the dependency:
//!
//! ```toml
//! [dependencies]
//! rallocator = "0.2"
//! ```
//!
//! Call [`rallocator!`] once in your executable to install the global allocator:
//!
//! ```rust
//! rallocator::rallocator!();
//! ```
//!
//! Compile-time allocator configuration has historically been cumbersome to
//! express in a manually declared `#[global_allocator]` static. The macro provides
//! an installation facade that can be extended with configuration options later.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(not(all(
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "linux", target_os = "macos", all(target_os = "windows", target_env = "msvc"))
)))]
compile_error!("rallocator supports x86_64 and aarch64 Linux, macOS and Windows MSVC");

mod abort;
mod backend;
mod buddy;
mod classes;
mod combining;
mod core;
mod hal;
mod observation;
mod pagemap;
mod recording;
mod remote;
mod thread;

use std::alloc::{GlobalAlloc, Layout};

/// A stateless handle to the process's allocator.
#[derive(Debug)]
pub struct Rallocator;

// SAFETY: The core returns disjoint, suitably aligned live allocations and
// routes their destruction to their unique owner; failure is reported as null.
unsafe impl GlobalAlloc for Rallocator {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        recording::allocate(layout, false)
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: GlobalAlloc requires a live allocation from this instance.
        unsafe { recording::deallocate(ptr, layout) };
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        recording::allocate(layout, true)
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: GlobalAlloc guarantees the live allocation and valid new size.
        unsafe { recording::reallocate(ptr, layout, new_size) }
    }
}

#[inline]
unsafe fn reallocate_core(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    let Some(request) = classes::Request::with_size(layout, new_size) else {
        return std::ptr::null_mut();
    };
    // override/rust.cc retains a pointer exactly when the aligned old and
    // new layouts select the same size class, not for arbitrary shrinking.
    if classes::Request::new(layout) == Some(request) {
        return ptr;
    }
    // SAFETY: The caller guarantees ptr is live for layout; the copy bound
    // fits both the old allocation and requested replacement.
    unsafe { thread::reallocate(ptr, request, layout.size().min(new_size)) }
}

/// Defines this crate's allocator as the executable's global allocator.
#[macro_export]
macro_rules! rallocator {
    () => {
        #[global_allocator]
        static GLOBAL: $crate::Rallocator = $crate::Rallocator;
    };
}
