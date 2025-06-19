// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fakes enabled by the `fakes` feature. Primarily intended to help users test their I/O logic.
//!
//! Internal testing logic is also exposed under `unstable-testing` for purpose of examples and
//! integration testing but this is not an officially supported API and may change at any time.

mod fake_read_stream;
mod fake_write_stream;
mod null_stream;
mod pending_stream;

pub use fake_read_stream::*;
pub use fake_write_stream::*;
pub use null_stream::*;
pub use pending_stream::*;