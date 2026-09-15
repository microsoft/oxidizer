// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared model for the `thread_aware` relocation benchmarks.
//!
//! ```
//! use thread_aware_benchmarking::{TREE_DEPTH, Tree};
//!
//! let tree = Tree::new();
//!
//! assert!(TREE_DEPTH > 0);
//! assert!(tree.node_count() > 0);
//! ```

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))] // Benchmark support is not exercised by the test suite.

mod model;

pub use model::{Payload, TREE_DEPTH, Tree};
