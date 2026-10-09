// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;

/// Represents the runtime's internal view over the result of executing a task.
///
/// Join handles convert this transport into `Result<R, JoinError>`.
///
/// A dropped sender represents shutdown cancellation or rejection.
pub(in crate::task) enum TaskResult<R> {
    Completed(R),
    Panicked(Box<dyn Any + Send + 'static>),
}
