//! Allocator telemetry APIs.

mod core;
pub(crate) use core::*;
pub use core::{snapshot, stats, track_callers};

pub mod snapshot;
pub mod stats;
