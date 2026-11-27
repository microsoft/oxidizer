// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Contract tests for Kubernetes TCP readiness.

#![cfg(feature = "tcp")]

use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::time::Duration;

use health::{Aggregator, Attribute, Component, Health, Monitor, Publisher};
use health_probe::kubernetes_tcp::serve;
use observed::Sink;
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_DELAY: Duration = Duration::from_millis(10);

fn unused_address() -> SocketAddr {
    let listener = StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    address
}

fn fixture() -> (Component, Monitor, Publisher) {
    let aggregator = Aggregator::new(Sink::noop());
    let component = aggregator.component("database");
    let monitor = aggregator.monitor();
    let publisher = component.publisher_with(Health::Unhealthy, [("phase", "starting")]);
    (component, monitor, publisher)
}

async fn publish_and_wait(publisher: &mut Publisher, monitor: &mut Monitor, state: Health) {
    publisher.publish(state, Vec::<Attribute>::new());
    while monitor.state() != state {
        monitor.changed().await;
    }
}

async fn wait_for_connection(address: SocketAddr, expected: bool) {
    timeout(TEST_TIMEOUT, async {
        loop {
            let connected = TcpStream::connect(address).await.is_ok();
            if connected == expected {
                return;
            }
            sleep(RETRY_DELAY).await;
        }
    })
    .await
    .unwrap();
}

async fn assert_server_closes_connection(address: SocketAddr) {
    let stream = TcpStream::connect(address).await.unwrap();
    timeout(TEST_TIMEOUT, stream.readable()).await.unwrap().unwrap();
    assert_eq!(stream.try_read(&mut [0_u8; 1]).unwrap(), 0);
}

#[tokio::test(flavor = "current_thread")]
#[cfg_attr(miri, ignore = "native and careful suites cover operating-system TCP sockets")]
async fn tcp_probe_follows_health_and_releases_port() {
    let address = unused_address();
    let (_component, mut monitor, mut publisher) = fixture();
    let server = tokio::spawn(serve(monitor.clone(), address));

    tokio::task::yield_now().await;
    wait_for_connection(address, false).await;

    publish_and_wait(&mut publisher, &mut monitor, Health::Healthy).await;
    wait_for_connection(address, true).await;
    assert_server_closes_connection(address).await;
    wait_for_connection(address, true).await;

    publish_and_wait(&mut publisher, &mut monitor, Health::Degraded).await;
    wait_for_connection(address, false).await;

    publish_and_wait(&mut publisher, &mut monitor, Health::Unhealthy).await;
    wait_for_connection(address, false).await;

    publish_and_wait(&mut publisher, &mut monitor, Health::Healthy).await;
    wait_for_connection(address, true).await;

    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    wait_for_connection(address, false).await;
    drop(StdTcpListener::bind(address).unwrap());
}

#[tokio::test(flavor = "current_thread")]
#[cfg_attr(miri, ignore = "native and careful suites cover operating-system TCP sockets")]
async fn tcp_probe_reports_bind_failure() {
    let listener = StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let (_component, mut monitor, mut publisher) = fixture();
    publish_and_wait(&mut publisher, &mut monitor, Health::Healthy).await;

    let error = serve(monitor, address).await.unwrap_err();

    assert_eq!(error.kind(), ErrorKind::AddrInUse);
}
