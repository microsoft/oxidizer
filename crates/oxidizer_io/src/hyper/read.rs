// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{self, Poll};

use hyper::rt::Read;
use pin_project::pin_project;

use crate::ReadStream;
use crate::mem::SequenceBuilder;

/// Trait to avoid having to name the future used by the `Adapter` struct below. See
/// [`ReadStreamHyperExt::into_hyper_read`] for more details.
pub trait ReadStreamAdapter<T>: Read + std::fmt::Debug + 'static {
    /// Returns the original stream that the adapter wraps, consuming the adapter.
    /// Returns `None` if the adapter is in the middle of a read operation.
    fn try_into_inner(self: Pin<Box<Self>>) -> Option<T>;
}

#[pin_project(project = AdapterProj)]
#[derive(derive_more::Debug)]
struct Adapter<S, F> {
    #[pin]
    state: AdapterState<S, F>,
    #[debug(ignore)]
    start_read: fn(S, usize) -> F,
}

fn start_read<S>(
    mut s: S,
    len: usize,
) -> impl Future<Output = (S, crate::Result<(usize, SequenceBuilder)>)> + Send
where
    S: ReadStream,
{
    let sequence_builder = s.reserve(len);
    async move {
        let result = s.read_at_most_into(len, sequence_builder).await;
        (s, result)
    }
}

#[pin_project(project = AdapterStateProj, project_replace = AdapterStateProjReplace)]
#[derive(derive_more::Debug)]
enum AdapterState<S, F> {
    Future(
        #[pin]
        #[debug(ignore)]
        F,
    ),
    Stream(S),
    Invalid,
}

impl<S, F> Read for Adapter<S, F>
where
    S: ReadStream,
    F: Future<Output = (S, crate::Result<(usize, SequenceBuilder)>)> + Send,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        mut buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        loop {
            if let AdapterStateProj::Future(f) = self.as_mut().project().state.project() {
                match f.poll(cx) {
                    Poll::Ready((stream, Ok((bytes_read, mut sequence_builder)))) => {
                        self.project().state.set(AdapterState::Stream(stream));

                        assert!(
                            bytes_read <= buf.remaining(),
                            "read from stream is larger than buffer - did caller change the buffer size between polls?"
                        );

                        assert_eq!(
                            bytes_read,
                            sequence_builder.len(),
                            "this logic assumes we do not reuse sequence builders - if this changes, we may need to adjust logic to match"
                        );

                        let mut data = sequence_builder.consume_all();

                        // Obviously, this copy is horribly inefficient to have in our I/O stack.
                        // We should work with Hyper authors to eliminate the need for this by
                        // enabling Hyper to use buffers owned by the I/O subsystem.
                        data.consume_all_chunks(|chunk| {
                            buf.put_slice(chunk);
                        });

                        return Poll::Ready(Ok(()));
                    }
                    Poll::Ready((stream, Err(e))) => {
                        self.project().state.set(AdapterState::Stream(stream));
                        return Poll::Ready(Err(e.into()));
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }

            let stream = match self
                .as_mut()
                .project()
                .state
                .project_replace(AdapterState::Invalid)
            {
                AdapterStateProjReplace::Future(_) => {
                    // The match above returns in all cases in which AdapterState::Future is encountered.
                    unreachable!("AdapterState::Future encountered despite previously checked")
                }
                AdapterStateProjReplace::Stream(stream) => stream,
                AdapterStateProjReplace::Invalid => {
                    // We should get here only if we panicked before we set the state two lines below.
                    // The only way for that to happen is if start_read panics, in which case this adapter
                    // is beyond repair anyway.
                    panic!("Reading from ReadStreamAdapter in invalid state")
                }
            };

            let AdapterProj {
                mut state,
                start_read,
            } = self.as_mut().project();
            state.set(AdapterState::Future(start_read(stream, buf.remaining())));
        }
    }
}

impl<S, F> ReadStreamAdapter<S> for Adapter<S, F>
where
    S: ReadStream + 'static,
    F: Future<Output = (S, crate::Result<(usize, SequenceBuilder)>)> + Send + 'static,
{
    fn try_into_inner(mut self: Pin<Box<Self>>) -> Option<S> {
        match self
            .as_mut()
            .project()
            .state
            .project_replace(AdapterState::Invalid)
        {
            AdapterStateProjReplace::Stream(stream) => Some(stream),
            _ => None,
        }
    }
}

pub trait ReadStreamHyperExt: ReadStream {
    /// Converts the stream into an implementation of the Hyper `Read` trait. Since the implementation
    /// depends on unnameable futures in the returned type, this method returns an impl trait.
    /// Once <https://github.com/rust-lang/rust/issues/63063> is resolved, we'll be able to name the type
    /// and some of this will get a bit simpler.
    fn into_hyper_read(self) -> impl ReadStreamAdapter<Self>
    where
        Self: Sized;
}

impl<S> ReadStreamHyperExt for S
where
    S: ReadStream + 'static,
{
    fn into_hyper_read(self) -> impl ReadStreamAdapter<Self>
    where
        Self: Sized,
    {
        Adapter {
            state: AdapterState::Stream(self),
            start_read: |a, b| start_read::<S>(a, b),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;

    use super::*;
    use crate::FakeReadStream;
    use crate::mem::FakeMemoryProvider;
    use crate::testing::async_test;

    #[test]
    fn smoke_test() {
        async_test! {
            let test_data = FakeMemoryProvider::copy_from_static(b"Hello, world!");
            let read_stream = FakeReadStream::new(test_data);

            let mut hyper_read = pin!(read_stream.into_hyper_read());

            // First read - 5 bytes.
            let mut buf = [0; 5];
            let mut read_buf = hyper::rt::ReadBuf::new(&mut buf[..]);

            let result = hyper_read.as_mut().poll_read(
                &mut task::Context::from_waker(futures::task::noop_waker_ref()),
                read_buf.unfilled(),
            );

            assert!(matches!(result, Poll::Ready(Ok(()))));

            assert_eq!(read_buf.filled().len(), 5);
            assert_eq!(read_buf.filled(), b"Hello");

            // Second read - 8 bytes (out of max 64).
            let mut buf = [0; 64];
            let mut read_buf = hyper::rt::ReadBuf::new(&mut buf[..]);

            let result = hyper_read.as_mut().poll_read(
                &mut task::Context::from_waker(futures::task::noop_waker_ref()),
                read_buf.unfilled(),
            );

            assert!(matches!(result, Poll::Ready(Ok(()))));

            assert_eq!(read_buf.filled().len(), 8);
            assert_eq!(read_buf.filled(), b", world!");

            // Third read - 0 bytes (end of stream).
            let mut buf = [0; 64];
            let mut read_buf = hyper::rt::ReadBuf::new(&mut buf[..]);

            let result = hyper_read.as_mut().poll_read(
                &mut task::Context::from_waker(futures::task::noop_waker_ref()),
                read_buf.unfilled(),
            );

            assert!(matches!(result, Poll::Ready(Ok(()))));

            assert_eq!(read_buf.filled().len(), 0);
        }
    }

    #[test]
    fn boxed_into_inner() {
        let test_data = FakeMemoryProvider::copy_from_static(b"Hello, world!");
        let read_stream = FakeReadStream::new(test_data);

        // Note: this only works if boxed. Stack pinning via pin! does not allow value ownership.
        let hyper_read = Box::pin(read_stream.into_hyper_read());

        let _original: FakeReadStream = hyper_read
            .try_into_inner()
            .expect("Should not be in the middle of read");
    }
}