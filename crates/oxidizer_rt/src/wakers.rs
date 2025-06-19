// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// A waker that can be used to wake up a thread.
///
/// This is useful for workers that are waiting for work to do.
///
/// This struct is Cloneable and thread-safe.
#[derive(Debug, Clone)]
pub struct ThreadWaker {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

#[expect(
    clippy::mutex_atomic,
    reason = "we are using the mutex to construct an event primitive"
)]
#[expect(
    clippy::allow_attributes,
    reason = "lint failure is conditional on build configuration"
)]
#[allow(
    dead_code,
    reason = "conditionally used with some build configurations"
)]
impl ThreadWaker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    // Mutants are skipped on this because it's impossible to test without timing making the test very fragile.
    // Since this code will end up being replaced with an IO specific waker in the future, it's not worth the effort.
    // `wait` is *never* going to be slower than the implementation it replaces.

    /// Notifies the waiting thread and wakes it up.
    #[mutants::skip]
    pub fn notify(&self) {
        *self.inner.0.lock().unwrap() = true;
        self.inner.1.notify_one();
    }

    /// Waits for the waker to be notified.
    #[mutants::skip]
    pub fn wait(&self, timeout: Duration) {
        let mut notified = self.inner.0.lock().unwrap();
        while !*notified {
            let x = self.inner.1.wait_timeout(notified, timeout).unwrap();
            notified = x.0;
            let t = x.1;
            if t.timed_out() {
                return;
            }
        }
        *notified = false;
    }
}