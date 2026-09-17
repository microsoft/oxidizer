// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! One shared offload thread, kept alive by owner leases and accepted work, not inert handles.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::Waker;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use arty_io_core::{DriverError, SystemTask, SystemTasks};

pub(super) struct ExecutionLease {
    sender: mpsc::Sender<AcceptedTask>,
}

struct AcceptedTask {
    task: SystemTask,
    execution: Arc<ExecutionLease>,
}

pub(super) struct SystemPool {
    execution: Option<Arc<ExecutionLease>>,
    finished: mpsc::Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl SystemPool {
    pub(super) fn start() -> Result<Self, DriverError> {
        let (sender, receiver) = mpsc::channel::<AcceptedTask>();
        let (finished_tx, finished) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("in-memory-io-system".into())
            .spawn(move || {
                while let Ok(work) = receiver.recv() {
                    work.task.run();
                    drop(work.execution);
                }
                // The owner can time out and abandon the join without invalidating this thread.
                let _ = finished_tx.send(());
            })
            .map_err(DriverError::from_cause)?;
        Ok(Self {
            execution: Some(Arc::new(ExecutionLease { sender })),
            finished,
            thread: Some(thread),
        })
    }

    pub(super) fn execution_lease(&self) -> Arc<ExecutionLease> {
        Arc::clone(
            self.execution
                .as_ref()
                .expect("execution leases are acquired before controller shutdown"),
        )
    }

    pub(super) fn handle(&self) -> SystemTasks {
        let execution = Arc::downgrade(
            self.execution
                .as_ref()
                .expect("system-task handles are created before controller shutdown"),
        );
        SystemTasks::new(move |task| {
            let execution = execution
                .upgrade()
                .ok_or_else(|| DriverError::from_message("system work has no remaining execution owner"))?;
            // Acceptance transfers execution authority to the job, including any work it submits.
            // The sender disappears only after owners AND accepted jobs retire.
            let job_execution = Arc::clone(&execution);
            execution
                .sender
                .send(AcceptedTask {
                    task,
                    execution: job_execution,
                })
                .map_err(|error| DriverError::from_message(format!("system work was not accepted: {error}")))
        })
    }

    pub(super) fn stop(&mut self, deadline: Instant) -> Result<(), DriverError> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        // There is no Stop marker that can overtake a retained owner's later cleanup. Inert
        // SystemTasks handles are weak; they cannot keep graceful shutdown pending by themselves.
        drop(self.execution.take());
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
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn wait_stopped(&self, timeout: std::time::Duration) -> Result<(), mpsc::RecvTimeoutError> {
        self.finished.recv_timeout(timeout)
    }
}

/// A callback owns this completion flag independently of the driver or drain.
pub(super) struct Cleanup {
    complete: Arc<AtomicBool>,
    submission_error: Option<DriverError>,
}

impl Cleanup {
    pub(super) fn start(tasks: &SystemTasks, ready: Waker) -> Self {
        let complete = Arc::new(AtomicBool::new(false));
        let callback_complete = Arc::clone(&complete);
        let submission_error = tasks
            .spawn(move || {
                // Release publishes cleanup before the source's publish-then-wake notification.
                callback_complete.store(true, Ordering::Release);
                ready.wake();
            })
            .err();
        Self {
            complete,
            submission_error,
        }
    }

    pub(super) fn check_complete(&mut self) -> Result<bool, DriverError> {
        if let Some(error) = self.submission_error.take() {
            return Err(error);
        }
        // Acquire observes everything preceding the callback's completion publication.
        Ok(self.complete.load(Ordering::Acquire))
    }
}

pub(super) fn report(message: &str) {
    // Unlike eprintln!, a failed stderr write must not cause a second panic during unwinding.
    let _ = writeln!(io::stderr().lock(), "{message}");
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
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
            tasks.spawn(move || sender.send(thread::current().id()).unwrap()).unwrap();
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
        pool.handle()
            .spawn(move || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                completed_tx.send(resource.to_string()).unwrap();
            })
            .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(pool.stop(Instant::now()).unwrap_err().is_shutdown_timeout());
        assert!(weak.upgrade().is_some());
        release_tx.send(()).unwrap();
        assert_eq!(completed_rx.recv_timeout(Duration::from_secs(5)).unwrap(), "callback-owned");
        pool.finished.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn accepted_work_retains_authority_for_followup_work_after_controller_timeout() {
        let mut pool = SystemPool::start().unwrap();
        let tasks = pool.handle();
        let retained_handle = tasks.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        pool.handle()
            .spawn(move || {
                let owner = thread::current().id();
                entered_tx.send(owner).unwrap();
                resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                tasks.spawn(move || finished_tx.send(thread::current().id()).unwrap()).unwrap();
            })
            .unwrap();
        let owner = entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(pool.stop(Instant::now()).unwrap_err().is_shutdown_timeout());
        resume_tx.send(()).unwrap();
        assert_eq!(finished_rx.recv_timeout(Duration::from_secs(5)).unwrap(), owner);
        pool.wait_stopped(Duration::from_secs(5)).unwrap();
        drop(retained_handle);
    }

    #[test]
    fn an_inert_handle_reports_rejection_after_execution_retires() {
        let mut pool = SystemPool::start().unwrap();
        let tasks = pool.handle();
        pool.stop(Instant::now() + Duration::from_secs(5)).unwrap();
        let ran = Arc::new(AtomicBool::new(false));
        let task_ran = Arc::clone(&ran);
        let error = tasks.spawn(move || task_ran.store(true, Ordering::Relaxed)).unwrap_err();
        assert!(error.to_string().contains("no remaining execution owner"));
        assert!(!ran.load(Ordering::Relaxed));
    }
}
