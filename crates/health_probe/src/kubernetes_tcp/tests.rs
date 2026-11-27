// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Unit tests for TCP error and health transition races.

use std::collections::VecDeque;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use futures::channel::oneshot;
use health::{Aggregator, Attribute, Component, Health, Monitor, Publisher};
use observed::Sink;
use tick::{Clock, ClockControl};
use tokio::runtime::Builder;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::{ACCEPT_RETRY_DELAY, HealthMonitor, TcpIo, serve_with, wait_before_retry};

const TEST_TIMEOUT: Duration = Duration::from_secs(1);
const ADDRESS: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8081);

macro_rules! runtime_test {
    ($name:ident, $body:block) => {
        #[test]
        fn $name() {
            Builder::new_current_thread().enable_time().build().unwrap().block_on(async $body);
        }
    };
}

#[derive(Clone)]
struct ScriptedIo {
    inner: Arc<ScriptedIoInner>,
}

struct ScriptedIoInner {
    binds: Mutex<VecDeque<BindStep>>,
    accepts: Mutex<VecDeque<AcceptStep>>,
    bind_calls: AtomicUsize,
    accept_calls: AtomicUsize,
}

enum BindStep {
    Ready,
    WaitReady(oneshot::Receiver<()>),
    Error(io::ErrorKind),
}

enum AcceptStep {
    Error(io::ErrorKind),
    Pending,
}

#[derive(Clone)]
struct ScriptedMonitor {
    healthy: Arc<AtomicBool>,
}

impl ScriptedMonitor {
    fn healthy() -> Self {
        Self {
            healthy: Arc::new(AtomicBool::new(true)),
        }
    }

    fn set(&self, state: Health) {
        self.healthy.store(state == Health::Healthy, Ordering::SeqCst);
    }
}

impl HealthMonitor for ScriptedMonitor {
    fn state(&self) -> Health {
        if self.healthy.load(Ordering::SeqCst) {
            Health::Healthy
        } else {
            Health::Unhealthy
        }
    }

    async fn changed(&mut self) {
        std::future::pending::<()>().await;
    }
}

impl ScriptedIo {
    fn new(binds: impl IntoIterator<Item = BindStep>, accepts: impl IntoIterator<Item = AcceptStep>) -> Self {
        Self {
            inner: Arc::new(ScriptedIoInner {
                binds: Mutex::new(binds.into_iter().collect()),
                accepts: Mutex::new(accepts.into_iter().collect()),
                bind_calls: AtomicUsize::new(0),
                accept_calls: AtomicUsize::new(0),
            }),
        }
    }

    async fn wait_for_bind_calls(&self, count: usize) {
        timeout(TEST_TIMEOUT, async {
            while self.inner.bind_calls.load(Ordering::SeqCst) < count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    async fn wait_for_accept_calls(&self, count: usize) {
        timeout(TEST_TIMEOUT, async {
            while self.inner.accept_calls.load(Ordering::SeqCst) < count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

impl TcpIo for ScriptedIo {
    type Listener = ();
    type Stream = ();

    fn bind(&self, _address: SocketAddr) -> impl Future<Output = io::Result<Self::Listener>> + Send {
        let inner = Arc::clone(&self.inner);
        async move {
            inner.bind_calls.fetch_add(1, Ordering::SeqCst);
            let step = inner.binds.lock().unwrap().pop_front().unwrap();
            match step {
                BindStep::Ready => Ok(()),
                BindStep::WaitReady(release) => {
                    release.await.unwrap();
                    Ok(())
                }
                BindStep::Error(kind) => Err(io::Error::from(kind)),
            }
        }
    }

    fn accept<'a>(&'a self, _listener: &'a Self::Listener) -> impl Future<Output = io::Result<Self::Stream>> + Send + 'a {
        let inner = Arc::clone(&self.inner);
        async move {
            inner.accept_calls.fetch_add(1, Ordering::SeqCst);
            let step = inner.accepts.lock().unwrap().pop_front().unwrap();
            match step {
                AcceptStep::Error(kind) => Err(io::Error::from(kind)),
                AcceptStep::Pending => std::future::pending().await,
            }
        }
    }
}

async fn healthy_fixture() -> (Component, Monitor, Publisher) {
    let aggregator = Aggregator::new(Sink::noop());
    let component = aggregator.component("database");
    let mut monitor = aggregator.monitor();
    let publisher = component.publisher();
    while monitor.state() != Health::Healthy {
        monitor.changed().await;
    }
    (component, monitor, publisher)
}

async fn publish_and_wait(publisher: &mut Publisher, monitor: &mut Monitor, state: Health) {
    publisher.publish(state, Vec::<Attribute>::new());
    while monitor.state() != state {
        monitor.changed().await;
    }
}

fn spawn_server(io: ScriptedIo, monitor: Monitor, clock: Clock) -> JoinHandle<io::Result<()>> {
    tokio::spawn(async move {
        let mut monitor = monitor;
        serve_with(io, &mut monitor, ADDRESS, &clock).await
    })
}

runtime_test!(retries_accept_and_rebind_errors, {
    let io = ScriptedIo::new(
        [BindStep::Ready, BindStep::Error(io::ErrorKind::AddrInUse), BindStep::Ready],
        [AcceptStep::Error(io::ErrorKind::ConnectionAborted), AcceptStep::Pending],
    );
    let (_component, monitor, _publisher) = healthy_fixture().await;
    let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    let server = spawn_server(io.clone(), monitor, clock);

    timeout(TEST_TIMEOUT, async {
        io.wait_for_bind_calls(3).await;
        io.wait_for_accept_calls(2).await;
    })
    .await
    .unwrap();

    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
});

runtime_test!(health_change_cancels_retry_wait, {
    let (_component, mut monitor, mut publisher) = healthy_fixture().await;
    let mut retry_monitor = monitor.clone();
    let clock = Clock::new_frozen();
    let wait = tokio::spawn(async move { wait_before_retry(&mut retry_monitor, &clock).await });

    tokio::task::yield_now().await;
    publish_and_wait(&mut publisher, &mut monitor, Health::Unhealthy).await;
    timeout(TEST_TIMEOUT, wait).await.unwrap().unwrap();
});

runtime_test!(healthy_report_change_keeps_retry_waiting, {
    let (_component, mut monitor, mut publisher) = healthy_fixture().await;
    let mut retry_monitor = monitor.clone();
    let clock = Clock::new_frozen();
    let wait = tokio::spawn(async move { wait_before_retry(&mut retry_monitor, &clock).await });

    tokio::task::yield_now().await;
    publisher.publish(Health::Healthy, [("revision", 2_i64)]);
    monitor.changed().await;
    tokio::task::yield_now().await;
    assert!(!wait.is_finished());

    publish_and_wait(&mut publisher, &mut monitor, Health::Unhealthy).await;
    timeout(TEST_TIMEOUT, wait).await.unwrap().unwrap();
});

runtime_test!(health_change_after_rebind_failure_stops_rebinding, {
    let io = ScriptedIo::new(
        [BindStep::Ready, BindStep::Error(io::ErrorKind::AddrInUse)],
        [AcceptStep::Error(io::ErrorKind::ConnectionAborted)],
    );
    let (_component, mut monitor, mut publisher) = healthy_fixture().await;
    let control = ClockControl::new();
    let clock = control.to_clock();
    let mut server_monitor = monitor.clone();
    let mut server = Box::pin(serve_with(io.clone(), &mut server_monitor, ADDRESS, &clock));

    assert!(matches!(futures::poll!(server.as_mut()), Poll::Pending));
    assert_eq!(io.inner.accept_calls.load(Ordering::SeqCst), 1);
    control.advance(ACCEPT_RETRY_DELAY);
    assert!(matches!(futures::poll!(server.as_mut()), Poll::Pending));
    assert_eq!(io.inner.bind_calls.load(Ordering::SeqCst), 2);
    publish_and_wait(&mut publisher, &mut monitor, Health::Unhealthy).await;
    control.advance(ACCEPT_RETRY_DELAY);
    assert!(matches!(futures::poll!(server.as_mut()), Poll::Pending));
    assert_eq!(io.inner.bind_calls.load(Ordering::SeqCst), 2);
});

runtime_test!(health_change_after_bind_skips_listening, {
    let (release, wait) = oneshot::channel();
    let io = ScriptedIo::new([BindStep::WaitReady(wait), BindStep::Ready], [AcceptStep::Pending]);
    let (_component, mut monitor, mut publisher) = healthy_fixture().await;
    let server = spawn_server(io.clone(), monitor.clone(), Clock::new_frozen());

    io.wait_for_bind_calls(1).await;
    publish_and_wait(&mut publisher, &mut monitor, Health::Unhealthy).await;
    release.send(()).unwrap();
    tokio::task::yield_now().await;
    publish_and_wait(&mut publisher, &mut monitor, Health::Healthy).await;
    timeout(TEST_TIMEOUT, async {
        io.wait_for_bind_calls(2).await;
        io.wait_for_accept_calls(1).await;
    })
    .await
    .unwrap();

    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
});

runtime_test!(post_bind_health_check_skips_accept, {
    let (release, wait) = oneshot::channel();
    let io = ScriptedIo::new([BindStep::WaitReady(wait)], [AcceptStep::Pending]);
    let mut monitor = ScriptedMonitor::healthy();
    let monitor_control = monitor.clone();
    let control = ClockControl::new();
    let clock = control.to_clock();
    let mut server = Box::pin(serve_with(io.clone(), &mut monitor, ADDRESS, &clock));

    assert!(matches!(futures::poll!(server.as_mut()), Poll::Pending));
    monitor_control.set(Health::Unhealthy);
    release.send(()).unwrap();
    assert!(matches!(futures::poll!(server.as_mut()), Poll::Pending));
    assert_eq!(io.inner.accept_calls.load(Ordering::SeqCst), 0);
});

runtime_test!(healthy_report_change_keeps_listener, {
    let io = ScriptedIo::new([BindStep::Ready], [AcceptStep::Pending, AcceptStep::Pending]);
    let (_component, monitor, mut publisher) = healthy_fixture().await;
    let server = spawn_server(io.clone(), monitor, Clock::new_frozen());

    io.wait_for_accept_calls(1).await;
    publisher.publish(Health::Healthy, [("revision", 2_i64)]);
    timeout(TEST_TIMEOUT, io.wait_for_accept_calls(2)).await.unwrap();
    assert_eq!(io.inner.bind_calls.load(Ordering::SeqCst), 1);

    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
});
