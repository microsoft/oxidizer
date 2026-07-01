// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Helpers shared across the public-API integration test files.

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Drops increment a shared counter, so tests can assert exact drop counts.
pub(crate) struct DropCounter(pub(crate) StdArc<AtomicUsize>);

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
