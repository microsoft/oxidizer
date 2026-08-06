// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
use std::sync::atomic::{self, AtomicBool};
use std::task::Wake;

/// A waker that simply records whether it was awakened.
pub(crate) struct TestWaker {
    pub(crate) awakened: AtomicBool,
}

impl TestWaker {
    pub(crate) fn new() -> Self {
        Self {
            awakened: AtomicBool::new(false),
        }
    }
}

impl Wake for TestWaker {
    fn wake(self: Arc<Self>) {
        self.awakened.store(true, atomic::Ordering::Relaxed);
    }
}
