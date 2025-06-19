// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::mem;
use std::pin::Pin;
use std::task::{self, Poll};

use bytes::BufMut;
use pin_project::pin_project;

use crate::WriteStream;

/// Adapts an Oxidizer `WriteStream` to partially implement the Hyper `Write` trait.
///
/// The adapter may be used for any number of consecutive (but not concurrent) writes.
///
/// The `Write` trait is specific to TCP streams, so we only adapt the elementary
/// "write bytes" operation from this trait generically, not the entire trait. You
/// only get a full-trait adapter from an `oxidizer_net::TcpWriteStream`.
#[derive(derive_more::Debug)]
#[pin_project]
pub struct WriteStreamPartialAdapter<'s, S>
where
    S: WriteStream + 's,
{
    // Safety-critical field ordering: this must be above `inner` to ensure that it (and any
    // references it holds) are dropped before `inner` because it may reference contents of `inner`.
    #[debug(ignore)]
    #[pin]
    ongoing_write: Option<Pin<Box<dyn Future<Output = crate::Result<()>> + Send + 's>>>,

    // It is only valid to access this field when `ongoing_write` is `None` because if it is `Some`
    // then it may be holding an exclusive reference to the contents of the box.
    //
    // This is Option to allow the contents to be extracted from the adapter via into_inner().
    //
    // This is Box to separate the borrow of the adapter from the borrow of the inner stream,
    // as without being boxed we would have an aliasing violation when we store the future
    // beyond the lifetime of the `&mut self` borrow.
    inner: Option<Box<S>>,
}

impl<'s, S> WriteStreamPartialAdapter<'s, S>
where
    S: WriteStream + 's,
{
    pub(crate) fn new(inner: S) -> Self {
        Self {
            ongoing_write: None,
            inner: Some(Box::new(inner)),
        }
    }

    /// Returns the original stream that the adapter wraps, consuming the adapter.
    ///
    /// # Panics
    ///
    /// TODO: Document panics
    #[must_use]
    pub fn into_inner(mut self: Pin<Box<Self>>) -> S {
        // Ensures that any exclusive reference to `inner` contents is dropped.
        // It is now safe to create a new reference to `inner` contents.
        self.ongoing_write = None;

        *self
            .inner
            .take()
            .expect("adapter double-consume is impossible")
    }

    /// Implements the logic for a single Hyper `poll_write` call.
    ///
    /// # Panics
    ///
    /// Panics if the buffer size changes between calls that are part of the same logical write.
    pub fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        // If we start a new write, we re-enter the loop to immediately progress it.
        loop {
            let mut this = self.as_mut().project();

            if let Some(write_future) = this.ongoing_write.as_mut().as_pin_mut() {
                // There is an ongoing write, so poll it to make progress.
                return match write_future.poll(cx) {
                    Poll::Ready(Ok(())) => {
                        *this.ongoing_write.as_mut() = None;
                        Poll::Ready(Ok(buf.len()))
                    }
                    Poll::Ready(Err(e)) => {
                        *this.ongoing_write.as_mut() = None;
                        Poll::Ready(Err(e.into()))
                    }
                    Poll::Pending => Poll::Pending,
                };
            }

            // It is OK to create references to contents of `inner` as long as `ongoing_ˇwrite` is
            // `None`. We are guaranteed that `ongoing_write` is `None` because we just checked it.
            let inner_mut = this
                .inner
                .as_mut()
                .expect("can only be None when instance is consumed");

            // No ongoing write, so we start a new one. We allocate new I/O memory for each write.
            // Obviously, this is very inefficient but it is unavoidable due to the way Hyper works
            // with buffers. We need to collaborate with Hyper authors to improve this.
            let mut sequence_builder = inner_mut.reserve(buf.len());
            sequence_builder.put(buf);
            let sequence = sequence_builder.consume_all();

            // Our futures have a lifetime of no more than 's because that is the lifetime bound of
            // the `inner` stream. Here we stamp the lifetime 's onto the reference to ensure that
            // the future is created with that lifetime and cannot exceed it.
            //
            // SAFETY: This extends the reference lifetime beyond this method. We guarantee that we
            // do not access contents of `inner` unless the future is `None`. Likewise, we guarantee
            // that the future is dropped before the contents of `inner` are dropped.
            let inner_mut_s = unsafe { mem::transmute::<&mut S, &'s mut S>(inner_mut) };

            let write_future = inner_mut_s.write(sequence);

            // Note: yes, this is nasty heap allocation here. When we implement proper zero-copy
            // via Hyper, we should also get rid of this heap allocation, as it is anti-performant.
            *this.ongoing_write.as_mut() = Some(Box::pin(write_future));
        }
    }
}

pub trait WriteStreamHyperExt: WriteStream {
    /// Converts the stream into an adapter that can be used to partially implement
    /// the Hyper `Write` trait.
    ///
    /// As the `Write` trait is specific to TCP streams, we only adapt the elementary
    /// "write bytes" operation from this trait generically, not the entire trait.
    fn into_partial_hyper_write<'s>(self) -> WriteStreamPartialAdapter<'s, Self>
    where
        Self: Sized + 's;
}

impl<S> WriteStreamHyperExt for S
where
    S: WriteStream,
{
    fn into_partial_hyper_write<'s>(self) -> WriteStreamPartialAdapter<'s, Self>
    where
        Self: Sized + 's,
    {
        WriteStreamPartialAdapter::new(self)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Buf;

    use super::*;
    use crate::FakeWriteStream;

    #[test]
    fn smoke_test() {
        let write_stream = FakeWriteStream::new();
        let mut partial_hyper_write = Box::pin(write_stream.into_partial_hyper_write());

        let payload = b"Hello, world!";

        let mut cx = task::Context::from_waker(futures::task::noop_waker_ref());
        let result = partial_hyper_write.as_mut().poll_write(&mut cx, payload);

        assert!(matches!(result, Poll::Ready(Ok(13))));

        let original = partial_hyper_write.into_inner();
        let mut contents = original.into_inner().consume_all();
        assert_eq!(contents.len(), 13);
        assert_eq!(contents.get_u8(), 72); // 'H'
        assert_eq!(contents.get_u8(), 101); // 'e'
        assert_eq!(contents.get_u8(), 108); // 'l'
        assert_eq!(contents.get_u8(), 108); // 'l'
        assert_eq!(contents.get_u8(), 111); // 'o'
        assert_eq!(contents.get_u8(), 44); // ','
        assert_eq!(contents.get_u8(), 32); // ' '
        assert_eq!(contents.get_u8(), 119); // 'w'
        assert_eq!(contents.get_u8(), 111); // 'o'
        assert_eq!(contents.get_u8(), 114); // 'r'
        assert_eq!(contents.get_u8(), 108); // 'l'
        assert_eq!(contents.get_u8(), 100); // 'd'
        assert_eq!(contents.get_u8(), 33); // '!'
    }
}