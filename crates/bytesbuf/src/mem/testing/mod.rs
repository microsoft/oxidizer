//! Utilities for testing memory management logic.

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
