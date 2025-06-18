// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::sync::atomic::{self, AtomicUsize};
use std::sync::{Arc, Mutex};

use crate::pal::{
    CompletionQueue, CompletionQueueWaker, CompletionQueueWakerFacade, MockCompletionNotification,
    PrimitiveFacade,
};

pub type CompletionNotificationSource = Arc<Mutex<VecDeque<MockCompletionNotification>>>;

/// A simulated completion queue that returns completions one at a time when polled, with the
/// completions obtained from a shared queue.
///
/// Designed to be used in scenarios where a mock platform is used, as simulating a completion queue
/// via mockall is problematic due to lack of proper support for self-reference return types.
///
/// Completions are retrieved from a `VecDeque` supplied at creation time.
#[derive(Debug)]
pub struct SimulatedCompletionQueue {
    completed: CompletionNotificationSource,

    wake_signals_received: Arc<AtomicUsize>,
}

impl SimulatedCompletionQueue {
    /// Creates a completion queue that receives completions from the provided source.
    #[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
    pub(crate) fn new(
        completed: CompletionNotificationSource,
        wake_signals_received: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            completed,
            wake_signals_received,
        }
    }

    /// Creates a completion queue that never receives any completions.
    #[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
    pub(crate) fn new_disconnected() -> Self {
        Self {
            completed: Arc::new(Mutex::new(VecDeque::new())),
            wake_signals_received: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl CompletionQueue for SimulatedCompletionQueue {
    fn bind(&self, _primitive: &PrimitiveFacade) -> crate::Result<()> {
        Ok(())
    }

    fn process_completions<CB>(&mut self, _max_wait_time_millis: u32, mut cb: CB)
    where
        CB: FnMut(&crate::pal::CompletionNotificationFacade),
    {
        let Some(new_entry) = self.completed.lock().unwrap().pop_front() else {
            return;
        };

        cb(&new_entry.into());
    }

    fn waker(&self) -> CompletionQueueWakerFacade {
        SimulatedCompletionQueueWaker {
            wake_signals_received: Arc::clone(&self.wake_signals_received),
        }
        .into()
    }
}

#[derive(Clone, Debug)]
pub struct SimulatedCompletionQueueWaker {
    // There is no actual "sleeping" in the simulated queue so all we can do is count.
    wake_signals_received: Arc<AtomicUsize>,
}

impl CompletionQueueWaker for SimulatedCompletionQueueWaker {
    fn wake(&self) {
        self.wake_signals_received
            .fetch_add(1, atomic::Ordering::Relaxed);
    }
}