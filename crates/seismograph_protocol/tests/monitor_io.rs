// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Instance-isolated monitor I/O without host files or endpoints.

#![cfg(feature = "test-util")]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

#[cfg(test)]
mod tests {

    use std::io::{self, Read as _, Write as _};
    use std::path::Path;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant, UNIX_EPOCH};

    use seismograph_protocol::discovery::{self, Directory};
    use seismograph_protocol::io::{IoOperation, MonitorIo, MonitorIoControl};
    use seismograph_protocol::message::{Request, Response};
    use seismograph_protocol::monitor::{MonitorDescriptor, MonitorEndpoint, MonitorOwner};
    use seismograph_protocol::transport::{Connection, Listener};
    use seismograph_protocol::{Error, read_request, read_response, write_request, write_response};

    fn descriptor(io: &MonitorIo) -> (Directory, MonitorDescriptor) {
        let directory = Directory::current(io).unwrap();
        let owner = io.current_owner().unwrap();
        let process_id = io.process_id();
        let instance_id = io.instance_id().unwrap();
        let descriptor = MonitorDescriptor {
            name: "worker".into(),
            instance: None,
            endpoint: MonitorEndpoint::for_instance(directory.path(), &owner, process_id, instance_id),
            owner,
            process_id,
            instance_id,
        };
        (directory, descriptor)
    }

    fn error_kind(error: &Error) -> io::ErrorKind {
        match error {
            Error::Io(source) | Error::Registry { source, .. } => source.kind(),
            error => panic!("expected an I/O error, got {error}"),
        }
    }

    fn wait_for_blocked(control: &MonitorIoControl) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !control.operations().contains(&IoOperation::Wait) {
            assert!(Instant::now() < deadline, "mock transport did not block");
            thread::yield_now();
        }
    }

    #[test]
    fn clones_share_identity_failures_and_operations_but_other_instances_do_not() {
        let (io, control) = MonitorIo::new_mocked();
        let clone = io.clone();
        let (other, other_control) = MonitorIo::new_mocked();
        control.fail_next(IoOperation::CurrentExe, io::ErrorKind::PermissionDenied.into());
        assert_eq!(clone.process_id(), 42);
        assert_eq!(clone.current_exe().unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(io.current_exe().unwrap().file_name(), Some(std::ffi::OsStr::new("worker")));
        assert_eq!(
            control.operations(),
            [IoOperation::ProcessId, IoOperation::CurrentExe, IoOperation::CurrentExe]
        );
        assert_eq!(other_control.operations(), []);
        assert_ne!(io.instance_id().unwrap(), clone.instance_id().unwrap());
        assert_eq!(other.instance_id().unwrap().as_bytes()[..8], 1_u64.to_le_bytes());
    }

    #[test]
    fn mocked_time_advances_only_when_requested() {
        let (io, control) = MonitorIo::new_mocked();
        let start = io.instant();
        io.wait(Duration::from_mins(1));
        assert_eq!(io.instant(), start);
        assert_eq!(io.system_time(), UNIX_EPOCH);
        control.advance_time(Duration::from_secs(7));
        assert_eq!(io.instant().duration_since(start), Duration::from_secs(7));
        assert_eq!(io.system_time().duration_since(UNIX_EPOCH).unwrap(), Duration::from_secs(7));
    }

    #[test]
    fn discovery_is_read_only_and_startup_initializes_the_user_registry() {
        let (io, control) = MonitorIo::new_mocked();
        control.fail_next(IoOperation::CreateDirectory, io::ErrorKind::PermissionDenied.into());
        assert_eq!(discovery::directories(&io).unwrap().count(), 0);
        assert!(!control.operations().contains(&IoOperation::CreateDirectory));
        assert_eq!(error_kind(&Directory::current(&io).unwrap_err()), io::ErrorKind::PermissionDenied);
        let directory = Directory::current(&io).unwrap();
        let entries = discovery::directories(&io).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), directory.path());
    }

    #[test]
    fn publication_round_trips_partial_writes_and_removes_only_its_descriptor() {
        let (io, control) = MonitorIo::new_mocked();
        control.set_max_chunk(3);
        let (directory, descriptor) = descriptor(&io);
        let unrelated = directory.path().join("notes.txt");
        control.put_file(&unrelated, descriptor.owner.clone(), b"keep".to_vec());
        let path = io.publish_descriptor(&directory, &descriptor).unwrap();
        assert_eq!(MonitorDescriptor::read_file(&io, &path).unwrap(), descriptor);
        assert_eq!(
            directory.descriptors().unwrap().collect::<Result<Vec<_>, _>>().unwrap(),
            [descriptor]
        );
        io.remove_descriptor(&path).unwrap();
        assert_eq!(control.file_bytes(&path), None);
        assert_eq!(control.file_bytes(&unrelated).unwrap(), b"keep");
        assert_eq!(error_kind(&io.remove_descriptor(&path).unwrap_err()), io::ErrorKind::NotFound);
    }

    #[test]
    fn publication_replaces_an_existing_descriptor() {
        let (io, _) = MonitorIo::new_mocked();
        let (directory, mut value) = descriptor(&io);
        let path = io.publish_descriptor(&directory, &value).unwrap();
        value.name = "replacement".into();
        assert_eq!(io.publish_descriptor(&directory, &value).unwrap(), path);
        assert_eq!(MonitorDescriptor::read_file(&io, &path).unwrap(), value);
    }

    #[test]
    fn descriptor_removal_requires_parent_access() {
        let (io, control) = MonitorIo::new_mocked();
        let (directory, value) = descriptor(&io);
        let path = io.publish_descriptor(&directory, &value).unwrap();
        let original = control.file_bytes(&path).unwrap();
        #[cfg(unix)]
        control.set_owner(MonitorOwner::UnixUser(1002));
        #[cfg(windows)]
        control.set_owner(MonitorOwner::WindowsSid("S-1-5-21-1-2-3-1002".into()));
        assert_eq!(
            error_kind(&io.remove_descriptor(&path).unwrap_err()),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(control.file_bytes(&path), Some(original));
        control.set_administrator(true);
        io.remove_descriptor(&path).unwrap();
        assert_eq!(control.file_bytes(&path), None);
    }

    #[test]
    fn pending_native_read_observation_rejects_mocked_connections() {
        let (_, control) = MonitorIo::new_mocked();
        let (connection, _) = control.connected_pair();
        assert_eq!(
            seismograph_protocol::testing::read_is_pending(&connection).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }

    #[test]
    fn publication_failures_clean_up_only_exclusively_created_files() {
        for operation in [
            IoOperation::CreateDescriptor,
            IoOperation::WriteDescriptor,
            IoOperation::PublishDescriptor,
        ] {
            let (io, control) = MonitorIo::new_mocked();
            let (directory, descriptor) = descriptor(&io);
            let path = directory.path().join(descriptor.file_name());
            control.fail_next(
                operation,
                io::Error::new(io::ErrorKind::PermissionDenied, "injected publication failure"),
            );
            let error = io.publish_descriptor(&directory, &descriptor).unwrap_err();
            assert_eq!(error_kind(&error), io::ErrorKind::PermissionDenied, "{operation:?}");
            assert_eq!(control.file_bytes(&path), None);
            assert_eq!(control.file_bytes(&path.with_extension("monitor.tmp")), None);
        }

        let (io, control) = MonitorIo::new_mocked();
        let (directory, descriptor) = descriptor(&io);
        let temporary = directory.path().join(descriptor.file_name()).with_extension("monitor.tmp");
        control.put_file(&temporary, descriptor.owner.clone(), b"occupied".to_vec());
        assert_eq!(
            error_kind(&io.publish_descriptor(&directory, &descriptor).unwrap_err()),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(control.file_bytes(&temporary).unwrap(), b"occupied");
        assert!(!control.operations().contains(&IoOperation::RemoveDescriptor));
    }

    #[test]
    fn cleanup_failure_preserves_the_publication_failure_as_its_source() {
        let (io, control) = MonitorIo::new_mocked();
        let (directory, descriptor) = descriptor(&io);
        let temporary = directory.path().join(descriptor.file_name()).with_extension("monitor.tmp");
        control.fail_next(IoOperation::WriteDescriptor, io::Error::other("write failed"));
        control.fail_next(IoOperation::RemoveDescriptor, io::Error::other("cleanup failed"));
        let error = io.publish_descriptor(&directory, &descriptor).unwrap_err();
        assert!(error.to_string().contains("write failed"));
        assert!(error.to_string().contains("cleanup failed"));
        assert!(std::error::Error::source(&error).is_some());
        assert!(control.file_bytes(&temporary).is_some());
        io.remove_descriptor(&temporary).unwrap();
    }

    #[test]
    fn publication_rejects_directories_from_other_io_instances() {
        let (io, control) = MonitorIo::new_mocked();
        let (other, _) = MonitorIo::new_mocked();
        let (directory, descriptor) = descriptor(&io);
        assert_eq!(
            error_kind(&other.publish_descriptor(&directory, &descriptor).unwrap_err()),
            io::ErrorKind::PermissionDenied
        );
        assert!(!control.operations().contains(&IoOperation::CreateDescriptor));
    }

    #[test]
    fn invalid_descriptor_identity_is_rejected_before_creating_files() {
        let (io, control) = MonitorIo::new_mocked();
        let (directory, mut descriptor) = descriptor(&io);
        descriptor.process_id = 0;
        assert!(matches!(
            io.publish_descriptor(&directory, &descriptor),
            Err(Error::InvalidDescriptor)
        ));
        descriptor.process_id = io.process_id();
        descriptor.name.clear();
        assert!(matches!(
            io.publish_descriptor(&directory, &descriptor),
            Err(Error::InvalidDescriptor)
        ));
        assert!(!control.operations().contains(&IoOperation::CreateDescriptor));
    }

    #[test]
    fn unsafe_entries_and_injected_registry_errors_remain_visible() {
        let (io, control) = MonitorIo::new_mocked();
        let (directory, descriptor) = descriptor(&io);
        let path = io.publish_descriptor(&directory, &descriptor).unwrap();
        control.set_secure(&path, false);
        assert_eq!(
            error_kind(&MonitorDescriptor::read_file(&io, &path).unwrap_err()),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(directory.descriptors().unwrap().filter(Result::is_err).count(), 1);
        control.set_secure(&path, true);
        control.fail_next(IoOperation::Enumerate, io::ErrorKind::Other.into());
        assert_eq!(error_kind(&directory.descriptors().err().unwrap()), io::ErrorKind::Other);
        control.set_secure(directory.path(), false);
        assert_eq!(
            error_kind(&Directory::open(&io, directory.path(), directory.owner()).unwrap_err()),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(error_kind(&Directory::current(&io).unwrap_err()), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn malformed_and_incompatible_descriptors_do_not_hide_valid_peers() {
        let (io, control) = MonitorIo::new_mocked();
        let (directory, first) = descriptor(&io);
        let first_path = io.publish_descriptor(&directory, &first).unwrap();
        let (_, second) = descriptor(&io);
        let second_path = io.publish_descriptor(&directory, &second).unwrap();
        let mut incompatible = control.file_bytes(&first_path).unwrap();
        incompatible[4..6].copy_from_slice(&7_u16.to_le_bytes());
        control.put_file(&first_path, first.owner, incompatible);
        let descriptors = directory.descriptors().unwrap().collect::<Vec<_>>();
        assert_eq!(descriptors.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(
            descriptors
                .iter()
                .filter_map(|result| result.as_ref().ok())
                .any(|value| *value == second)
        );
        control.put_file(&second_path, second.owner, b"bad".to_vec());
        assert_eq!(directory.descriptors().unwrap().filter(Result::is_err).count(), 2);
    }

    #[test]
    fn discovery_and_connection_follow_owner_or_administrator_access() {
        let (io, control) = MonitorIo::new_mocked();
        let (first_directory, first) = descriptor(&io);
        io.publish_descriptor(&first_directory, &first).unwrap();
        let listener = Listener::bind(&io, &first).unwrap();

        let second_owner = match first.owner {
            MonitorOwner::UnixUser(_) => MonitorOwner::UnixUser(2002),
            MonitorOwner::WindowsSid(_) => MonitorOwner::WindowsSid("S-1-5-21-1-2-3-2002".into()),
        };
        control.set_owner(second_owner.clone());
        let (second_directory, second) = descriptor(&io);
        io.publish_descriptor(&second_directory, &second).unwrap();
        let visible = discovery::directories(&io).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].owner(), &second_owner);
        assert_eq!(
            Connection::connect(&io, &first, Duration::from_secs(1)).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        control.set_administrator(true);
        let visible = discovery::directories(&io).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(visible.len(), 2);
        let mut client = Connection::connect(&io, &first, Duration::from_secs(1)).unwrap();
        let mut server = listener.accept().unwrap();
        client.write_all(b"administrator").unwrap();
        let mut bytes = [0; 13];
        server.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"administrator");
    }

    #[test]
    fn listener_reports_busy_names_and_verifies_the_servers_actual_process() {
        let (io, _) = MonitorIo::new_mocked();
        let (_, mut descriptor) = descriptor(&io);
        descriptor.process_id += 1;
        descriptor.endpoint = MonitorEndpoint::for_instance(
            Directory::current(&io).unwrap().path(),
            &descriptor.owner,
            descriptor.process_id,
            descriptor.instance_id,
        );
        let listener = Listener::bind(&io, &descriptor).unwrap();
        assert_eq!(Listener::bind(&io, &descriptor).unwrap_err().kind(), io::ErrorKind::AddrInUse);
        listener.set_nonblocking(true).unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), io::ErrorKind::WouldBlock);
        assert_eq!(
            Connection::connect(&io, &descriptor, Duration::from_secs(1)).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        drop(listener);
        assert_eq!(
            Connection::connect(&io, &descriptor, Duration::from_secs(1)).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn transport_frames_survive_partial_reads_and_writes() {
        let (io, control) = MonitorIo::new_mocked();
        control.set_max_chunk(2);
        let (_, descriptor) = descriptor(&io);
        let listener = Listener::bind(&io, &descriptor).unwrap();
        let mut client = Connection::connect(&io, &descriptor, Duration::from_secs(1)).unwrap();
        let mut server = listener.accept().unwrap();
        write_request(&mut client, 9, &Request::ReadServerVersion).unwrap();
        assert_eq!(read_request(&mut server).unwrap(), (9, Request::ReadServerVersion));
        write_response(&mut server, 9, &Response::ServerVersion("0.2.0".into())).unwrap();
        assert_eq!(read_response(&mut client).unwrap(), (9, Response::ServerVersion("0.2.0".into())));
    }

    #[test]
    fn peek_preserves_bytes_and_last_clone_drop_produces_eof() {
        let (_, control) = MonitorIo::new_mocked();
        let (mut client, mut server) = control.connected_pair();
        let mut clone = client.try_clone().unwrap();
        client.write_all(b"abc").unwrap();
        let mut bytes = [0; 3];
        assert_eq!(server.peek(&mut bytes).unwrap(), 3);
        assert_eq!(&bytes, b"abc");
        server.read_exact(&mut bytes).unwrap();
        drop(client);
        clone.write_all(b"d").unwrap();
        drop(clone);
        assert_eq!(server.read(&mut bytes).unwrap(), 1);
        assert_eq!(bytes[0], b'd');
        assert_eq!(server.read(&mut bytes).unwrap(), 0);
    }

    #[test]
    fn advancing_time_releases_a_blocked_read_on_a_connection_clone() {
        let (_, control) = MonitorIo::new_mocked();
        let (_client, server) = control.connected_pair();
        server.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut clone = server.try_clone().unwrap();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || sender.send(clone.read(&mut [0]).unwrap_err().kind()).unwrap());
        wait_for_blocked(&control);
        control.advance_time(Duration::from_secs(10));
        assert_eq!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(), io::ErrorKind::TimedOut);
        worker.join().unwrap();
    }

    #[test]
    fn advancing_time_releases_a_write_blocked_by_backpressure() {
        let (_, control) = MonitorIo::new_mocked();
        let (mut client, _server) = control.connected_pair();
        client.write_all(&vec![0; 65_536]).unwrap();
        client.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || sender.send(client.write(&[1]).unwrap_err().kind()).unwrap());
        wait_for_blocked(&control);
        control.advance_time(Duration::from_secs(3));
        assert_eq!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(), io::ErrorKind::TimedOut);
        worker.join().unwrap();
    }

    #[test]
    fn close_cancels_blocked_clones_even_when_reporting_an_injected_error() {
        let (_, control) = MonitorIo::new_mocked();
        let (_client, mut server) = control.connected_pair();
        let cancellation = server.try_clone().unwrap();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || sender.send(server.read(&mut [0]).unwrap_err().kind()).unwrap());
        wait_for_blocked(&control);
        control.fail_next(IoOperation::Close, io::Error::other("injected close failure"));
        assert_eq!(cancellation.close().unwrap_err().kind(), io::ErrorKind::Other);
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            io::ErrorKind::ConnectionAborted
        );
        worker.join().unwrap();
    }

    #[test]
    fn injected_connection_errors_are_reported_and_consumed_once() {
        let (io, control) = MonitorIo::new_mocked();
        let (_, descriptor) = descriptor(&io);
        let listener = Listener::bind(&io, &descriptor).unwrap();
        control.fail_next(IoOperation::Connect, io::ErrorKind::PermissionDenied.into());
        assert_eq!(
            Connection::connect(&io, &descriptor, Duration::from_secs(1)).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        let mut client = Connection::connect(&io, &descriptor, Duration::from_secs(1)).unwrap();
        let mut server = listener.accept().unwrap();
        control.fail_next(IoOperation::Write, io::ErrorKind::Interrupted.into());
        client.write_all(b"ok").unwrap();
        control.fail_next(IoOperation::Read, io::ErrorKind::Interrupted.into());
        let mut bytes = [0; 2];
        server.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ok");
        control.fail_next(IoOperation::Flush, io::ErrorKind::Other.into());
        assert_eq!(client.flush().unwrap_err().kind(), io::ErrorKind::Other);
        client.flush().unwrap();
    }

    #[test]
    fn injected_error_after_partial_frame_is_not_treated_as_clean_eof() {
        let (_, control) = MonitorIo::new_mocked();
        let (mut peer, mut connection) = control.connected_pair();
        peer.write_all(b"SGMP").unwrap();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || sender.send(read_request(&mut connection)).unwrap());
        wait_for_blocked(&control);
        control.fail_next(IoOperation::Read, io::ErrorKind::PermissionDenied.into());
        peer.write_all(&[8]).unwrap();
        let error = receiver.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err();
        assert_eq!(error_kind(&error), io::ErrorKind::PermissionDenied);
        worker.join().unwrap();
    }

    #[test]
    fn mocked_snapshot_writes_are_isolated_and_fallible() {
        let (io, control) = MonitorIo::new_mocked();
        let path = Path::new("capture.seismograph");
        io.write_snapshot(path, b"snapshot").unwrap();
        assert_eq!(control.file_bytes(path).unwrap(), b"snapshot");
        control.fail_next(IoOperation::WriteSnapshot, io::ErrorKind::PermissionDenied.into());
        assert_eq!(
            io.write_snapshot(path, b"replacement").unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(control.file_bytes(path).unwrap(), b"snapshot");
        let missing = Path::new("missing").join("capture.seismograph");
        assert_eq!(
            io.write_snapshot(&missing, b"snapshot").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(control.file_bytes(&missing), None);
        assert_eq!(
            io.write_snapshot(Path::new("."), b"snapshot").unwrap_err().kind(),
            io::ErrorKind::IsADirectory
        );
    }
}
