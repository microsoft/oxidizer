// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::time::Duration;

use arty_io_core::Parker;

pub(super) struct NoopParker;

impl Parker for NoopParker {
    fn park(&self, _max_wait: Duration) {}

    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }
}
