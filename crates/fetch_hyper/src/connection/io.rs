// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`HyperIo`] trait alias plus a [`Connection`] impl for
//! [`Pin<Box<dyn HyperIo>>`], so a type-erased stream can be used wherever a
//! [`HyperIo`] is required. The [`Read`]/[`Write`] impls for `Pin<P>` come
//! from `hyper` itself.

use std::pin::Pin;

use hyper::rt::{Read, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};

/// A trait alias for any I/O stream usable by hyper.
///
/// A [`Pin<Box<dyn HyperIo>>`] itself implements [`HyperIo`], which lets a
/// connector multiplex over several concrete stream types (e.g. real TCP
/// plus named pipes) while still exposing a single output type to satisfy
/// [`Connect<S>`](crate::Connect). Pinning the box (rather than using a plain
/// [`Box<dyn HyperIo>`]) means the erased stream is not required to be
/// [`Unpin`], because the hyper [`Read`]/[`Write`] impls for [`Pin<P>`] do not
/// impose that bound.
pub trait HyperIo: Read + Write + Send + Connection + 'static {}

impl<T: Read + Write + Send + Connection + 'static> HyperIo for T {}

impl Connection for Pin<Box<dyn HyperIo>> {
    fn connected(&self) -> Connected {
        (**self).connected()
    }
}

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

    fn create() -> Pin<Box<dyn HyperIo>> {
        Box::pin(PanickingStream)
    }

    #[should_panic(expected = "connected")]
    #[test]
    fn connected_delegates_to_inner() {
        let _ = create().connected();
    }
}
