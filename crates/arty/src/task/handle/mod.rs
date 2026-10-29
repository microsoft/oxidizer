// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public handles consuming values or panics from task execution.
//!
//! Task execution owns the result transport; join handles await its outcome.

mod channel;
mod error;
mod join_handle;

pub(crate) use channel::{JoinHandlePool, Sender};
pub use error::JoinError;
pub use join_handle::JoinHandle;
