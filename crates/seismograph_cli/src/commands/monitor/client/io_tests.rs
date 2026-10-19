// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Write as _;
use std::path::Path;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use seismograph_protocol::discovery::Directory;
use seismograph_protocol::io::IoOperation;
use seismograph_protocol::monitor::MonitorEndpoint;
use seismograph_protocol::transport::Listener;
use seismograph_protocol::{read_request, write_response};

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

fn serve(io: &MonitorIo, descriptor: &MonitorDescriptor, conversations: Vec<Vec<(Request, Response)>>) -> JoinHandle<()> {
    let listener = Listener::bind(io, descriptor).unwrap();
    listener.set_nonblocking(true).unwrap();
    thread::spawn(move || {
        for conversation in conversations {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok(stream) => break stream,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "client did not connect for the scripted conversation");
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("scripted accept failed: {error}"),
                }
            };
            for (request, response) in conversation {
                let (id, actual) = read_request(&mut stream).unwrap();
                assert_eq!(actual, request);
                write_response(&mut stream, id, &response).unwrap();
            }
        }
    })
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
    for operation in [
        IoOperation::Connect,
        IoOperation::ReadTimeout,
        IoOperation::WriteTimeout,
        IoOperation::Write,
        IoOperation::Read,
        IoOperation::Flush,
    ] {
        let (io, control) = MonitorIo::new_mocked();
        let (_, descriptor) = descriptor(&io);
        let _listener = Listener::bind(&io, &descriptor).unwrap();
        control.fail_next(
            operation,
            io::Error::new(io::ErrorKind::PermissionDenied, "injected client failure"),
        );
        let error = handshake(&io, &descriptor).unwrap_err();
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
    let (_, control) = MonitorIo::new_mocked();
    let (mut peer, mut connection) = control.connected_pair();
    control.fail_next(IoOperation::Peek, io::ErrorKind::TimedOut.into());
    assert!(matches!(read_cache_recording(&mut connection), Err(Error::Io(error)) if error.kind() == io::ErrorKind::TimedOut));
    peer.write_all(b"SGMP").unwrap();
    drop(peer);
    assert!(matches!(
        read_cache_recording(&mut connection),
        Err(Error::Protocol(seismograph_protocol::Error::Io(error))) if error.kind() == io::ErrorKind::UnexpectedEof
    ));
    let (peer, mut connection) = control.connected_pair();
    drop(peer);
    assert_eq!(read_cache_recording(&mut connection).unwrap(), None);
    assert_eq!(read_server_version(&mut connection).unwrap(), None);
    assert_eq!(read_recorder_activity(&mut connection).unwrap(), None);
}
