// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The Oxidizer I/O subsystem provides mechanisms to execute low-level I/O operations on operating
//! system I/O primitives (file handles, sockets, pipes, ...). These mechanisms underpin
//! higher-level I/O endpoint types like `TcpConnection`, `Socket` and `File`, which themselves
//! are not part of the I/O subsystem and are offered by other Oxidizer crates like `oxidizer_net`.
//!
//! Design goals of the I/O subsystem include:
//!
//! * Focus on high-throughput asynchronous zero-copy I/O.
//! * Native support for vectored I/O.
//! * Efficient operation even on systems with 200+ processors.
//! * Common API for all supported operating systems, which today are:
//!     * Windows 11 or newer
//!     * Windows Server 2022 or newer
//!
//! The I/O subsystem consists of the following major components, each relevant for a different
//! audience:
//!
//! 1. Engineers who use I/O endpoints like `File` and `TcpClient` will need to use
//!    [the provided types for manipulating the input/output byte sequences][19] and managing their
//!    storage in memory owned by the I/O subsystem.
//! 1. Engineers implementing I/O endpoints will want to familiarize themselves with the nature
//!    of the [I/O context][1], which provides the functionality to bind native I/O
//!    primitives to the I/O subsystem, enabling I/O operations to be started on them.
//! 1. Engineers implementing async task runtimes that require I/O capability will need to use
//!    the [I/O driver][5] that interacts with the operating system to facilitate the processing
//!    of low-level asynchronous I/O operations started on bound I/O primitives.
//!
//! [1]: crate::Context
//! [5]: crate::Driver
//! [19]: crate::mem

#![cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "We compile on Linux just to verify that we do not blatantly write Windows-only code but there is no Linux PAL, so only mock PAL tests work. As a result, there can be a lot of dead code."
    )
)]

pub mod mem {
    pub use oxidizer_mem::*;
}

pub(crate) mod pal;
pub mod testing;

mod constants;
mod context;
mod driver;
mod error;
mod internal_macros;
mod operations;
mod primitives;
mod reserve_options;
mod resources;
mod runtime;
mod streams;
mod thread_safe;
mod waker;

pub(crate) use constants::ERR_POISONED_LOCK;
pub use context::*;
pub use driver::*;
pub use error::*;
pub(crate) use internal_macros::nz;
pub use operations::*;
pub use primitives::*;
pub use reserve_options::*;
pub(crate) use resources::Resources;
pub use runtime::*;
pub use streams::*;
pub use waker::*;

#[cfg(any(feature = "fakes", test))]
mod fake;
#[cfg(any(feature = "fakes", test))]
pub use fake::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(feature = "hyper")]
pub mod hyper;

#[cfg(test)]
mod tests {
    #[test]
    fn is_64_bit() {
        // This crate requires at least pointers to be 64 bits long.
        // We have various size/pointer/offset logic that assumes this.
        // If we ever want to target 32-bit, we likely need to adjust the math in many places
        // because while reaching u64::MAX is never going to happen with reasonable inputs,
        // u32::MAX is easy to reach even with reasonable inputs (4 GB is nothing!).
        static_assertions::const_assert!(size_of::<usize>() >= 8);
    }
}