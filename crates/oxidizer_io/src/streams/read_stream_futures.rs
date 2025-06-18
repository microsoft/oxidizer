// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt::{self, Debug};
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::task::Poll::{Pending, Ready};
use std::{mem, task};

use crate::ReadStream;
use crate::mem::{Sequence, SequenceBuilder};

/// Adapts a `ReadStream` to the `futures::Stream` API.
///
/// # Security
///
/// This adapter is not secure if the the other side of the stream is not trusted. An attacker
/// may trickle data byte-by-byte, consuming a large amount of I/O resources.
///
/// Robust code working with untrusted streams should take precautions such as only processing
/// read data when either a time or length threshold is reached and reusing byte sequences that
/// have remaining capacity, meanwhile appending to existing memory using
/// [`read_more_into()`][crate::ReadStream::read_more_into] instead of reserving new memory
/// for each read operation.
pub struct ReadStreamAsFuturesStream<S>
where
    S: ReadStream + Debug,
{
    // References `inner`. Must be defined above `inner` to ensure it gets dropped first.`
    active_read: Option<Pin<Box<dyn Future<Output = ReadAnyResult>>>>,

    // Safety invariant: we can only touch this field if `active_read` is `None`.
    inner: S,

    // This struct must remain pinned because `active_read` references `inner`.
    _require_pin: PhantomPinned,
}

type ReadAnyResult = crate::Result<SequenceBuilder>;

impl<S> ReadStreamAsFuturesStream<S>
where
    S: ReadStream + Debug,
{
    pub(crate) fn new(inner: S) -> Pin<Box<Self>> {
        Box::pin(Self {
            active_read: None,
            inner,
            _require_pin: PhantomPinned,
        })
    }

    /// Abandons ongoing read operations (if any) and returns the inner type
    /// implementing [`ReadStream`].
    #[must_use]
    pub fn into_inner(self: Pin<Box<Self>>) -> S {
        // SAFETY: We are going to unpin `self` by first dropping `active_read`, which is the thing
        // that references `inner` and requires pinning. Once `active_read` has been dropped,
        // no more pinning requirements exist.
        let mut unpinned = unsafe { Pin::into_inner_unchecked(self) };

        unpinned.active_read = None;
        unpinned.inner
    }
}

impl<S> futures::Stream for ReadStreamAsFuturesStream<S>
where
    S: ReadStream + Debug,
{
    type Item = crate::Result<Sequence>;

    fn poll_next<'a>(
        self: Pin<&'a mut Self>,
        cx: &'a mut task::Context,
    ) -> task::Poll<Option<Self::Item>> {
        // SAFETY: We are not moving `inner`, which is the field that must remain pinned.
        let this = unsafe { self.get_unchecked_mut() };

        let mut active_read = if let Some(active_read) = this.active_read.take() {
            active_read
        } else {
            let inner = &mut this.inner;
            let future = async move { inner.read_any().await };
            let boxed_future = Box::pin(future);

            // SAFETY: We overwrite the lifetime of the future to 'static because in reality we
            // have a lifetime bounded to the lifetime of the struct itself but this cannot be
            // meaningfully expressed in Rust, so we have to expand it to 'static. For safety, we
            // have to ensure that the future does not outlive either the struct instance itself
            // or `inner`, and that we do not touch `inner` while the future exists.
            unsafe {
                mem::transmute::<
                    Pin<Box<dyn Future<Output = ReadAnyResult> + 'a>>,
                    Pin<Box<dyn Future<Output = ReadAnyResult>>>,
                >(boxed_future)
            }
        };

        let result = active_read.as_mut().poll(cx);

        match result {
            Ready(Ok(mut sequence_builder)) => {
                let sequence = sequence_builder.consume_all();

                if sequence.is_empty() {
                    // We have reached the end of the stream.
                    return Ready(None);
                }

                Ready(Some(Ok(sequence)))
            }
            Ready(Err(e)) => Ready(Some(Err(e))),
            Pending => {
                this.active_read = Some(active_read);
                Pending
            }
        }
    }
}

impl<S> Debug for ReadStreamAsFuturesStream<S>
where
    S: ReadStream + Debug,
{
    #[cfg_attr(test, mutants::skip)] // We have no contract to test.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .field("active_read.is_some()", &self.active_read.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;

    use bytes::Buf;
    use futures::task::noop_waker_ref;
    use futures::{Stream, StreamExt};

    use super::*;
    use crate::mem::FakeMemoryProvider;
    use crate::testing::async_test;
    use crate::{FakeReadStream, PendingStream, ReadStreamExt};

    #[test]
    fn smoke_test() {
        async_test! {
            let inner = FakeReadStream::with_max_read_size(
                FakeMemoryProvider::copy_from_static(b"Hello, w"),
                2
            );

            let mut futures_stream = inner.into_futures_stream();

            // It can be read from.
            let mut payload1 = futures_stream.next().await.unwrap().unwrap();
            assert_eq!(payload1.len(), 2);
            assert_eq!(payload1.get_u8(), b'H');
            assert_eq!(payload1.get_u8(), b'e');

            let mut payload2 = futures_stream.next().await.unwrap().unwrap();
            assert_eq!(payload2.len(), 2);
            assert_eq!(payload2.get_u8(), b'l');
            assert_eq!(payload2.get_u8(), b'l');

            // We can get back the original.
            let mut original = futures_stream.into_inner();

            let mut payload3 = original.read_exactly(2).await.unwrap();
            assert_eq!(payload3.len(), 2);
            assert_eq!(payload3.get_u8(), b'o');
            assert_eq!(payload3.get_u8(), b',');

            // Back to the futures::Stream!
            let mut futures_stream = original.into_futures_stream();

            let mut payload4 = futures_stream.next().await.unwrap().unwrap();
            assert_eq!(payload4.len(), 2);
            assert_eq!(payload4.get_u8(), b' ');
            assert_eq!(payload4.get_u8(), b'w');

            // And once we hit end of stream, it needs to return `None`.
            assert!(futures_stream.next().await.is_none());
        }
    }

    #[test]
    fn pending_read_cancelled_on_into_inner() {
        let inner = PendingStream;

        let mut futures_stream = inner.into_futures_stream();

        // We can cancel a pending read. Need to test this directly against the impl,
        // as the `futures::Stream` extension methods are lazy and will not start the read.
        // let mut futures_stream = original.into_futures_stream();

        let mut cx = task::Context::from_waker(noop_waker_ref());
        assert!(matches!(
            futures_stream.as_mut().poll_next(&mut cx),
            task::Poll::Pending
        ));

        // The inner stream is not capable of completing reads, so a bit hard to test.
        // Well, as long as there is no panic or Miri complaint, we can be satisfied.
        let mut inner = futures_stream.into_inner();

        let read_future = pin!(inner.read_any());
        assert!(read_future.poll(&mut cx).is_pending());
    }
}