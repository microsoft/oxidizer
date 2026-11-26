// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public task-completion, executor-lifecycle, and retained-waker contracts.

mod lifecycle {
    mod completion_lifecycle;
    mod configuration;
    mod shutdown;
    mod support;
    mod waker_lifecycle;
}
