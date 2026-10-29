// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;

/// Selects how a contained blocking-callback panic affects the runtime.
///
/// The shutdown variant wakes the associated async worker so it can coordinate
/// shutdown with the other workers.
#[derive(Clone, Debug)]
pub(crate) enum BlockingPanicPolicy {
    Ignore,
    Shutdown(Waker),
}
