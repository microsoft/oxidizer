// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sink types: identity id, core sink, and its builder.

mod builder;
mod core;
mod id;
mod recursion_guard;

pub use core::Sink;

pub use builder::SinkBuilder;
pub use id::SinkId;
use recursion_guard::try_acquire_reentrancy_guard;
