// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared scenario metadata for allocation and fat-pointer benchmarks.

/// Distinct routing keys present in the high case of the directory-scan pair.
///
/// The scan is linear, so this count sets the scan length the measured
/// allocation pays. It is high enough for the per-entry slope to clear
/// measurement noise, yet stays in the range of key counts real programs
/// present, which is what the linear scan is chosen for.
/// Ref: docs/implementation/multi-pool.md, "Lookup".
pub(crate) const SPREAD_LAYOUTS: usize = 16;
