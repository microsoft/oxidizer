// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Receiving task results through `JoinHandle`.
//!
//! Await a task's [`JoinHandle`] to receive its result or a [`JoinError`].

mod channel;
mod error;
mod join_handle;

pub(crate) use channel::{JoinHandlePool, Sender};
pub use error::JoinError;
pub use join_handle::JoinHandle;
