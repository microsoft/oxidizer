// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`TrackedStream`]: wraps a stream to emit `http.client.connection.closed`
//! on drop and to attach the [`ConnectionInfo`] response extension.

use std::io::IoSlice;
use std::pin::Pin;
use std::task::{Context, Poll};

use fetch_options::ConnectionInfo;
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};
use observed::{Sink, emit};
use templated_uri::BaseUri;

use crate::telemetry::{ConnectionClosed, protocol_version};

/// A wrapper around a stream that tracks connection lifecycle.
#[derive(Debug)]
pub(crate) struct TrackedStream<T> {
    inner: T,
    base_uri: BaseUri,
    info: ConnectionInfo,
    sink: Sink,
    connected: Connected,
}

impl<T> TrackedStream<T> {
    pub(crate) fn new(inner: T, base_uri: BaseUri, info: ConnectionInfo, sink: Sink, connected: Connected) -> Self {
        Self {
            inner,
            base_uri,
            info,
            sink,
            connected,
        }
    }
}

impl<T> Drop for TrackedStream<T> {
    fn drop(&mut self) {
        emit!(
            self.sink,
            ConnectionClosed {
                server_address: self.base_uri.host(),
                server_port: self.base_uri.effective_port(),
                scheme: self.base_uri.scheme().as_str(),
                protocol_version: protocol_version(&self.connected),
                duration_secs: self.info.age().as_secs_f64(),
            }
        );
    }
}

impl<T: Read + Unpin> Read for TrackedStream<T> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: Write + Unpin> Write for TrackedStream<T> {
    // Skip: `Poll::from(Ok(N))` mutations spin hyper's write loop forever (timeout, not failure).
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    // Skip: see `poll_write`.
    fn poll_write_vectored(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[IoSlice<'_>]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }
}

impl<T: Connection> Connection for TrackedStream<T> {
    fn connected(&self) -> Connected {
        self.inner.connected().extra(self.info.clone())
    }
}

impl<T: Unpin> Unpin for TrackedStream<T> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::pin;
    use std::task::Waker;
    use std::time::Duration;

    use fetch_options::PoolIndex;
    use observed::Severity;
    use observed_testing::{ExpectedEvent, TEST_ID, test_emitter};

    use super::*;
    use crate::testing::PanickingStream;

    #[test]
    fn gamma_survivor_tracked_stream_preserves_write_vectored_capability() {
        let (inner, _) = tokio::io::duplex(64);
        let inner = hyper_util::rt::TokioIo::new(inner);
        let capability = hyper::rt::Write::is_write_vectored(&inner);
        let now = tick::Clock::new_frozen().instant();
        let stream = TrackedStream {
            inner,
            base_uri: BaseUri::from_static("https://example.test"),
            info: ConnectionInfo::new(move || now, PoolIndex::new(0), None),
            sink: Sink::noop(),
            connected: Connected::new(),
        };
        assert_eq!(hyper::rt::Write::is_write_vectored(&stream), capability);
    }

    fn frozen_info(pool_index: usize) -> ConnectionInfo {
        ConnectionInfo::new(std::time::Instant::now, PoolIndex::new(pool_index), None)
    }

    fn create() -> TrackedStream<PanickingStream> {
        TrackedStream {
            inner: PanickingStream,
            base_uri: BaseUri::from_static("https://example.com"),
            info: frozen_info(0),
            sink: Sink::noop(),
            connected: Connected::new(),
        }
    }

    #[test]
    fn drop_emits_closed_event_with_inferred_port() {
        let (sink, processor) = test_emitter(TEST_ID);
        let control = tick::ClockControl::new();
        let clock = control.to_clock();
        let stream = TrackedStream::new(
            PanickingStream,
            BaseUri::from_static("https://example.com"),
            ConnectionInfo::new(move || clock.instant(), PoolIndex::new(0), None),
            sink,
            Connected::new().negotiated_h2(),
        );
        control.advance(Duration::from_millis(500));

        drop(stream);

        assert_eq!(
            processor.single_event(),
            ExpectedEvent::new("http.client.connection.closed", Severity::Debug)
                .body("connection closed")
                .dimension("http.client.connection.duration", 0.5)
                .dimension("network.protocol.version", "2")
                .dimension("server.address", "example.com")
                .dimension("server.port", 443_u16)
                .dimension("url.scheme", "https")
                .metric(),
        );
    }

    #[test]
    fn connected_does_not_panic_for_well_behaved_inner() {
        let stream = TrackedStream {
            inner: ConnectedOnlyStream,
            base_uri: BaseUri::from_static("https://example.com"),
            info: frozen_info(7),
            sink: Sink::noop(),
            connected: Connected::new(),
        };

        let _connected = stream.connected();
    }

    /// Minimal stream type whose `connected()` does NOT panic, allowing us
    /// to assert that `TrackedStream` extends the inner metadata rather than
    /// replacing it.
    struct ConnectedOnlyStream;

    impl Read for ConnectedOnlyStream {
        fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
            unreachable!()
        }
    }

    impl Write for ConnectedOnlyStream {
        fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: &[u8]) -> Poll<std::io::Result<usize>> {
            unreachable!()
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            unreachable!()
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            unreachable!()
        }
    }

    impl Connection for ConnectedOnlyStream {
        fn connected(&self) -> Connected {
            Connected::new()
        }
    }

    #[should_panic(expected = "poll_read")]
    #[test]
    fn poll_read_delegates_to_inner() {
        let mut cx = Context::from_waker(Waker::noop());
        let stream = pin!(create());
        let mut buf = hyper::rt::ReadBuf::uninit(&mut []);
        let _ = stream.poll_read(&mut cx, buf.unfilled());
    }

    #[should_panic(expected = "poll_write")]
    #[test]
    fn poll_write_delegates_to_inner() {
        let mut cx = Context::from_waker(Waker::noop());
        let stream = pin!(create());
        let _ = stream.poll_write(&mut cx, b"x");
    }

    #[should_panic(expected = "poll_flush")]
    #[test]
    fn poll_flush_delegates_to_inner() {
        let mut cx = Context::from_waker(Waker::noop());
        let stream = pin!(create());
        let _ = stream.poll_flush(&mut cx);
    }

    #[should_panic(expected = "poll_shutdown")]
    #[test]
    fn poll_shutdown_delegates_to_inner() {
        let mut cx = Context::from_waker(Waker::noop());
        let stream = pin!(create());
        let _ = stream.poll_shutdown(&mut cx);
    }

    #[should_panic(expected = "is_write_vectored")]
    #[test]
    fn is_write_vectored_delegates_to_inner() {
        let stream = create();
        let _ = stream.is_write_vectored();
    }
}
