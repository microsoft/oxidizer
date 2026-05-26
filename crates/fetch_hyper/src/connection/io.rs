// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`HyperIo`] trait alias plus a [`Connection`] impl for
//! [`Box<dyn HyperIo>`], so a heap-allocated, type-erased stream can be used
//! directly wherever a [`HyperIo`] is required. The matching [`Read`]/
//! [`Write`] impls for `Box<T>` are provided by `hyper` itself.

use hyper::rt::{Read, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};

/// A trait alias for any I/O stream usable by hyper.
///
/// A [`Box<dyn HyperIo>`] itself implements [`HyperIo`], which lets a
/// connector multiplex over several concrete stream types (e.g. real TCP
/// plus named pipes) while still exposing a single output type to satisfy
/// [`Connect<S>`](crate::Connect).
pub trait HyperIo: Read + Write + Send + Connection + Unpin + 'static {}

impl<T: Read + Write + Send + Connection + Unpin + 'static> HyperIo for T {}

impl Connection for Box<dyn HyperIo> {
    fn connected(&self) -> Connected {
        (**self).connected()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::testing::PanickingStream;

    fn create() -> Box<dyn HyperIo> {
        Box::new(PanickingStream)
    }

    #[should_panic(expected = "connected")]
    #[test]
    fn connected_delegates_to_inner() {
        let _ = create().connected();
    }
}
