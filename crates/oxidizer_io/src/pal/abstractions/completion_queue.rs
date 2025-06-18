// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use crate::pal::{CompletionNotificationFacade, CompletionQueueWakerFacade, PrimitiveFacade};

/// Allows I/O primitives to be bound to itself, after which it receives completion notifications
/// whenever I/O operations complete on that I/O primitive.
///
/// The I/O driver of each I/O capable thread owns and operates a completion queue to which
/// all I/O primitives on the same thread must be bound at creation time.
///
/// # Ownership
///
/// Shared ownership via I/O driver resources. Intended to be RefCell-governed.
///
/// # Thread safety
///
/// No thread safety is required from implementations (no `Send` or `Sync` bounds).
pub trait CompletionQueue: Debug {
    /// Binds an I/O primitive to the completion queue, causing the queue to receive all
    /// completion notifications for this primitive.
    ///
    /// An I/O primitive can only be bound to a single completion queue and, once bound, cannot
    /// be unbound.
    ///
    /// Completion notifications are only received for asynchronous I/O. If an operation completes
    /// synchronously, no notification will be received for that operation.
    fn bind(&self, primitive: &PrimitiveFacade) -> crate::Result<()>;

    /// Polls the completion queue for new completion notifications and executes the provided
    /// callback for each received completion notification, waiting for up to
    /// `max_wait_time_millis` when there are no completion notifications pending.
    ///
    /// Returns when the first batch of completions has been processed or when the max wait
    /// time is reached, whichever is soonest.
    fn process_completions<CB>(&mut self, max_wait_time_millis: u32, cb: CB)
    where
        CB: FnMut(&CompletionNotificationFacade);

    /// Creates a new waker that can be used to wake up the completion queue when it is blocked
    /// waiting for I/O operations.
    fn waker(&self) -> CompletionQueueWakerFacade;
}