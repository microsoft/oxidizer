// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicUsize, Ordering};

// Keep the conditional-update primitive on the workspace's minimum compiler.
// Retry semantics belong to std rather than separate allocator CAS loops.
#[expect(
    clippy::allow_attributes,
    reason = "The minimum compiler does not deprecate this API, so expect(deprecated) cannot cover every supported compiler"
)]
#[allow(deprecated, reason = "AtomicUsize::try_update is newer than the Rust 1.95 minimum")]
pub(super) fn try_update(
    atomic: &AtomicUsize,
    success: Ordering,
    failure: Ordering,
    update: impl FnMut(usize) -> Option<usize>,
) -> Result<usize, usize> {
    atomic.fetch_update(success, failure, update)
}
