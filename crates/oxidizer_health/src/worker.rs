// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Weak};
use std::thread::{self, JoinHandle};

use crate::component::ComponentShared;

pub(crate) enum Work {
    Component(Weak<ComponentShared>),
    Stop,
}

/// Both process and component handles keep this worker alive.
pub(crate) struct Worker {
    sender: Sender<Work>,
    thread: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

impl Worker {
    pub(crate) fn new() -> Arc<Self> {
        let (sender, receiver) = mpsc::channel();
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let spawned = thread::Builder::new()
            .name(String::from("oxidizer-health"))
            .spawn(move || run(&receiver, worker_running));
        Arc::new(Self::from_spawn(sender, running, spawned))
    }

    fn from_spawn(sender: Sender<Work>, running: Arc<AtomicBool>, spawned: io::Result<JoinHandle<()>>) -> Self {
        let thread = match spawned {
            Ok(thread) => Some(thread),
            Err(error) => {
                // Release: state readers observe that no worker is available.
                running.store(false, Ordering::Release);
                tracing::error!(%error, "failed to start health aggregation worker");
                None
            }
        };
        Self { sender, thread, running }
    }

    pub(crate) fn sender(&self) -> Sender<Work> {
        self.sender.clone()
    }

    pub(crate) fn is_running(&self) -> bool {
        // Acquire: pairs with the Release store when the worker fails or stops.
        self.running.load(Ordering::Acquire)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        // A stopped worker has already reported its startup failure or panic.
        let _ = self.sender.send(Work::Stop);
        if thread.thread().id() == thread::current().id() {
            // A wake callback released the last owner on this thread. The queued stop is
            // consumed when that callback returns; a worker cannot join itself.
            return;
        }
        if thread.join().is_err() {
            tracing::error!("health aggregation worker terminated unexpectedly");
        }
    }
}

fn run(receiver: &Receiver<Work>, running: Arc<AtomicBool>) {
    let _running = Running(running);
    while let Ok(Work::Component(component)) = receiver.recv() {
        if let Some(component) = component.upgrade() {
            component.process();
        }
    }
}

struct Running(Arc<AtomicBool>);

impl Drop for Running {
    fn drop(&mut self) {
        // Release: state readers fail closed if the worker has stopped.
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_failure_marks_worker_unavailable_until_shutdown() {
        let (sender, receiver) = mpsc::channel();
        let worker = Worker::from_spawn(sender, Arc::new(AtomicBool::new(true)), Err(io::ErrorKind::WouldBlock.into()));
        assert!(!worker.is_running());
        drop(worker);
        assert!(matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected)));
    }
}
