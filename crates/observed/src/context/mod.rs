// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event's context.
//!
//! Enrichment lives in a thread-local slot, so it does not follow work moved to
//! another thread or task on its own. This module carries it across explicitly:
//! [`Transfer`] is the captured, sendable snapshot, and [`Transferred`] is the
//! future wrapper that restores it around every poll.

mod transfer;
mod transferred;

pub use transfer::Transfer;
pub use transferred::Transferred;
