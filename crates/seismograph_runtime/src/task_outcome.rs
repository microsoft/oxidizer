// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Terminal outcome shared by asynchronous and blocking tasks.
///
/// Pass this value to [`asynchronous::Poll::finished`] or
/// [`blocking::Execution::finished`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TaskOutcome {
    /// The task returned normally.
    Completed,
    /// The runtime canceled the task.
    Cancelled,
    /// The task panicked.
    Panicked,
}

impl TaskOutcome {
    pub(crate) const fn event_value(&self) -> u64 {
        match self {
            Self::Completed => 1,
            Self::Cancelled => 2,
            Self::Panicked => 3,
        }
    }
}
