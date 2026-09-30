// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;

/// Represents the runtime's internal view over the result of executing a task.
///
/// Before presenting it to user code, we first post-process it (e.g. by re-throwing panics).
/// As far as user code is concerned, only the `R` in `Completed(R)` is ever visible.
///
/// A cancelled task (e.g. due to shutdown) is represented as a missing `TaskResult`.
pub(in crate::task) enum TaskResult<R> {
    Completed(R),
    Panicked(Box<dyn Any + Send + 'static>),
}
