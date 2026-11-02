// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Blocking worker admission and pool execution.
//!
//! `worker` accepts and runs blocking callbacks. `pools` owns the blocking
//! thread pools. `local_worker` stores thread-local worker state.
//! `panic_policy` applies the configured panic policy.

mod local_worker;
mod panic_policy;
mod pools;
mod worker;

pub(crate) use local_worker::LocalWorker;
pub(crate) use panic_policy::PanicPolicy;
pub(crate) use pools::Pools;
pub(crate) use worker::{Pool, Worker};

pub(crate) type PoolIdentity = performables::arc::Arc<()>;
