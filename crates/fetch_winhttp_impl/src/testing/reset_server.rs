// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::thread::JoinHandle;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::oneshot;

use super::startup::{Startup, StartupListener, bind_listener, launch_worker};

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
        let startup = Self::start_with(
            |address| StdTcpListener::bind(address).unwrap(),
            run,
            |name, work| launch_worker(name, tokio::runtime::Builder::new_current_thread, work),
        );
        Self {
            address: startup.address,
            shutdown: startup.shutdown,
            thread: startup.thread,
        }
    }

    fn start_with<L: StartupListener, W, F: Future<Output = ()>>(
        bind_tcp: impl FnOnce(SocketAddr) -> L,
        serve: impl FnOnce(L, oneshot::Receiver<()>) -> F,
        launch: impl FnOnce(String, F) -> W,
    ) -> Startup<(), W> {
        let (listener, address) = bind_listener(IpAddr::V4(Ipv4Addr::LOCALHOST), bind_tcp);
        let (shutdown, shutdown_rx) = oneshot::channel();

        let work = serve(listener, shutdown_rx);
        let thread = launch("fetch-winhttp-reset-server".to_owned(), work);

        Startup {
            address,
            state: (),
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

// #[gamma::skip(fn_value.unit, reason = "this adapter accepts native TCP sockets and sets SO_LINGER to send a reset; startup and shutdown ownership are tested without opening a listener")]
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

#[cfg(test)]
mod gamma_survivor_tests {
    use std::cell::{Cell, RefCell};
    use std::thread;

    use super::super::startup::gamma_round2::{GammaListener, GammaRuntime};
    use super::super::startup::run_worker;
    use super::*;

    #[test]
    fn gamma_round2_reset_startup_retains_named_worker_and_shutdown() {
        let address: SocketAddr = "127.0.0.1:4123".parse().unwrap();
        let shutdown_receiver = RefCell::new(None);
        let served = Cell::new(0);
        let mut startup = ResetServer::start_with(
            |bind| {
                assert_eq!(bind, "127.0.0.1:0".parse().unwrap());
                GammaListener {
                    address,
                    nonblocking: Cell::new(None),
                }
            },
            |listener, receiver| {
                assert_eq!(listener.address, address);
                assert_eq!(listener.nonblocking.get(), Some(true));
                *shutdown_receiver.borrow_mut() = Some(receiver);
                async {
                    served.set(served.get() + 1);
                }
            },
            |name, work| {
                assert_eq!(name, "fetch-winhttp-reset-server");
                run_worker(&GammaRuntime, work);
                61
            },
        );
        assert_eq!(served.get(), 1);
        assert_eq!(startup.address, address);
        assert_eq!(startup.thread.take(), Some(61));
        startup.shutdown.take().unwrap().send(()).unwrap();
        assert_eq!(shutdown_receiver.borrow_mut().as_mut().unwrap().try_recv(), Ok(()));
    }

    #[test]
    fn gamma_round2_reset_stop_propagates_a_worker_panic() {
        let mut server = ResetServer {
            address: SocketAddr::from(([127, 0, 0, 1], 8123)),
            shutdown: None,
            thread: Some(thread::spawn(|| panic!("gamma_round2_worker"))),
        };
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| server.stop())).is_err());
        assert!(server.thread.is_none());
    }

    #[test]
    fn gamma_survivor_stopped_reset_url_and_shutdown_sender_are_exact() {
        let (sender, mut receiver) = oneshot::channel();
        let mut server = ResetServer {
            address: SocketAddr::from(([127, 0, 0, 1], 8123)),
            shutdown: Some(sender),
            thread: None,
        };
        assert_eq!(server.url("/reset?q=1"), "http://127.0.0.1:8123/reset?q=1");
        server.stop();
        assert!(server.shutdown.is_none());
        assert_eq!(receiver.try_recv(), Ok(()));
        server.stop();
        let (sender, mut receiver) = oneshot::channel();
        drop(ResetServer {
            address: SocketAddr::from(([127, 0, 0, 1], 8123)),
            shutdown: Some(sender),
            thread: None,
        });
        assert_eq!(receiver.try_recv(), Ok(()));
    }
}
