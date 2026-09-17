// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! One shared offload thread, kept alive until every worker finishes draining.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::Waker;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use arty_io_core::{DriverError, SystemTask, SystemTasks};

enum Command {
    Run(SystemTask),
    Stop,
}

pub(super) struct SystemPool {
    sender: mpsc::Sender<Command>,
    finished: mpsc::Receiver<()>,
    thread: Option<JoinHandle<()>>,
    failure: Arc<Mutex<Option<String>>>,
}

impl SystemPool {
    pub(super) fn start() -> Result<Self, DriverError> {
        let (sender, receiver) = mpsc::channel();
        let (finished_tx, finished) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("in-memory-io-system".into())
            .spawn(move || {
                while let Ok(Command::Run(task)) = receiver.recv() {
                    task();
                }
                // The owner can time out and abandon the join without invalidating this thread.
                let _ = finished_tx.send(());
            })
            .map_err(DriverError::from_cause)?;
        Ok(Self {
            sender,
            finished,
            thread: Some(thread),
            failure: Arc::default(),
        })
    }

    pub(super) fn handle(&self) -> SystemTasks {
        let sender = self.sender.clone();
        let failure = Arc::clone(&self.failure);
        SystemTasks::new(move |task| {
            if let Err(error) = sender.send(Command::Run(task)) {
                let message = format!("system work was not accepted: {error}");
                report(&message);
                *failure
                    .lock()
                    .expect("only a panicking system-task submitter can poison this mutex") = Some(message);
            }
        })
    }

    pub(super) fn stop(&mut self, deadline: Instant) -> Result<(), DriverError> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        // A disconnected receiver is diagnosed by the exit notification or join below.
        drop(self.sender.send(Command::Stop));
        match self.finished.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                thread.join().map_err(|payload| {
                    drop(payload);
                    DriverError::from_message("the shared system-work thread panicked")
                })?;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // The detached thread still owns its current callback and all of its captures.
                drop(thread);
                return Err(DriverError::shutdown_timeout());
            }
        }
        let failure = self
            .failure
            .lock()
            .expect("only a panicking system-task submitter can poison this mutex")
            .take();
        if let Some(message) = failure {
            return Err(DriverError::from_message(message));
        }
        Ok(())
    }
}

/// A callback owns this completion flag independently of the driver or drain.
pub(super) struct Cleanup {
    complete: Arc<AtomicBool>,
}

impl Cleanup {
    pub(super) fn start(tasks: &SystemTasks, ready: Waker) -> Self {
        let complete = Arc::new(AtomicBool::new(false));
        let callback_complete = Arc::clone(&complete);
        tasks.spawn(move || {
            // Release publishes cleanup before the source's publish-then-wake notification.
            callback_complete.store(true, Ordering::Release);
            ready.wake();
        });
        Self { complete }
    }

    pub(super) fn is_complete(&self) -> bool {
        // Acquire observes everything preceding the callback's completion publication.
        self.complete.load(Ordering::Acquire)
    }
}

pub(super) fn report(message: &str) {
    // Unlike eprintln!, a failed stderr write must not cause a second panic during unwinding.
    let _ = writeln!(io::stderr().lock(), "{message}");
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test setup and assertions use the test backtrace")]
mod tests {
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::SystemPool;

    #[test]
    fn all_system_work_uses_one_shared_thread() {
        let mut pool = SystemPool::start().unwrap();
        let tasks = pool.handle();
        let (sender, receiver) = mpsc::channel();
        for _ in 0..4 {
            let sender = sender.clone();
            tasks.spawn(move || sender.send(thread::current().id()).unwrap());
        }
        let owner = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_ne!(owner, thread::current().id());
        for _ in 0..3 {
            assert_eq!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(), owner);
        }
        pool.stop(Instant::now() + Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn timed_out_system_work_retains_its_callback_storage() {
        let mut pool = SystemPool::start().unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (completed_tx, completed_rx) = mpsc::channel();
        let resource = Arc::new(String::from("callback-owned"));
        let weak = Arc::downgrade(&resource);
        pool.handle().spawn(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            completed_tx.send(resource.to_string()).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(pool.stop(Instant::now()).unwrap_err().is_shutdown_timeout());
        assert!(weak.upgrade().is_some());
        release_tx.send(()).unwrap();
        assert_eq!(completed_rx.recv_timeout(Duration::from_secs(5)).unwrap(), "callback-owned");
        pool.finished.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(weak.upgrade().is_none());
    }
}
