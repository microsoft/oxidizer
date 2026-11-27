// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Kubernetes TCP readiness over a dedicated socket.
//!
//! The server listens only while the aggregate state is [`Health::Healthy`].
//! It closes each accepted connection without reading or writing data. Use a
//! dedicated readiness address so health changes do not affect application traffic.
//!
//! The caller must use a continuously driven Tokio runtime with networking enabled.
//! The caller must also drive the supplied clock continuously. Health changes are
//! asynchronous, so the socket can briefly show the previous state during a transition.
//! Cancel the future during shutdown to release the address.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use health::{Health, Monitor};
use tick::Clock;
use tokio::net::{TcpListener, TcpStream};

/// Delay before retrying an accept failure.
///
/// The delay prevents resource-exhaustion errors from causing a tight loop. A short
/// delay keeps the readiness outage visible while allowing the process to recover.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

trait TcpIo {
    type Listener: Sync;
    type Stream: Send;

    fn bind(&self, address: SocketAddr) -> impl Future<Output = io::Result<Self::Listener>> + Send;
    fn accept<'a>(&'a self, listener: &'a Self::Listener) -> impl Future<Output = io::Result<Self::Stream>> + Send + 'a;
}

struct TokioTcp;

impl TcpIo for TokioTcp {
    type Listener = TcpListener;
    type Stream = TcpStream;

    async fn bind(&self, address: SocketAddr) -> io::Result<Self::Listener> {
        TcpListener::bind(address).await
    }

    async fn accept(&self, listener: &Self::Listener) -> io::Result<Self::Stream> {
        listener.accept().await.map(|(stream, _)| stream)
    }
}

/// Serves TCP readiness for `monitor` on `address`.
///
/// The function binds `address` only while the aggregate state is [`Health::Healthy`].
/// It closes accepted connections and continues to listen. When the state becomes
/// degraded or unhealthy, it drops the listener until the process recovers. It also
/// drops and reopens the listener after an accept failure.
///
/// Cancel this future to stop the server and release the address.
///
/// # Examples
///
/// ```no_run
/// # #[cfg(feature = "tcp")]
/// # async fn example(monitor: health::Monitor, clock: tick::Clock) -> std::io::Result<()> {
/// use health_probe::kubernetes_tcp::serve;
///
/// let address = "0.0.0.0:8081".parse().unwrap();
/// serve(monitor, address, clock).await
/// # }
/// ```
///
/// # Errors
///
/// Returns an error when the server cannot bind `address` before it has listened
/// successfully. Later bind failures are retried while the process remains healthy.
pub async fn serve(mut monitor: Monitor, address: SocketAddr, clock: Clock) -> io::Result<()> {
    serve_with(TokioTcp, &mut monitor, address, &clock).await
}

async fn serve_with<I: TcpIo + Send + Sync>(io: I, monitor: &mut Monitor, address: SocketAddr, clock: &Clock) -> io::Result<()> {
    let mut bound_once = false;

    'server: loop {
        while monitor.state() != Health::Healthy {
            monitor.changed().await;
        }

        let listener = loop {
            match io.bind(address).await {
                Ok(listener) => {
                    bound_once = true;
                    break listener;
                }
                Err(error) if !bound_once => return Err(error),
                Err(_) => {
                    wait_before_retry(monitor, clock).await;
                    if monitor.state() != Health::Healthy {
                        continue 'server;
                    }
                }
            }
        };
        if monitor.state() != Health::Healthy {
            continue;
        }

        let retry = loop {
            tokio::select! {
                biased;
                () = monitor.changed() => {
                    if monitor.state() != Health::Healthy {
                        break false;
                    }
                }
                accepted = io.accept(&listener) => {
                    match accepted {
                        Ok(stream) => drop(stream),
                        Err(_) => break true,
                    }
                }
            }
        };
        drop(listener);

        if retry {
            wait_before_retry(monitor, clock).await;
        }
    }
}

async fn wait_before_retry(monitor: &mut Monitor, clock: &Clock) {
    let delay = clock.delay(ACCEPT_RETRY_DELAY);
    tokio::pin!(delay);

    loop {
        tokio::select! {
            biased;
            () = monitor.changed() => {
                if monitor.state() != Health::Healthy {
                    return;
                }
            }
            () = &mut delay => return,
        }
    }
}

#[cfg(test)]
mod tests;
