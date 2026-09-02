// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod arc;
mod clone_fn;
mod factory;
pub mod storage;

mod builtin;
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests;

pub use arc::{Arc, FromStorageError};
pub use builtin::{PerNumaNode, PerProcess, PerThread};
pub(crate) use storage::Strategy;
