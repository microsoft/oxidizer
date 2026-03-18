// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use http::Request;
use http_body_util::Empty;
use hyper::body::Incoming;
use hyper::rt::{Read, ReadBufCursor, Write};

/// Creates a hyper [`Incoming`] body containing `body` without any real network IO.
pub async fn create_incoming(body: impl AsRef<[u8]>) -> Incoming {
    let body = body.as_ref();
    let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len(),);
    let mut data = header.into_bytes();
    data.extend_from_slice(body);

    let io = MockIo::new(data);

    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io)
        .await
        .expect("handshake succeeds because MockIo contains a well-formed HTTP response");

    let request = Request::builder()
        .uri("/")
        .body(Empty::<Bytes>::new())
        .expect("building a minimal GET request never fails");

    // Drive the connection and the request concurrently on the same task.
    // MockIo defers reads until a write occurs, so the response is only
    // returned after hyper sends the request.
    let (response, _conn) = futures::future::join(sender.send_request(request), conn).await;

    response
        .expect("response is received because MockIo provides a complete HTTP response")
        .into_body()
}

/// In-memory IO type implementing the [`Read`](hyper::rt::Read) and
/// [`Write`](hyper::rt::Write) traits from [`hyper`].
#[derive(Debug)]
pub struct MockIo {
    read_buf: std::io::Cursor<Vec<u8>>,
    /// Becomes `true` after the first write, unlocking the read side.
    can_read: bool,
    /// Stashed waker so we can wake the reader when the first write arrives.
    read_waker: Option<Waker>,
}

impl MockIo {
    /// Creates a new `MockIo` whose read side returns `data` once a write occurs.
    #[must_use]
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            read_buf: std::io::Cursor::new(data),
            can_read: false,
            read_waker: None,
        }
    }
}

impl Read for MockIo {
    #[expect(clippy::cast_possible_truncation, reason = "test code only")]
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, mut buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();

        // Defer response data until the client has started writing its request,
        // matching real connection behavior where the server only responds
        // after receiving the request.
        if !this.can_read {
            this.read_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        let pos = this.read_buf.position() as usize;
        let data = this.read_buf.get_ref();
        let remaining = &data[pos..];

        if remaining.is_empty() {
            return Poll::Ready(Ok(()));
        }

        let n = std::cmp::min(remaining.len(), buf.remaining());
        buf.put_slice(&remaining[..n]);
        this.read_buf.set_position((pos + n) as u64);

        Poll::Ready(Ok(()))
    }
}

impl Write for MockIo {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if !this.can_read {
            this.can_read = true;
            if let Some(waker) = this.read_waker.take() {
                waker.wake();
            }
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

mod tests {
    use http_body_util::BodyExt;

    use super::*;

    #[tokio::test]
    async fn create_incoming_with_body() {
        let incoming = create_incoming(b"hello world").await;
        let collected = incoming.collect().await.unwrap().to_bytes();
        assert_eq!(collected.as_ref(), b"hello world");
    }

    #[tokio::test]
    async fn create_incoming_empty() {
        let incoming = create_incoming(b"").await;
        let collected = incoming.collect().await.unwrap().to_bytes();
        assert!(collected.is_empty());
    }

    #[tokio::test]
    async fn create_incoming_from_str() {
        let incoming = create_incoming("json payload").await;
        let collected = incoming.collect().await.unwrap().to_bytes();
        assert_eq!(collected.as_ref(), b"json payload");
    }

    #[tokio::test]
    async fn create_incoming_integrates_with_http_body_builder() {
        use crate::HttpBodyBuilder;

        let builder = HttpBodyBuilder::new_fake();
        let incoming = create_incoming(b"integration test").await;
        let body = builder.incoming(incoming);

        let text = body.into_text().await.unwrap();
        assert_eq!(text, "integration test");
    }
}
