//! Utilities for testing memory management logic.
//!
//! This module contains special-purpose memory providers that are not optimized for real-world
//! usage but may be useful to test corner cases of byte sequence processing in your code.

#[cfg(test)]
mod test_block;

#[cfg(test)]
pub(crate) use test_block::*;

#[cfg(any(test, feature = "test-util"))]
mod fixed_block;

#[cfg(any(test, feature = "test-util"))]
mod transparent;

#[cfg(any(test, feature = "test-util"))]
pub use fixed_block::FixedBlockMemory;
#[cfg(any(test, feature = "test-util"))]
pub use transparent::TransparentMemory;

#[cfg(any(test, feature = "test-util"))]
pub(crate) mod std_alloc_block;
