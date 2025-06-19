// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;

use crate::{AsyncTask, CycleResult};

/// Abstraction of `AsyncTaskExecutor`.
///
/// Exists only for mocking purposes - no real alternative implementations are expected.
pub trait AbstractAsyncTaskExecutor {
    fn enqueue(&mut self, task: Pin<Box<dyn AsyncTask>>);
    fn execute_cycle(&mut self) -> CycleResult;
    fn begin_shutdown(&mut self);
}