// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::io::{self, IoSlice, Read as _, Write as _};
use std::mem::MaybeUninit;
use std::net::Shutdown;
use std::pin::Pin;
use std::task::{Context, Poll};

use arty_mio::{Registered, RegistrationClosed};
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};
use mio::net::TcpStream;

pub(crate) trait StreamIo: Send + Unpin + 'static {
    /// On success, returns at most `buffer.len()`, initializes that many leading
    /// entries, and does not inspect the buffer's previous contents.
    fn read(&mut self, buffer: &mut [MaybeUninit<u8>]) -> io::Result<usize>;
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize>;
    fn write_vectored(&mut self, buffers: &[IoSlice<'_>]) -> io::Result<usize>;
    fn shutdown(&self) -> io::Result<()>;
    fn poll_read_ready(&self, context: &mut Context<'_>) -> Poll<io::Result<()>>;
    fn poll_write_ready(&self, context: &mut Context<'_>) -> Poll<io::Result<()>>;
}

/// A registered Mio TCP stream that implements the Hyper I/O contract.
///
/// The stream accesses its source directly. After an operation returns
/// [`WouldBlock`](io::ErrorKind::WouldBlock), it polls only the matching
/// readiness direction and retries the operation. Readiness is only a hint.
pub struct MioStream {
    core: StreamCore<RegisteredIo>,
}

struct RegisteredIo {
    registration: Registered<TcpStream>,
}

pub(crate) struct StreamCore<I> {
    io: I,
}

impl MioStream {
    pub(crate) fn new(registration: Registered<TcpStream>) -> Self {
        Self {
            core: StreamCore::new(RegisteredIo { registration }),
        }
    }
}

impl<I> StreamCore<I> {
    pub(crate) fn new(io: I) -> Self {
        Self { io }
    }
}

impl fmt::Debug for MioStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MioStream")
            .field("token", &self.core.io.registration.token())
            .finish_non_exhaustive()
    }
}

impl<I: StreamIo> StreamCore<I> {
    pub(crate) fn poll_read(&mut self, cx: &mut Context<'_>, mut buf: ReadBufCursor<'_>) -> Poll<io::Result<()>> {
        loop {
            if buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }

            // SAFETY: `StreamIo::read` preserves initialized bytes and reports
            // exactly how many leading entries it initialized.
            let result = self.io.read(unsafe { buf.as_mut() });

            match result {
                Ok(length) => {
                    // SAFETY: `StreamIo::read` initialized the first `length`
                    // entries in the cursor's unfilled region.
                    unsafe {
                        buf.advance(length);
                    }
                    return Poll::Ready(Ok(()));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Poll::Ready(Err(error)),
            }

            match self.io.poll_read_ready(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    pub(crate) fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.poll_write_with(cx, |io| io.write(buf))
    }

    fn poll_write_with(&mut self, cx: &mut Context<'_>, mut write: impl FnMut(&mut I) -> io::Result<usize>) -> Poll<io::Result<usize>> {
        loop {
            match write(&mut self.io) {
                Ok(length) => return Poll::Ready(Ok(length)),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Poll::Ready(Err(error)),
            }

            match self.io.poll_write_ready(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    pub(crate) const fn poll_flush() -> Poll<io::Result<()>> {
        // Raw TCP writes go directly to the socket. There is no user-space buffer to flush.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(&self) -> Poll<io::Result<()>> {
        Poll::Ready(self.io.shutdown())
    }

    pub(crate) fn poll_write_vectored(&mut self, cx: &mut Context<'_>, bufs: &[IoSlice<'_>]) -> Poll<io::Result<usize>> {
        self.poll_write_with(cx, |io| io.write_vectored(bufs))
    }
}

impl StreamIo for RegisteredIo {
    fn read(&mut self, buffer: &mut [MaybeUninit<u8>]) -> io::Result<usize> {
        // SAFETY: Mio's `TcpStream` delegates to the operating-system socket
        // read, which only writes to the supplied memory and returns the number
        // of bytes initialized.
        let buffer = unsafe { std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), buffer.len()) };
        self.registration.source_mut().map_err(registration_closed)?.read(buffer)
    }

    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.registration.source_mut().map_err(registration_closed)?.write(buffer)
    }

    fn write_vectored(&mut self, buffers: &[IoSlice<'_>]) -> io::Result<usize> {
        self.registration.source_mut().map_err(registration_closed)?.write_vectored(buffers)
    }

    fn shutdown(&self) -> io::Result<()> {
        self.registration.source().map_err(registration_closed)?.shutdown(Shutdown::Write)
    }

    fn poll_read_ready(&self, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.registration
            .poll_read_ready(context)
            .map(|result| result.map_err(registration_closed))
    }

    fn poll_write_ready(&self, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.registration
            .poll_write_ready(context)
            .map(|result| result.map_err(registration_closed))
    }
}

impl Read for MioStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: ReadBufCursor<'_>) -> Poll<io::Result<()>> {
        self.core.poll_read(cx, buf)
    }
}

impl Write for MioStream {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.core.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        StreamCore::<RegisteredIo>::poll_flush()
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.core.poll_shutdown()
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_write_vectored(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[IoSlice<'_>]) -> Poll<io::Result<usize>> {
        self.core.poll_write_vectored(cx, bufs)
    }
}

impl Connection for MioStream {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

pub(crate) fn registration_closed(error: RegistrationClosed) -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, error)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Wake, Waker};

    use super::*;

    #[test]
    fn spurious_readiness_retries_io_before_waiting_again() {
        let io = ScriptedIo::new(
            vec![ReadStep::WouldBlock, ReadStep::WouldBlock, ReadStep::Data(b"ok".to_vec())],
            vec![],
        );
        io.readable.signal();
        let mut core = StreamCore::new(io.clone());
        let mut bytes = [0; 2];
        let mut buffer = hyper::rt::ReadBuf::new(&mut bytes);

        assert!(poll_read(&mut core, Waker::noop(), buffer.unfilled()).is_pending());
        io.readable.signal();
        assert!(poll_read(&mut core, Waker::noop(), buffer.unfilled()).is_ready());
        assert_eq!(buffer.filled(), b"ok");
        assert_eq!(io.read_calls(), 3);
    }

    #[test]
    fn read_and_write_waiters_replace_only_their_latest_waker() {
        let io = ScriptedIo::new(vec![], vec![]);
        let mut core = StreamCore::new(io.clone());
        let first_read = Arc::new(WakeCounter::default());
        let second_read = Arc::new(WakeCounter::default());
        let first_write = Arc::new(WakeCounter::default());
        let second_write = Arc::new(WakeCounter::default());
        let mut bytes = [0; 1];
        let mut buffer = hyper::rt::ReadBuf::new(&mut bytes);

        assert!(poll_read(&mut core, &Waker::from(Arc::clone(&first_read)), buffer.unfilled()).is_pending());
        assert!(poll_read(&mut core, &Waker::from(Arc::clone(&second_read)), buffer.unfilled()).is_pending());
        assert!(poll_write(&mut core, &Waker::from(Arc::clone(&first_write)), b"x").is_pending());
        assert!(poll_write(&mut core, &Waker::from(Arc::clone(&second_write)), b"x").is_pending());

        io.readable.signal();
        assert_eq!((first_read.count(), second_read.count()), (0, 1));
        assert_eq!((first_write.count(), second_write.count()), (0, 0));

        io.writable.signal();
        assert_eq!((first_write.count(), second_write.count()), (0, 1));
    }

    #[test]
    fn eof_does_not_advance_the_hyper_buffer() {
        let mut core = StreamCore::new(ScriptedIo::new(vec![ReadStep::Eof], vec![]));
        let mut bytes = [0; 4];
        let mut buffer = hyper::rt::ReadBuf::new(&mut bytes);

        assert!(poll_read(&mut core, Waker::noop(), buffer.unfilled()).is_ready());
        assert_eq!(buffer.filled(), b"");
    }

    #[test]
    fn read_initializes_the_full_hyper_buffer_without_an_intermediate_cap() {
        const LENGTH: usize = 256 * 1_024;

        let expected = (0_u8..=u8::MAX).cycle().take(LENGTH).collect::<Vec<_>>();
        let mut core = StreamCore::new(ScriptedIo::new(vec![ReadStep::Data(expected.clone())], vec![]));
        let mut bytes = vec![MaybeUninit::uninit(); LENGTH];
        let mut buffer = hyper::rt::ReadBuf::uninit(&mut bytes);

        assert!(poll_read(&mut core, Waker::noop(), buffer.unfilled()).is_ready());
        assert_eq!(buffer.filled(), expected);
        assert_eq!(core.io.read_calls(), 1);
    }

    #[test]
    fn full_hyper_buffer_skips_io_and_readiness() {
        let io = ScriptedIo::new(vec![ReadStep::Data(b"unused".to_vec())], vec![]);
        let mut core = StreamCore::new(io.clone());
        let mut bytes = [];
        let mut buffer = hyper::rt::ReadBuf::new(&mut bytes);

        assert!(poll_read(&mut core, Waker::noop(), buffer.unfilled()).is_ready());
        assert_eq!(io.read_calls(), 0);
    }

    #[test]
    fn partial_and_zero_writes_are_returned_verbatim() {
        let mut core = StreamCore::new(ScriptedIo::new(vec![], vec![WriteStep::Count(2), WriteStep::Count(0)]));

        assert!(matches!(poll_write(&mut core, Waker::noop(), b"four"), Poll::Ready(Ok(2))));
        assert!(matches!(
            poll_write_vectored(&mut core, Waker::noop(), &[IoSlice::new(b"one"), IoSlice::new(b"two")]),
            Poll::Ready(Ok(0))
        ));
    }

    #[test]
    fn raw_tcp_flush_is_an_immediate_noop() {
        assert!(matches!(StreamCore::<ScriptedIo>::poll_flush(), Poll::Ready(Ok(()))));
    }

    #[test]
    fn dropping_pending_stream_cleans_up_io_and_waiter() {
        let io = ScriptedIo::new(vec![], vec![]);
        let drops = Arc::clone(&io.drops);
        let waiter = Arc::new(WakeCounter::default());
        let mut core = StreamCore::new(io);
        let mut bytes = [0; 1];
        let mut buffer = hyper::rt::ReadBuf::new(&mut bytes);

        assert!(poll_read(&mut core, &Waker::from(Arc::clone(&waiter)), buffer.unfilled()).is_pending());
        assert_eq!(Arc::strong_count(&waiter), 2);
        drop(core);

        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(Arc::strong_count(&waiter), 1);
    }

    fn poll_read<I: StreamIo>(core: &mut StreamCore<I>, waker: &Waker, buffer: ReadBufCursor<'_>) -> Poll<io::Result<()>> {
        core.poll_read(&mut Context::from_waker(waker), buffer)
    }

    fn poll_write<I: StreamIo>(core: &mut StreamCore<I>, waker: &Waker, buffer: &[u8]) -> Poll<io::Result<usize>> {
        core.poll_write(&mut Context::from_waker(waker), buffer)
    }

    fn poll_write_vectored<I: StreamIo>(core: &mut StreamCore<I>, waker: &Waker, buffers: &[IoSlice<'_>]) -> Poll<io::Result<usize>> {
        core.poll_write_vectored(&mut Context::from_waker(waker), buffers)
    }

    #[derive(Clone)]
    struct ScriptedIo {
        reads: Arc<Mutex<VecDeque<ReadStep>>>,
        writes: Arc<Mutex<VecDeque<WriteStep>>>,
        read_calls: Arc<AtomicUsize>,
        readable: Signal,
        writable: Signal,
        drops: Arc<AtomicUsize>,
    }

    impl ScriptedIo {
        fn new(reads: Vec<ReadStep>, writes: Vec<WriteStep>) -> Self {
            Self {
                reads: Arc::new(Mutex::new(reads.into())),
                writes: Arc::new(Mutex::new(writes.into())),
                read_calls: Arc::new(AtomicUsize::new(0)),
                readable: Signal::default(),
                writable: Signal::default(),
                drops: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn read_calls(&self) -> usize {
            self.read_calls.load(Ordering::SeqCst)
        }
    }

    impl Drop for ScriptedIo {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl StreamIo for ScriptedIo {
        fn read(&mut self, buffer: &mut [MaybeUninit<u8>]) -> io::Result<usize> {
            self.read_calls.fetch_add(1, Ordering::SeqCst);
            let step = self.reads.lock().unwrap().pop_front().unwrap_or(ReadStep::WouldBlock);
            match step {
                ReadStep::Data(data) => {
                    let length = data.len().min(buffer.len());
                    for (destination, source) in buffer.iter_mut().zip(data) {
                        destination.write(source);
                    }
                    Ok(length)
                }
                ReadStep::Eof => Ok(0),
                ReadStep::WouldBlock => Err(io::ErrorKind::WouldBlock.into()),
            }
        }

        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            self.next_write()
        }

        fn write_vectored(&mut self, _buffers: &[IoSlice<'_>]) -> io::Result<usize> {
            self.next_write()
        }

        fn shutdown(&self) -> io::Result<()> {
            Ok(())
        }

        fn poll_read_ready(&self, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.readable.poll_ready(context)
        }

        fn poll_write_ready(&self, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.writable.poll_ready(context)
        }
    }

    impl ScriptedIo {
        fn next_write(&self) -> io::Result<usize> {
            let step = self.writes.lock().unwrap().pop_front().unwrap_or(WriteStep::WouldBlock);
            match step {
                WriteStep::Count(length) => Ok(length),
                WriteStep::WouldBlock => Err(io::ErrorKind::WouldBlock.into()),
            }
        }
    }

    enum ReadStep {
        Data(Vec<u8>),
        Eof,
        WouldBlock,
    }

    enum WriteStep {
        Count(usize),
        WouldBlock,
    }

    #[derive(Clone, Default)]
    struct Signal {
        state: Arc<Mutex<SignalState>>,
    }

    #[derive(Default)]
    struct SignalState {
        ready: bool,
        waker: Option<Waker>,
    }

    impl Signal {
        fn signal(&self) {
            let waker = {
                let mut state = self.state.lock().unwrap();
                state.ready = true;
                state.waker.take()
            };
            if let Some(waker) = waker {
                waker.wake();
            }
        }

        fn poll_ready(&self, context: &Context<'_>) -> Poll<io::Result<()>> {
            let mut state = self.state.lock().unwrap();
            if std::mem::take(&mut state.ready) {
                return Poll::Ready(Ok(()));
            }
            if state.waker.as_ref().is_none_or(|waker| !waker.will_wake(context.waker())) {
                state.waker = Some(context.waker().clone());
            }
            Poll::Pending
        }
    }

    #[derive(Default)]
    struct WakeCounter {
        count: AtomicUsize,
    }

    impl WakeCounter {
        fn count(&self) -> usize {
            self.count.load(Ordering::SeqCst)
        }
    }

    impl Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }
}
