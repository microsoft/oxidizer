// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Kubernetes TCP readiness over a dedicated socket.
//!
//! The server listens only while the aggregate state is [`Health::Healthy`].
//! It closes each accepted connection without reading or writing data. Use a
//! dedicated readiness address so health changes do not affect application traffic.
//!
//! The caller must run the returned future on a Tokio runtime with networking enabled.
//! Cancel the future during application shutdown to release the readiness address.

use std::io;
use std::net::SocketAddr;

use health::{Health, Monitor};
use tokio::net::TcpListener;

/// Serves TCP readiness for `monitor` on `address`.
///
/// The function binds `address` only while the aggregate state is [`Health::Healthy`].
/// It closes accepted connections and continues to listen. When the state becomes
/// degraded or unhealthy, it drops the listener until the process recovers.
///
/// Cancel this future to stop the server and release the address.
///
/// # Examples
///
/// ```no_run
/// # #[cfg(feature = "tcp")]
/// # async fn example(monitor: health::Monitor) -> std::io::Result<()> {
/// use health_probe::kubernetes_tcp::serve;
///
/// let address = "0.0.0.0:8081".parse().unwrap();
/// serve(monitor, address).await
/// # }
/// ```
///
/// # Errors
///
/// Returns an error when the server cannot bind `address` or accept a connection.
pub async fn serve(mut monitor: Monitor, address: SocketAddr) -> io::Result<()> {
    loop {
        while monitor.state() != Health::Healthy {
            monitor.changed().await;
        }

        let listener = TcpListener::bind(address).await?;
        if monitor.state() != Health::Healthy {
            continue;
        }

        loop {
            tokio::select! {
                biased;
                () = monitor.changed() => {
                    if monitor.state() != Health::Healthy {
                        break;
                    }
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    drop(stream);
                }
            }
        }
    }
}
