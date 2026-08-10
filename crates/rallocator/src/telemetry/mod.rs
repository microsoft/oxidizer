// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocator telemetry APIs.

mod core;
pub(crate) use core::*;
pub use core::{SnapshotError, SnapshotErrorKind, snapshot, stats, track_callers, try_snapshot};

pub mod snapshot;
pub mod stats;
