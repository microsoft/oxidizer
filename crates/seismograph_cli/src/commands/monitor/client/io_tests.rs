// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Write as _;
use std::path::Path;
use std::thread::{self, JoinHandle};

use seismograph_protocol::discovery::Directory;
use seismograph_protocol::io::IoOperation;
use seismograph_protocol::monitor::MonitorEndpoint;
use seismograph_protocol::transport::Listener;
use seismograph_protocol::write_response;

use super::*;

fn descriptor(io: &MonitorIo) -> (Directory, MonitorDescriptor) {
    let directory = Directory::current(io).unwrap();
    let owner = io.current_owner().unwrap();
    let process_id = io.process_id();
    let instance_id = io.instance_id().unwrap();
    let descriptor = MonitorDescriptor {
        name: "worker west/europe".into(),
        instance: None,
        endpoint: MonitorEndpoint::for_instance(directory.path(), &owner, process_id, instance_id),
        owner,
        process_id,
        instance_id,
    };
    (directory, descriptor)
}

fn hello(descriptor: &MonitorDescriptor) -> Response {
    Response::Hello {
        instance_id: descriptor.instance_id,
        recording: RecordingConfiguration::default(),
    }
}

fn accept(listener: &Listener) -> Connection {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok(stream) => return stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "client did not connect for the scripted conversation");
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("scripted accept failed: {error}"),
        }
    }
}

fn read_request(stream: &Connection) -> Result<(u64, Request), seismograph_protocol::Error> {
    let mut reader = stream.try_clone().unwrap();
    let (completed, completion) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || completed.send(seismograph_protocol::read_request(&mut reader)).unwrap());
    let result = completion.recv_timeout(Duration::from_secs(5));
    if result.is_err() {
        stream.close().unwrap();
    }
    worker.join().unwrap();
    result.unwrap()
}

fn serve(io: &MonitorIo, descriptor: &MonitorDescriptor, conversations: Vec<Vec<(Request, Response)>>) -> JoinHandle<()> {
    let listener = Listener::bind(io, descriptor).unwrap();
    listener.set_nonblocking(true).unwrap();
    thread::spawn(move || {
        for conversation in conversations {
            let mut stream = accept(&listener);
            for (request, response) in conversation {
                let (id, actual) = read_request(&stream).unwrap();
                assert_eq!(actual, request);
                write_response(&mut stream, id, &response).unwrap();
            }
        }
    })
}

fn serve_discoverable(io: &MonitorIo, descriptor: &MonitorDescriptor) -> JoinHandle<()> {
    serve(
        io,
        descriptor,
        vec![
            vec![
                (Request::Hello, hello(descriptor)),
                (Request::ReadCacheRecording, Response::CacheRecording(RecordingPolicy::default())),
            ],
            vec![
                (Request::Hello, hello(descriptor)),
                (Request::ReadServerVersion, Response::ServerVersion("healthy".into())),
            ],
        ],
    )
}

#[test]
fn complete_client_commands_use_the_selected_io_instance() {
    let (io, control) = MonitorIo::new_mocked();
    control.set_max_chunk(3);
    let (_, descriptor) = descriptor(&io);
    let worker = serve(
        &io,
        &descriptor,
        vec![
            vec![
                (Request::Hello, hello(&descriptor)),
                (Request::ReadCacheRecording, Response::CacheRecording(RecordingPolicy::default())),
            ],
            vec![
                (Request::Hello, hello(&descriptor)),
                (
                    Request::CaptureSnapshot(SnapshotOptions::default()),
                    Response::Snapshot(vec![1, 2, 3]),
                ),
            ],
            vec![
                (Request::Hello, hello(&descriptor)),
                (Request::ClearEventBuffers, Response::Acknowledged),
            ],
            vec![
                (Request::Hello, hello(&descriptor)),
                (Request::CaptureSnapshotAndStop, Response::Snapshot(vec![4, 5, 6])),
            ],
        ],
    );
    assert_eq!(handshake(&io, &descriptor).unwrap(), RecordingConfiguration::default());
    assert_eq!(capture_snapshot(&io, &descriptor, SnapshotOptions::default()).unwrap(), [1, 2, 3]);
    clear_event_buffers(&io, &descriptor).unwrap();
    assert_eq!(capture_snapshot_and_stop(&io, &descriptor).unwrap(), [4, 5, 6]);
    worker.join().unwrap();
    assert_eq!(
        control
            .operations()
            .iter()
            .filter(|operation| **operation == IoOperation::Connect)
            .count(),
        4
    );
}

#[test]
fn connection_setup_and_frame_failures_do_not_fall_back_to_native_io() {
    for operation in [IoOperation::Connect, IoOperation::ReadTimeout, IoOperation::WriteTimeout] {
        let (io, control) = MonitorIo::new_mocked();
        let (_, descriptor) = descriptor(&io);
        let _listener = Listener::bind(&io, &descriptor).unwrap();
        control.fail_next(
            operation,
            io::Error::new(io::ErrorKind::PermissionDenied, "injected client failure"),
        );
        let error = connect(&io, &descriptor).err().unwrap();
        assert!(error.to_string().contains("injected client failure"), "{operation:?}: {error}");
    }
    for operation in [IoOperation::Write, IoOperation::Read, IoOperation::Flush] {
        let (io, control) = MonitorIo::new_mocked();
        let (_peer, connection) = control.connected_pair();
        let mut connection = ClientConnection::new(&io, connection).unwrap();
        control.fail_next(
            operation,
            io::Error::new(io::ErrorKind::PermissionDenied, "injected client failure"),
        );
        let error = if operation == IoOperation::Read {
            read_control_response(&mut connection).unwrap_err()
        } else {
            write_request(&mut connection, 1, &Request::Hello).unwrap_err()
        };
        assert!(error.to_string().contains("injected client failure"), "{operation:?}: {error}");
    }
}

#[test]
fn discovery_reports_invalid_entries_and_ignores_only_stale_endpoints() {
    let (io, control) = MonitorIo::new_mocked();
    let (directory, live) = descriptor(&io);
    io.publish_descriptor(&directory, &live).unwrap();
    let (_, stale) = descriptor(&io);
    io.publish_descriptor(&directory, &stale).unwrap();
    control.put_file(&directory.path().join("invalid.monitor"), live.owner.clone(), b"bad".to_vec());
    control.put_file(
        &directory.path().parent().unwrap().join("invalid-owner"),
        live.owner.clone(),
        Vec::new(),
    );
    let worker = serve(
        &io,
        &live,
        vec![
            vec![
                (Request::Hello, hello(&live)),
                (Request::ReadCacheRecording, Response::CacheRecording(RecordingPolicy::default())),
            ],
            vec![
                (Request::Hello, hello(&live)),
                (Request::ReadServerVersion, Response::ServerVersion("test-version".into())),
            ],
        ],
    );
    let discovery = discover(&io).unwrap();
    assert_eq!(discovery.instances.len(), 1);
    assert_eq!(discovery.instances[0].descriptor, live);
    assert_eq!(discovery.instances[0].server_version.as_deref(), Some("test-version"));
    assert_eq!(discovery.warnings.len(), 2);
    worker.join().unwrap();

    control.fail_next(IoOperation::DiscoverDirectories, io::Error::other("registry unavailable"));
    assert!(discover(&io).err().unwrap().to_string().contains("registry unavailable"));
}

#[test]
fn discovery_suppresses_only_missing_or_refused_connections() {
    for (kind, warnings) in [
        (io::ErrorKind::NotFound, 0),
        (io::ErrorKind::ConnectionRefused, 0),
        (io::ErrorKind::PermissionDenied, 1),
        (io::ErrorKind::TimedOut, 1),
    ] {
        let (io, control) = MonitorIo::new_mocked();
        let (directory, descriptor) = descriptor(&io);
        io.publish_descriptor(&directory, &descriptor).unwrap();
        control.fail_next(IoOperation::Connect, io::Error::new(kind, "probe failed"));
        let discovery = discover(&io).unwrap();
        assert!(discovery.instances.is_empty());
        assert_eq!(discovery.warnings.len(), warnings, "{kind:?}");
        if let Some(warning) = discovery.warnings.first() {
            assert!(warning.contains("probe failed"));
        }
    }
}

#[test]
fn oversized_probe_responses_do_not_hide_healthy_monitors() {
    for probe in [Request::Hello, Request::ReadCacheRecording, Request::ReadServerVersion] {
        let (io, _) = MonitorIo::new_mocked();
        let (directory, malicious) = descriptor(&io);
        io.publish_descriptor(&directory, &malicious).unwrap();
        let (_, healthy) = descriptor(&io);
        io.publish_descriptor(&directory, &healthy).unwrap();
        let healthy_worker = serve_discoverable(&io, &healthy);
        let listener = Listener::bind(&io, &malicious).unwrap();
        listener.set_nonblocking(true).unwrap();
        let malicious_worker = thread::spawn(move || {
            let mut stream = accept(&listener);
            assert_eq!(read_request(&stream).unwrap(), (1, Request::Hello));
            let request_id = if probe == Request::Hello {
                1
            } else {
                write_response(&mut stream, 1, &hello(&malicious)).unwrap();
                assert_eq!(read_request(&stream).unwrap(), (2, Request::ReadCacheRecording));
                if probe == Request::ReadServerVersion {
                    write_response(&mut stream, 2, &Response::CacheRecording(RecordingPolicy::default())).unwrap();
                    drop(stream);
                    stream = accept(&listener);
                    assert_eq!(read_request(&stream).unwrap(), (1, Request::Hello));
                    write_response(&mut stream, 1, &hello(&malicious)).unwrap();
                    assert_eq!(read_request(&stream).unwrap(), (2, Request::ReadServerVersion));
                }
                2
            };
            let mut header = Vec::new();
            write_response(&mut header, request_id, &Response::Snapshot(Vec::new())).unwrap();
            header[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
            stream.write_all(&header).unwrap();
        });
        let discovery = discover(&io).unwrap();
        assert_eq!(discovery.instances.len(), 1);
        assert_eq!(discovery.instances[0].descriptor, healthy);
        assert_eq!(discovery.warnings.len(), 1);
        assert!(discovery.warnings[0].contains("exceeds its size limit"));
        malicious_worker.join().unwrap();
        healthy_worker.join().unwrap();
    }
}

#[test]
fn partial_progress_does_not_restart_a_probe_deadline() {
    let (io, control) = MonitorIo::new_mocked();
    let (mut peer, connection) = control.connected_pair();
    let mut connection = ClientConnection::new(&io, connection).unwrap();
    peer.write_all(b"abcdef").unwrap();
    control.advance_time(Duration::from_secs(9));
    let mut bytes = [0; 2];
    assert_eq!(connection.read(&mut bytes).unwrap(), 2);
    assert_eq!(&bytes, b"ab");
    control.advance_time(Duration::from_secs(1));
    assert_eq!(connection.read(&mut bytes).unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert_eq!(connection.peek(&mut bytes).unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert_eq!(connection.write(b"request").unwrap_err().kind(), io::ErrorKind::TimedOut);
    connection.set_read_timeout(None).unwrap();
    assert_eq!(connection.read(&mut bytes).unwrap(), 2);
    assert_eq!(&bytes, b"cd");
    assert_eq!(
        connection.set_read_timeout(Some(Duration::MAX)).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

#[test]
fn trickling_probe_times_out_and_discovery_continues() {
    let (io, control) = MonitorIo::new_mocked();
    let (directory, malicious) = descriptor(&io);
    io.publish_descriptor(&directory, &malicious).unwrap();
    let (_, healthy) = descriptor(&io);
    io.publish_descriptor(&directory, &healthy).unwrap();
    let healthy_worker = serve_discoverable(&io, &healthy);
    let listener = Listener::bind(&io, &malicious).unwrap();
    listener.set_nonblocking(true).unwrap();
    let (completed, completion) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || completed.send(discover(&io)).unwrap());
    let mut peer = accept(&listener);
    assert_eq!(read_request(&peer).unwrap(), (1, Request::Hello));
    let waits = || {
        control
            .operations()
            .iter()
            .filter(|operation| **operation == IoOperation::Wait)
            .count()
    };
    let wait_for_block = |before| {
        let watchdog = Instant::now() + Duration::from_secs(5);
        while waits() <= before {
            assert!(Instant::now() < watchdog, "probe did not reach its next read wait");
            thread::yield_now();
        }
    };
    let before = waits();
    control.advance_time(Duration::ZERO);
    wait_for_block(before);
    let before = waits();
    control.advance_time(Duration::from_secs(9));
    wait_for_block(before);
    let before = waits();
    peer.write_all(b"SGMP").unwrap();
    wait_for_block(before);
    control.advance_time(Duration::from_secs(1));
    let result = completion.recv_timeout(Duration::from_secs(5));
    peer.close().unwrap();
    let discovery = result.unwrap().unwrap();
    assert_eq!(discovery.instances.len(), 1);
    assert_eq!(discovery.instances[0].descriptor, healthy);
    assert_eq!(discovery.warnings.len(), 1);
    assert!(discovery.warnings[0].contains("timed out"));
    worker.join().unwrap();
    healthy_worker.join().unwrap();
}

#[test]
fn explicit_snapshot_commands_still_accept_payloads_larger_than_control_responses() {
    let (io, _) = MonitorIo::new_mocked();
    let (_, descriptor) = descriptor(&io);
    let bytes = vec![7; 65_537];
    let worker = serve(
        &io,
        &descriptor,
        vec![vec![
            (Request::Hello, hello(&descriptor)),
            (Request::CaptureSnapshotAndStop, Response::Snapshot(bytes.clone())),
        ]],
    );
    assert_eq!(capture_snapshot_and_stop(&io, &descriptor).unwrap(), bytes);
    worker.join().unwrap();
}

#[test]
fn monitor_order_uses_owner_then_name_instance_and_process_not_discovery_order() {
    use seismograph_protocol::monitor::{InstanceId, MonitorOwner};

    let instance_id = InstanceId::from_bytes([1; 16]);
    let mut instances = [
        (1002, "alpha", None, 1),
        (1001, "zeta", None, 1),
        (1001, "alpha", Some("b"), 1),
        (1001, "alpha", Some("a"), 2),
        (1001, "alpha", Some("a"), 1),
        (1001, "alpha", None, 99),
    ]
    .map(|(uid, name, instance, process_id)| {
        let owner = MonitorOwner::UnixUser(uid);
        Instance {
            descriptor: MonitorDescriptor {
                endpoint: MonitorEndpoint::for_instance(Path::new("/registry"), &owner, process_id, instance_id),
                owner,
                name: name.into(),
                instance: instance.map(str::to_owned),
                process_id,
                instance_id,
            },
            recording: RecordingConfiguration::default(),
            server_version: None,
        }
    });
    sort_instances(&mut instances);
    let actual = instances
        .iter()
        .map(|instance| {
            (
                instance.descriptor.owner.to_string(),
                instance.descriptor.name.as_str(),
                instance.descriptor.instance.as_deref(),
                instance.descriptor.process_id,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        [
            ("1001".into(), "alpha", None, 99),
            ("1001".into(), "alpha", Some("a"), 1),
            ("1001".into(), "alpha", Some("a"), 2),
            ("1001".into(), "alpha", Some("b"), 1),
            ("1001".into(), "zeta", None, 1),
            ("1002".into(), "alpha", None, 1),
        ]
    );
}

#[test]
fn snapshot_saving_uses_mocked_wall_time_and_preserves_write_errors() {
    let (io, control) = MonitorIo::new_mocked();
    let (_, descriptor) = descriptor(&io);
    control.advance_time(Duration::from_secs(5));
    let path = save_snapshot(&io, &descriptor, &[1, 2, 3]).unwrap();
    assert_eq!(path, Path::new(".").join("worker-west-europe-42-5000000000.seismograph"));
    assert_eq!(control.file_bytes(&path).unwrap(), [1, 2, 3]);
    control.fail_next(
        IoOperation::WriteSnapshot,
        io::Error::new(io::ErrorKind::PermissionDenied, "output is read-only"),
    );
    assert!(
        save_snapshot(&io, &descriptor, &[4])
            .unwrap_err()
            .to_string()
            .contains("output is read-only")
    );
    assert_eq!(control.file_bytes(&path).unwrap(), [1, 2, 3]);
}

#[test]
fn capability_probe_distinguishes_eof_truncation_and_io_failures() {
    let (io, control) = MonitorIo::new_mocked();
    let (mut peer, connection) = control.connected_pair();
    let mut connection = ClientConnection::new(&io, connection).unwrap();
    write_response(&mut peer, 2, &Response::CacheRecording(RecordingPolicy::default())).unwrap();
    control.fail_next(IoOperation::Peek, io::ErrorKind::TimedOut.into());
    assert!(matches!(read_cache_recording(&mut connection), Err(Error::Io(error)) if error.kind() == io::ErrorKind::TimedOut));
    assert_eq!(read_cache_recording(&mut connection).unwrap(), Some(RecordingPolicy::default()));
    peer.write_all(b"SGMP").unwrap();
    drop(peer);
    assert!(matches!(
        read_cache_recording(&mut connection),
        Err(Error::Protocol(seismograph_protocol::Error::Io(error))) if error.kind() == io::ErrorKind::UnexpectedEof
    ));
    let (peer, connection) = control.connected_pair();
    let mut connection = ClientConnection::new(&io, connection).unwrap();
    drop(peer);
    assert_eq!(read_cache_recording(&mut connection).unwrap(), None);
    assert_eq!(read_server_version(&mut connection).unwrap(), None);
    assert_eq!(read_recorder_activity(&mut connection).unwrap(), None);
}
