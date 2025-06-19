// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
/// Allows a completion queue to be woken up when it is blocked waiting for an I/O completion.
///
/// Instances may be cloned and moved to any thread and may outlive the completion queue they are
/// associated with.
pub trait CompletionQueueWaker: Clone + Debug + Send + 'static {
    /// Wakes up the completion queue, either from an ongoing or
    /// upcoming wait for I/O operations in `process_completions()`.
    fn wake(&self);
}