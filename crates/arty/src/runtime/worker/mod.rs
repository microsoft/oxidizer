// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Async and blocking worker execution.
//!
//! `Runtime` owns `WorkerJoiner`, which joins the asynchronous worker threads.
//! Each asynchronous worker owns its executor, I/O drivers, and blocking
//! worker. Each blocking worker owns its thread pools. `WorkerMarker` identifies
//! both worker types without destructible thread-local state.

pub(crate) mod asynchronous;
pub(crate) mod blocking;

mod joiner;
mod worker_marker;

pub(crate) use joiner::WorkerJoiner;
pub(crate) use worker_marker::WorkerMarker;
