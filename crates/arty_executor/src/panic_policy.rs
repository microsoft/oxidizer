// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Selects how the executor responds to a contained task panic.
#[derive(Clone, Debug, Default)]
pub enum PanicPolicy {
    /// Start executor shutdown with a panic cause.
    #[default]
    Shutdown,

    /// Contain the failed task and continue running other tasks.
    Ignore,
}
