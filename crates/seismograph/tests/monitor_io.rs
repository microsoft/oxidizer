// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public monitor behavior using instance-scoped in-memory I/O.

#![cfg(feature = "monitor")]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::io::{self, Read as _, Write as _};
use std::time::{Duration, Instant};

use seismograph::monitor::Monitor;
use seismograph_protocol::discovery::{self, Directory};
use seismograph_protocol::io::{IoOperation, MonitorIo};
use seismograph_protocol::message::{Request, Response, SnapshotOptions};
use seismograph_protocol::transport::Connection;
use seismograph_protocol::{read_response, write_request};

#[test]
fn startup_failures_are_injectable_without_host_registry_setup() {
    for operation in [
        IoOperation::CurrentExe,
        IoOperation::InstanceId,
        IoOperation::CreateDirectory,
        IoOperation::CurrentOwner,
        IoOperation::Bind,
        IoOperation::ConfigureListener,
        IoOperation::CreateDescriptor,
        IoOperation::WriteDescriptor,
        IoOperation::PublishDescriptor,
    ] {
        let (io, control) = MonitorIo::new_mocked();
        control.fail_next(
            operation,
            io::Error::new(io::ErrorKind::PermissionDenied, "injected startup failure"),
        );
        let error = Monitor::builder().io(io.clone()).start().unwrap_err();
        assert!(error.to_string().contains("injected startup failure"), "{operation:?}: {error}");
        let source = std::error::Error::source(&error).unwrap();
        let source = if matches!(operation, IoOperation::Bind | IoOperation::ConfigureListener) {
            source.downcast_ref::<io::Error>().unwrap()
        } else {
            let protocol = source.downcast_ref::<seismograph_protocol::Error>().unwrap();
            std::error::Error::source(protocol).unwrap().downcast_ref::<io::Error>().unwrap()
        };
        assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(Directory::current(&io).unwrap().descriptors().unwrap().count(), 0);
    }
}

#[test]
fn injected_monitor_publishes_captures_and_removes_its_own_descriptor() {
    let (io, control) = MonitorIo::new_mocked();
    control.set_max_chunk(2);
    let monitor = Monitor::builder().io(io.clone()).name("worker").instance("one").start().unwrap();
    let descriptor = monitor.descriptor().clone();
    assert_eq!(descriptor.owner, io.current_owner().unwrap());
    let directory = Directory::current(&io).unwrap();
    let path = directory.path().join(descriptor.file_name());
    let descriptors = directory.descriptors().unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(descriptors.as_slice(), std::slice::from_ref(&descriptor));

    let mut connection = Connection::connect(&io, &descriptor, Duration::from_secs(1)).unwrap();
    write_request(&mut connection, 1, &Request::Hello).unwrap();
    assert!(
        matches!(read_response(&mut connection).unwrap(), (1, Response::Hello { instance_id, .. }) if instance_id == descriptor.instance_id)
    );
    write_request(&mut connection, 2, &Request::CaptureSnapshot(SnapshotOptions::default())).unwrap();
    let (id, Response::Snapshot(bytes)) = read_response(&mut connection).unwrap() else {
        panic!("capture must return a snapshot");
    };
    assert_eq!(id, 2);
    seismograph::snapshot::decode(&bytes).unwrap();
    drop(monitor);
    assert_eq!(control.file_bytes(&path), None);
    assert_eq!(connection.read(&mut [0]).unwrap(), 0);
}

#[test]
fn dropping_an_injected_monitor_cancels_a_partial_frame_without_advancing_time() {
    let (io, control) = MonitorIo::new_mocked();
    let initial = io.instant();
    let monitor = Monitor::builder().io(io.clone()).name("worker").start().unwrap();
    assert!(monitor.client_connections().unwrap().is_empty());
    let mut connection = Connection::connect(&io, monitor.descriptor(), Duration::from_secs(1)).unwrap();
    connection.write_all(b"SGMP").unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while control
        .operations()
        .iter()
        .filter(|operation| **operation == IoOperation::Read)
        .count()
        < 2
    {
        assert!(Instant::now() < deadline, "server did not start the second partial-header read");
        std::thread::yield_now();
    }
    let server = monitor.client_connections().unwrap().pop().unwrap();
    assert_eq!(seismograph_protocol::testing::bytes_read(&server), 4);
    assert_eq!(seismograph_protocol::testing::close_calls(&server), 0);
    drop(monitor);
    assert!(control.operations().contains(&IoOperation::Close));
    assert_eq!(seismograph_protocol::testing::close_calls(&server), 1);
    assert_eq!(connection.read(&mut [0]).unwrap(), 0);
    assert_eq!(io.instant(), initial);
    assert!(control.operations().contains(&IoOperation::RemoveDescriptor));
    drop(server);
}

#[test]
fn monitors_share_only_the_explicitly_selected_io_instance() {
    let (io, _) = MonitorIo::new_mocked();
    let (other, _) = MonitorIo::new_mocked();
    let first = Monitor::builder().io(io.clone()).start().unwrap();
    let second = Monitor::builder().io(io.clone()).start().unwrap();
    assert_ne!(first.descriptor().instance_id, second.descriptor().instance_id);
    assert_eq!(first.descriptor().name, "worker");
    assert_eq!(Directory::current(&other).unwrap().descriptors().unwrap().count(), 0);
    drop(first);
    let descriptors = discovery::directories(&io)
        .unwrap()
        .flat_map(|directory| directory.unwrap().descriptors().unwrap().collect::<Vec<_>>())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(descriptors, [second.descriptor().clone()]);
}
