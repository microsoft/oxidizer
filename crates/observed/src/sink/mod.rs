// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sink types: identity id and core sink.

mod core;
mod id;
mod recursion_guard;

pub use core::Sink;

pub use id::SinkId;
use recursion_guard::try_acquire_reentrancy_guard;
