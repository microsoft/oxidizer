// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Poll;

/// Observes the top-level polls of one executor task.
pub trait TaskObserver: 'static {
    /// Runs immediately before the executor polls the task.
    fn poll_start(&mut self) {}

    /// Runs immediately after the executor polls the task.
    fn poll_end(&mut self, _outcome: Poll<()>) {}
}

impl TaskObserver for () {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TaskState {
    Ready,
    Waiting,
}
