// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public task-completion, executor-lifecycle, and retained-waker contracts.

#[path = "lifecycle/completion_lifecycle.rs"]
mod completion_lifecycle;
#[path = "lifecycle/support.rs"]
mod support;
#[path = "lifecycle/waker_lifecycle.rs"]
mod waker_lifecycle;
