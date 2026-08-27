// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Utilities for testing code that uses `bytesbuf_io` abstractions.

#![allow(unknown_lints, reason = "the pinned and latest Clippy versions expose different async lints")]

mod fake_read;
mod fake_write;
mod null;
mod pending;

pub use fake_read::*;
pub use fake_write::*;
pub use null::*;
pub use pending::*;
