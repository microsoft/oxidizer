// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::pal::{CompletionQueueWaker, CompletionQueueWakerFacade};

/// Allows an I/O driver to be woken up when it is blocked waiting for an I/O completion.
///
/// This can be used by one thread to notify another when it has created some non-I/O work for the
/// other thread to do, ensuring that this work is picked up with minimal latency.
///
/// You can clone the waker to share it between many callers or threads.
///
/// # Performance
///
/// The overhead of wake signals is minimal - feel free to invoke wake-up as often as you like.
///
/// The impact of spurious wake-ups depends on what the woken thread is doing besides I/O but if
/// there is no other impactful action, it can be assumed to be in the low microseconds.
///
/// # Lifecycle
///
/// It is legal to use this object even after the I/O driver it came from has been dropped. Wake-ups
/// issued in this state will have no effect, though may consume a small amount of system resources
/// due to unconsumed notifications until the waker is dropped.
///
/// # Thread safety
///
/// This type is thread-safe.
#[derive(Debug, Clone)]
pub struct Waker {
    inner: CompletionQueueWakerFacade,
}

impl Waker {
    pub(crate) const fn new(inner: CompletionQueueWakerFacade) -> Self {
        Self { inner }
    }

    /// Wakes up the target thread, either from an ongoing or upcoming wait for I/O operations.
    pub fn wake(&self) {
        self.inner.wake();
    }
}