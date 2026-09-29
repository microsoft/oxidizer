// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! OS-thread lifecycle, joining, and the blocking-context guard.

mod blocking;
mod lifecycle;
pub(super) mod waiter;

pub(crate) use blocking::assert_not_flagged;
pub(super) use blocking::flag_current_thread;
pub(super) use lifecycle::spawn;
