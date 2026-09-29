// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;

use arty_io_core::{PendingWork, PendingWorkTracker};

/// A no-op tracker for sample drivers that neither wait nor start background work.
///
/// Real I/O drivers require a tracker that implements interruption and completion barriers.
#[derive(Debug)]
pub(crate) struct Coordinator;

impl PendingWorkTracker for Coordinator {
    fn start_work(&mut self, _interrupt: Waker) -> PendingWork {
        PendingWork::new(Waker::noop().clone())
    }
}
