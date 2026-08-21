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

pub use arc::Arc;
pub use builtin::{PerCore, PerNuma, PerProcess};
pub(crate) use storage::Strategy;
