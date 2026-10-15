// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Async worker execution, wake notification, and executor shutdown.

mod async_worker;
pub(super) mod protocol;
pub(super) mod signal;

pub(super) use async_worker::AsyncWorker;
