// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// A loopback TCP server that resets every connection it accepts.
///
/// The fixture completes the TCP handshake and then discards the connection
/// with a zero linger interval, which turns the close into an RST rather than a
/// FIN. A caller therefore observes an established connection failing mid-flight
/// instead of a refused one, and observes it as soon as the reset arrives: the
/// fixture consults no timer and keeps nothing open.
#[derive(Debug)]
pub struct ResetServer {
    address: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl ResetServer {
    /// Starts the fixture on an arbitrary free port of the IPv4 loopback.
    pub fn start() -> Self {
        let listener = StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown, shutdown_rx) = oneshot::channel();

        let thread = thread::Builder::new()
            .name("fetch-winhttp-reset-server".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                runtime.block_on(run(listener, shutdown_rx));
            })
            .unwrap();

        Self {
            address,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    /// The absolute URL that reaches `path` on this fixture.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}:{}{path}", self.address.ip(), self.address.port())
    }

    fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

impl Drop for ResetServer {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run(listener: StdTcpListener, mut shutdown: oneshot::Receiver<()>) {
    let listener = TcpListener::from_std(listener).unwrap();

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let Ok((stream, _peer)) = accepted else {
                    continue;
                };

                // A zero linger interval makes the close send an RST instead of
                // a FIN, so the peer sees a reset rather than a graceful
                // end-of-stream. Setting it can only fail if the socket is
                // already gone, in which case the peer already has its error.
                //
                // The deprecation warns that `SO_LINGER` blocks the closing
                // thread. That applies to a positive interval, where the close
                // waits for queued data to flush; a zero interval discards the
                // queue and resets at once, so it cannot block.
                #[expect(deprecated, reason = "a zero linger interval is how a reset is forced, and cannot block the close")]
                let _ = stream.set_linger(Some(Duration::ZERO));
                drop(stream);
            }
        }
    }
}
