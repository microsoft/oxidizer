// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Asynchronous I/O abstractions expressed via `bytesbuf` types.
//!
//! These types model byte sources that can be read from ([`Read`] trait) and byte sinks that can be
//! written to ([`Write`] trait). All operations use byte sequences represented by types from
//! `bytesbuf` instead of raw byte slices, enabling the level of flexibility required for
//! implementing and using high-performance I/O endpoints that consume or produce byte streams.
//!
//! All operations are asynchronous and take ownership of the data/buffers passed to them,
//! enabling efficient implementation of high-performance I/O endpoints with zero-copy semantics.
//!
//! The `futures-stream` feature enables integration with the `futures` crate, providing
//! an adapter that exposes a [`Read`] implementation as a `futures::Stream` of byte sequences.
//!
//! The `test-util` feature enables additional utilities for testing implementations of
//! types that produce or consume streams of bytes. These are in the `testing` module.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/bytesbuf_io/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/bytesbuf_io/favicon.ico")]

mod error;
mod read;
mod read_ext;
#[cfg(feature = "futures-stream")]
mod read_futures;
mod write;
mod write_ext;

pub use error::{Error, Result};
pub use read::Read;
pub use read_ext::{ReadExt, ReadInspectDecision};
#[cfg(feature = "futures-stream")]
pub use read_futures::ReadAsFuturesStream;
pub use write::Write;
pub use write_ext::WriteExt;

#[cfg(any(test, feature = "test-util"))]
pub mod testing;
