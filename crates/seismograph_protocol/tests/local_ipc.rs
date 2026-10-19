// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native local transport, deadlines, cancellation, and descriptor identity.

#![cfg(feature = "test-util")]
#![cfg(not(miri))] // Native and careful suites exercise real kernel handles.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::io::{self, Read as _, Write as _};
use std::thread;
use std::time::{Duration, Instant};

use seismograph_protocol::message::{RecordingConfiguration, Request, Response};
use seismograph_protocol::monitor::MonitorDescriptor;
use seismograph_protocol::transport::{Connection, Listener};
use seismograph_protocol::{read_request, read_response, write_request, write_response};

#[test]
fn local_handshake_is_secret_free_and_connections_are_independent() {
    let listener = seismograph_protocol::testing::listener();
    let descriptor = listener.descriptor().clone();
    let worker_descriptor = descriptor.clone();
    let worker = thread::spawn(move || {
        for request_id in [1, 2] {
            let mut stream = listener.accept().unwrap();
            assert_eq!(read_request(&mut stream).unwrap(), (request_id, Request::Hello));
            write_response(
                &mut stream,
                request_id,
                &Response::Hello {
                    instance_id: worker_descriptor.instance_id,
                    recording: RecordingConfiguration::default(),
                },
            )
            .unwrap();
        }
    });
    for request_id in [1, 2] {
        let mut stream = Connection::connect(
            &seismograph_protocol::io::MonitorIo::new_native(),
            &descriptor,
            Duration::from_secs(1),
        )
        .unwrap();
        let mut hello = Vec::new();
        write_request(&mut hello, request_id, &Request::Hello).unwrap();
        assert_eq!(hello.len(), 20);
        stream.write_all(&hello).unwrap();
        assert_eq!(
            read_response(&mut stream).unwrap(),
            (
                request_id,
                Response::Hello {
                    instance_id: descriptor.instance_id,
                    recording: RecordingConfiguration::default(),
                }
            )
        );
    }
    worker.join().unwrap();
}

#[test]
fn cloned_connection_can_cancel_a_blocked_read() {
    let (_peer, mut server) = seismograph_protocol::testing::connected_pair();
    let cancellation = server.try_clone().unwrap();
    server.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let (completed, completion) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        completed.send(server.read(&mut [0])).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while !seismograph_protocol::testing::read_is_pending(&cancellation).unwrap() {
        assert!(Instant::now() < deadline, "native read did not become pending");
        thread::yield_now();
    }
    cancellation.close().unwrap();
    assert!(matches!(completion.recv_timeout(Duration::from_secs(1)).unwrap(), Ok(0) | Err(_)));
    worker.join().unwrap();
}

#[test]
fn client_close_notifies_the_peer_while_client_clones_are_alive() {
    let (client, mut server) = seismograph_protocol::testing::connected_pair();
    let mut clone = client.try_clone().unwrap();
    server.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    client.close().unwrap();
    assert_eq!(server.read(&mut [0]).unwrap(), 0);
    assert!(matches!(
        clone.write(&[1]).unwrap_err().kind(),
        io::ErrorKind::ConnectionAborted | io::ErrorKind::BrokenPipe
    ));
}

#[test]
fn closing_a_connection_repeatedly_is_idempotent() {
    let (client, _server) = seismograph_protocol::testing::connected_pair();
    client.close().unwrap();
    client.close().unwrap();
}

#[test]
fn peer_disconnect_does_not_prevent_configuring_read_deadlines() {
    let (client, mut server) = seismograph_protocol::testing::connected_pair();
    drop(client);
    server.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    assert_eq!(server.peek(&mut [0]).unwrap(), 0);
    assert_eq!(server.read(&mut [0]).unwrap(), 0);
}

#[test]
fn read_and_write_deadlines_remain_bounded() {
    let (_peer, mut server) = seismograph_protocol::testing::connected_pair();
    server.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
    let start = Instant::now();
    let error = server.read(&mut [0]).unwrap_err();
    assert!(matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock));
    assert!(start.elapsed() < Duration::from_secs(1));

    server.set_write_timeout(Some(Duration::from_millis(20))).unwrap();
    let bytes = vec![0; 1024 * 1024];
    let start = Instant::now();
    let error = server.write_all(&bytes).unwrap_err();
    assert!(matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock));
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn descriptor_claims_cannot_change_the_endpoint_or_owner() {
    let directory = seismograph_protocol::testing::Directory::new();
    let descriptor = directory.descriptor();
    let path = directory.path().join(descriptor.file_name());
    descriptor
        .write_file(&seismograph_protocol::io::MonitorIo::new_native(), &path)
        .unwrap();
    assert_eq!(
        MonitorDescriptor::read_file(&seismograph_protocol::io::MonitorIo::new_native(), &path).unwrap(),
        descriptor
    );
    let mut different = descriptor;
    different.process_id += 1;
    assert_eq!(
        Listener::bind(&seismograph_protocol::io::MonitorIo::new_native(), &different)
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        Connection::connect(
            &seismograph_protocol::io::MonitorIo::new_native(),
            &different,
            Duration::from_secs(1)
        )
        .unwrap_err()
        .kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[test]
fn a_client_that_closes_immediately_does_not_kill_the_listener() {
    let listener = seismograph_protocol::testing::listener();
    let descriptor = listener.descriptor().clone();
    drop(
        Connection::connect(
            &seismograph_protocol::io::MonitorIo::new_native(),
            &descriptor,
            Duration::from_secs(1),
        )
        .unwrap(),
    );
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    match listener.accept() {
        Ok(connection) => drop(connection),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            // Windows recycles a pipe whose client closed before accept.
        }
        Err(error) => panic!("listener lost an abandoned connection: {error}"),
    }
    let mut client = Connection::connect(
        &seismograph_protocol::io::MonitorIo::new_native(),
        &descriptor,
        Duration::from_secs(1),
    )
    .unwrap();
    loop {
        match listener.accept() {
            Ok(mut server) => {
                client.write_all(&[42]).unwrap();
                let mut byte = [0];
                server.read_exact(&mut byte).unwrap();
                assert_eq!(byte, [42]);
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => thread::yield_now(),
            Err(error) => panic!("listener did not recover: {error}"),
        }
    }
}
