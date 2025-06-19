// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// In scenarios where multiple instances of a task are spawned, identifies the specific instance
/// that is being spawned by index (e.g. to facilitate work partitioning by instance index).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct SpawnInstance {
    index: usize,
    count: usize,
}

impl SpawnInstance {
    #[must_use]
    pub const fn new(index: usize, count: usize) -> Self {
        Self { index, count }
    }

    /// Returns the zero-based index of the current instance of the task.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// Returns the total number of instances of the task being spawned.
    #[must_use]
    pub const fn count(&self) -> usize {
        self.count
    }
}