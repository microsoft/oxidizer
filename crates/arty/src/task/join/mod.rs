// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public handles consuming values or panics from task execution.
//!
//! Task execution owns the result transport; join handles await its outcome.

mod error;
mod remote;

pub use error::JoinError;
pub use remote::JoinHandle;
