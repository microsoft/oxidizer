// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// If a lock is poisoned then safety invariants may have been violated and execution cannot
// continue because we can no longer uphold our security and privacy guarantees.
pub(crate) const ERR_POISONED_LOCK: &str =
    "poisoned lock - cannot continue execution because security and privacy guarantees can no longer be upheld";

/// If a `Sequence` or `SequenceBuilder` needs to track no more than this many spans, its metadata
/// (and only its metadata) will be entirely allocated inline, without a separate heap allocation.
///
/// The idea is that a typical byte sequence is short and at most experiences non-contiguous memory
/// when either giant (in which case a little extra heap allocation may not hurt much) or when
/// encountering boundary conditions in streaming scenarios (in which case the increase in span
/// count is likely only temporary and will remain under this threshold).
///
/// This is purely an efficiency fine-tuning knob and does not have any effect on correctness.
/// We should fine-tune this based on real-world data if/when we get any.
///
/// This is contractually PRIVATE but is marked `pub` so benchmarks can reference it.
/// We may also reference it in other Oxidizer crates to ensure that the same constant is
/// used where it needs to match on higher layers for efficiency. Once we separate this
/// package from Oxidizer, we probably need to make this part of the public API.
#[doc(hidden)]
pub const MAX_INLINE_SPANS: usize = 8;
