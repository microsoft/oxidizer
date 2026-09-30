// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public handles consuming values or panics from task execution.
//!
//! Task execution owns the result transport; join handles await its outcome.

mod error;
mod local;
mod remote;

pub use error::JoinError;
pub use local::LocalJoinHandle;
pub use remote::JoinHandle;
