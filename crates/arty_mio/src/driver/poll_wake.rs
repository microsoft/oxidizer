// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io;
use std::sync::{Arc, Mutex};
use std::task::Wake;

use mio::Registry;

use crate::worker::DRIVER_WAKE_TOKEN;

pub(super) struct PollWake {
    inner: Mutex<Option<mio::Waker>>,
}

impl PollWake {
    pub(super) fn new(registry: &Registry) -> io::Result<Self> {
        mio::Waker::new(registry, DRIVER_WAKE_TOKEN).map(|inner| Self {
            inner: Mutex::new(Some(inner)),
        })
    }

    pub(super) fn wake_poll(&self) {
        if let Some(waker) = self
            .inner
            .lock()
            .expect("poll wake does not call user code while the native waker lock is held")
            .as_ref()
        {
            let _ = waker.wake();
        }
    }

    pub(super) fn deactivate(&self) {
        self.inner
            .lock()
            .expect("poll wake deactivation only removes the native waker")
            .take();
    }
}

impl Wake for PollWake {
    fn wake(self: Arc<Self>) {
        self.wake_poll();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake_poll();
    }
}
