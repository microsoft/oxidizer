// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public native monitor handshake, capture, and shutdown behavior.

#![cfg(all(feature = "test-util", not(miri)))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::io::{self, Read as _, Write as _};
use std::time::{Duration, Instant};

use seismograph::monitor::Monitor;
#[cfg(unix)]
use seismograph_protocol::discovery::Directory;
use seismograph_protocol::message::{Request, Response, SnapshotOptions};
#[cfg(unix)]
use seismograph_protocol::monitor::MonitorDescriptor;
use seismograph_protocol::monitor::MonitorOwner;
use seismograph_protocol::transport::Connection;
use seismograph_protocol::{read_response, write_request};

#[test]
fn ordinary_native_startup_initializes_and_discovers_its_own_registry() {
    let io = seismograph_protocol::io::MonitorIo::new_native();
    let monitor = Monitor::builder().io(io.clone()).name("zero-setup-native").start().unwrap();
    let descriptor = monitor.descriptor().clone();
    let mut own_directory = None;
    for directory in seismograph_protocol::discovery::directories(&io).unwrap() {
        let directory = match directory {
            Ok(directory) => directory,
            Err(error) => {
                eprintln!("ignoring unrelated registry: {error}");
                continue;
            }
        };
        let found = match directory.descriptors() {
            Ok(mut entries) => entries.any(|entry| entry.as_ref().is_ok_and(|entry| *entry == descriptor)),
            Err(error) => {
                eprintln!("ignoring unavailable registry {}: {error}", directory.path().display());
                continue;
            }
        };
        if found {
            own_directory = Some(directory);
            break;
        }
    }
    let directory = own_directory.unwrap();
    let descriptor_path = directory.path().join(descriptor.file_name());
    let mut connection = Connection::connect(&io, &descriptor, Duration::from_secs(1)).unwrap();
    connection.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write_request(&mut connection, 1, &Request::Hello).unwrap();
    assert!(matches!(read_response(&mut connection).unwrap(), (1, Response::Hello { .. })));
    drop(directory);
    let path = descriptor.endpoint.path().to_owned();
    #[cfg(unix)]
    let temporary_parent = path
        .parent()
        .filter(|parent| {
            parent
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("seismograph-"))
        })
        .map(std::path::Path::to_owned);
    drop(monitor);
    assert!(!descriptor_path.exists());
    #[cfg(unix)]
    {
        assert!(!path.exists());
        if let Some(temporary) = temporary_parent {
            assert!(!temporary.exists());
        }
    }
    #[cfg(windows)]
    assert!(!path.as_os_str().is_empty());
}

#[test]
fn monitor_publishes_a_native_endpoint_and_returns_unchanged_snapshot_data() {
    let monitor = Monitor::builder().isolated().name("worker").instance("one").start().unwrap();
    let descriptor = monitor.descriptor();
    assert_eq!(descriptor.owner, MonitorOwner::current().unwrap());
    let path = descriptor.endpoint.path();
    #[cfg(unix)]
    let directory = path.parent().unwrap().to_owned();
    #[cfg(windows)]
    assert!(path.to_str().unwrap().starts_with(r"\\.\pipe\seismograph-"));
    #[cfg(unix)]
    {
        let entries = Directory::open(&seismograph_protocol::io::MonitorIo::new_native(), &directory, &descriptor.owner).unwrap();
        let descriptors = entries.descriptors().unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(descriptors.as_slice(), std::slice::from_ref(descriptor));
        assert_eq!(
            MonitorDescriptor::read_file(
                &seismograph_protocol::io::MonitorIo::new_native(),
                directory.join(descriptor.file_name())
            )
            .unwrap(),
            *descriptor
        );
    }
    let mut connection = Connection::connect(
        &seismograph_protocol::io::MonitorIo::new_native(),
        descriptor,
        Duration::from_secs(1),
    )
    .unwrap();
    connection.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write_request(&mut connection, 1, &Request::Hello).unwrap();
    assert!(
        matches!(read_response(&mut connection).unwrap(), (1, Response::Hello { instance_id, .. }) if instance_id == descriptor.instance_id)
    );
    write_request(&mut connection, 2, &Request::CaptureSnapshot(SnapshotOptions::default())).unwrap();
    let (request_id, response) = read_response(&mut connection).unwrap();
    assert_eq!(request_id, 2);
    let Response::Snapshot(bytes) = response else {
        panic!("capture must return a snapshot")
    };
    seismograph::snapshot::decode(&bytes).unwrap();
    #[cfg(unix)]
    let endpoint = descriptor.endpoint.clone();
    drop(monitor);
    assert_closed(&mut connection);
    #[cfg(unix)]
    {
        assert!(!endpoint.path().exists());
        assert!(!directory.exists());
    }
}

#[test]
fn dropping_a_monitor_cancels_a_client_stalled_mid_handshake() {
    let monitor = Monitor::builder().isolated().name("worker").start().unwrap();
    let mut connection = Connection::connect(
        &seismograph_protocol::io::MonitorIo::new_native(),
        monitor.descriptor(),
        Duration::from_secs(1),
    )
    .unwrap();
    connection.write_all(b"SGMP").unwrap();
    let start = Instant::now();
    drop(monitor);
    assert!(start.elapsed() < Duration::from_secs(1));
    connection.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    assert_closed(&mut connection);
}

fn assert_closed(connection: &mut Connection) {
    match connection.read(&mut [0]) {
        Ok(count) => assert_eq!(count, 0),
        Err(error) => assert!(matches!(
            error.kind(),
            io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted | io::ErrorKind::NotConnected
        )),
    }
}
