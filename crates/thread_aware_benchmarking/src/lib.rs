// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))] // Benchmark support is not exercised by the test suite.

//! Shared model for the `thread_aware` relocation benchmarks.

mod model;

pub use model::{Payload, TREE_DEPTH, Tree};
