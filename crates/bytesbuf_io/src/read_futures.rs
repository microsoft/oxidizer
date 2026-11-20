// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt::{self, Debug};
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::task::Poll::{Pending, Ready};
use std::{mem, task};

use bytesbuf::{BytesBuf, BytesView};

use crate::Read;

/// Adapts a [`Read`] implementation to the `futures::Stream` API.
///
/// Each item in the `futures::Stream` is a [`BytesView`] containing some bytes read
/// from the underlying [`Read`].
///
/// # Security
///
/// **This adapter is insecure if the side producing the bytes is not trusted**. An attacker
/// may trickle data byte-by-byte, consuming a large amount of resources.
///
/// Robust code working with untrusted sources should take precautions such as only processing
/// read data when either a time or length threshold is reached and reusing buffers that
/// have remaining capacity, appending additional data to existing buffers using
/// [`read_more_into()`][crate::Read::read_more_into] instead of reserving new buffers
/// for each read operation.
pub struct ReadAsFuturesStream<S>
where
    S: Read + Debug,
{
    // References `inner`. Must be defined above `inner` to ensure it gets dropped first.`
    #[expect(clippy::type_complexity, reason = "never needs to be named, good enough")]
    active_read: Option<Pin<Box<dyn Future<Output = Result<BytesBuf, S::Error>> + Send>>>,

    // Safety invariant: we can only touch this field if `active_read` is `None`.
    // Separate storage keeps stream reborrows from invalidating the pending read.
    inner: Box<S>,

    // This struct must remain pinned because `active_read` references `inner`.
    _require_pin: PhantomPinned,
}

impl<S> ReadAsFuturesStream<S>
where
    S: Read + Debug,
{
    pub(crate) fn new(inner: S) -> Pin<Box<Self>> {
        Box::pin(Self {
            active_read: None,
            inner: Box::new(inner),
            _require_pin: PhantomPinned,
        })
    }

    /// Abandons any ongoing read operation and returns the source.
    #[must_use]
    pub fn into_inner(self: Pin<Box<Self>>) -> S {
        // SAFETY: We are going to unpin `self` by first dropping `active_read`, which is the thing
        // that references `inner` and requires pinning. Once `active_read` has been dropped,
        // no more pinning requirements exist.
        let mut unpinned = unsafe { Pin::into_inner_unchecked(self) };

        unpinned.active_read = None;
        *unpinned.inner
    }
}

impl<S> futures_core::Stream for ReadAsFuturesStream<S>
where
    S: Read + Debug,
{
    type Item = Result<BytesView, S::Error>;

    fn poll_next<'a>(self: Pin<&'a mut Self>, cx: &'a mut task::Context) -> task::Poll<Option<Self::Item>> {
        // SAFETY: We are not moving `inner`, which is the field that must remain pinned.
        let this = unsafe { self.get_unchecked_mut() };

        let mut active_read = if let Some(active_read) = this.active_read.take() {
            active_read
        } else {
            let inner = &mut *this.inner;
            let future = async move { inner.read_any().await };
            let boxed_future = Box::pin(future);

            // SAFETY: We overwrite the lifetime of the future to 'static because in reality we
            // have a lifetime bounded to the lifetime of the struct itself but this cannot be
            // meaningfully expressed in Rust, so we have to expand it to 'static. For safety, we
            // have to ensure that the future does not outlive either the struct instance itself
            // or `inner`, and that we do not touch `inner` while the future exists.
            unsafe {
                mem::transmute::<
                    Pin<Box<dyn Future<Output = Result<BytesBuf, S::Error>> + Send + 'a>>,
                    Pin<Box<dyn Future<Output = Result<BytesBuf, S::Error>> + Send>>,
                >(boxed_future)
            }
        };

        let result = active_read.as_mut().poll(cx);

        match result {
            Ready(Ok(mut buf)) => {
                let data = buf.consume_all();

                if data.is_empty() {
                    // We have reached the end of the stream.
                    return Ready(None);
                }

                Ready(Some(Ok(data)))
            }
            Ready(Err(e)) => Ready(Some(Err(e))),
            Pending => {
                this.active_read = Some(active_read);
                Pending
            }
        }
    }
}

impl<S> Debug for ReadAsFuturesStream<S>
where
    S: Read + Debug,
{
    #[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct(type_name::<Self>());
        if self.active_read.is_none() {
            debug.field("inner", &self.inner);
        }
        debug
            .field("active_read.is_some()", &self.active_read.is_some())
            .finish_non_exhaustive()
    }
}
