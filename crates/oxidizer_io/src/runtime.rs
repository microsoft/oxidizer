// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

/// Functionality that the I/O subsystem expects the runtime environment to provide.
///
/// # Enqueued tasks on drop
///
/// The I/O subsystem does not expect the runtime to wait for all spawned tasks to complete if it is
/// dropped, or to cancel them - the runtime implementation is free to decide for itself what it
/// does with remaining tasks when dropped.
///
/// # Thread safety
///
/// Implementations are expected to be thread-safe and may be called from any thread (including
/// from inside tasks scheduled on the runtime).
///
/// The runtime may execute enqueued tasks on any thread.
pub trait Runtime: Send + Sync + Debug {
    /// Enqueues a synchronous task intended to make a potentially blocking call into the the
    /// operating system.
    ///
    /// The runtime environment should do its best to execute the task as soon as possible.
    /// It should consider that the task may block the thread for several seconds.
    ///
    /// The method will not wait for the task to execute, returning immediately.
    fn enqueue_system_task(&self, category: SystemTaskCategory, body: SystemTask);

    /// Enqueues an asynchronous task on an arbitrary thread.
    ///
    /// The runtime environment should do its best to execute the task as soon as possible.
    /// These tasks will not block the thread, typically lasting less than a millisecond.
    ///
    /// The method will not wait for the task to execute, returning immediately.
    fn enqueue_task(&self, body: AsyncTask);
}

/// An async task that can be enqueued on the runtime environment of the I/O subsystem.
///
/// All tasks enqueued by the I/O subsystem are thread-mobile (`Send`) to allow
/// the runtime environment to freely select the thread on which they will be executed.
pub type AsyncTask = Box<dyn Future<Output = ()> + Send + 'static>;

/// A synchronous system call task that can be enqueued on the runtime environment of the I/O subsystem.
///
/// All tasks enqueued by the I/O subsystem are thread-mobile (`Send`) to allow
/// the runtime environment to freely select the thread on which they will be executed.
pub type SystemTask = Box<dyn FnOnce() + Send + 'static>;

/// Different system tasks may be given different treatment by the runtime environment
/// depending on their nature.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SystemTaskCategory {
    /// The task is scheduled to release resources and should therefore be prioritized above
    /// other types of system tasks, as releasing resources quickly can be performance-critical.
    ReleaseResources,

    /// The default category to be used when no other category is a match.
    Default,
}