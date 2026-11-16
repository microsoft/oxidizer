// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "test-util")]

//! Deterministic registry and transport survivor oracles.

use std::io::{self, Read, Write};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use seismograph_protocol::Error;
use seismograph_protocol::discovery::{self, Directory};
use seismograph_protocol::io::{IoOperation, MonitorIo};
use seismograph_protocol::monitor::{InstanceId, MonitorDescriptor, MonitorEndpoint, MonitorOwner};
use seismograph_protocol::transport::{Connection, Listener};

fn descriptor(io: &MonitorIo, directory: &Directory) -> MonitorDescriptor {
    let owner = directory.owner().clone();
    let instance_id = InstanceId::from_bytes([0x27; 16]);
    MonitorDescriptor {
        name: "worker".into(),
        instance: Some("shard".into()),
        process_id: io.process_id(),
        instance_id,
        endpoint: MonitorEndpoint::for_instance(directory.path(), &owner, io.process_id(), instance_id),
        owner,
    }
}

fn other_owner(owner: &MonitorOwner) -> MonitorOwner {
    match owner {
        MonitorOwner::UnixUser(_) => MonitorOwner::UnixUser(2002),
        MonitorOwner::WindowsSid(_) => MonitorOwner::WindowsSid("S-1-5-21-1-2-3-2002".into()),
    }
}

fn registry_error(error: Error, expected_path: &Path, expected_kind: io::ErrorKind) -> io::Error {
    let (path, source) = match error {
        Error::Registry { path, source } => Some((path, source)),
        _ => None,
    }
    .expect("test callers must supply an Error::Registry to inspect its path and source");
    assert_eq!(path, expected_path);
    assert_eq!(source.kind(), expected_kind);
    source
}

#[test]
fn mock_identity_clocks_and_operation_trace() {
    let (io, control) = MonitorIo::new_mocked();
    #[cfg(windows)]
    assert_eq!(io.current_exe().unwrap(), Path::new(r"C:\seismograph-mock\worker"));
    #[cfg(unix)]
    assert_eq!(io.current_exe().unwrap(), Path::new("/seismograph-mock/worker"));
    let first = io.instance_id().unwrap();
    let second = io.instance_id().unwrap();
    assert_eq!(first.as_bytes(), [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(second.as_bytes(), [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let start = io.instant();
    assert_eq!(io.system_time(), UNIX_EPOCH);
    control.advance_time(Duration::from_secs(7));
    assert_eq!(io.instant().duration_since(start), Duration::from_secs(7));
    assert_eq!(io.system_time(), UNIX_EPOCH + Duration::from_secs(7));
    assert_eq!(
        control.operations(),
        [
            IoOperation::CurrentExe,
            IoOperation::InstanceId,
            IoOperation::InstanceId,
            IoOperation::Instant,
            IoOperation::SystemTime,
            IoOperation::Instant,
            IoOperation::SystemTime,
        ]
    );
    control.fail_next(IoOperation::InstanceId, io::Error::other("entropy refused"));
    assert!(matches!(io.instance_id(), Err(Error::Io(source)) if source.to_string() == "entropy refused"));
    control.fail_next(IoOperation::CurrentOwner, io::Error::other("owner refused"));
    assert!(matches!(Directory::current(&io), Err(Error::Io(source)) if source.to_string() == "owner refused"));
    control.fail_next(IoOperation::DiscoverDirectories, io::Error::other("discovery refused"));
    assert!(matches!(discovery::directories(&io), Err(Error::Io(source)) if source.to_string() == "discovery refused"));
}

#[test]
fn gamma_survivor_publication_rejects_other_owners_instances_and_endpoints() {
    let (io, control) = MonitorIo::new_mocked();
    let directory = Directory::current(&io).unwrap();
    let valid = descriptor(&io, &directory);
    let (other_io, _) = MonitorIo::new_mocked();
    let error = other_io.publish_descriptor(&directory, &valid).unwrap_err();
    assert_eq!(
        registry_error(error, directory.path(), io::ErrorKind::PermissionDenied).to_string(),
        "descriptor and directory must belong to the same I/O instance and owner"
    );
    let mut invalid = valid.clone();
    invalid.owner = other_owner(&valid.owner);
    let error = io.publish_descriptor(&directory, &invalid).unwrap_err();
    assert_eq!(
        registry_error(error, directory.path(), io::ErrorKind::PermissionDenied).to_string(),
        "descriptor and directory must belong to the same I/O instance and owner"
    );
    invalid = valid.clone();
    invalid.endpoint = MonitorEndpoint::NamedPipe(r"\\.\pipe\other".into());
    assert!(matches!(io.publish_descriptor(&directory, &invalid), Err(Error::InvalidDescriptor)));
    assert!(!control.operations().contains(&IoOperation::CreateDescriptor));
    let path = io.publish_descriptor(&directory, &valid).unwrap();
    assert_eq!(MonitorDescriptor::read_file(&io, &path).unwrap(), valid);
    assert!(control.file_bytes(&path.with_extension("monitor.tmp")).is_none());
}

#[test]
fn gamma_survivor_descriptor_name_owner_endpoint_and_empty_instance_checks() {
    let (io, control) = MonitorIo::new_mocked();
    let directory = Directory::current(&io).unwrap();
    let valid = descriptor(&io, &directory);
    let path = io.publish_descriptor(&directory, &valid).unwrap();
    let bytes = control.file_bytes(&path).unwrap();
    let foreign_owner = other_owner(&valid.owner);
    control.set_owner(foreign_owner);
    let foreign_directory = Directory::current(&io).unwrap();
    let foreign = descriptor(&io, &foreign_directory);
    let foreign_path = io.publish_descriptor(&foreign_directory, &foreign).unwrap();
    let foreign_bytes = control.file_bytes(&foreign_path).unwrap();
    control.set_owner(valid.owner.clone());
    control.put_file(&path, valid.owner.clone(), foreign_bytes);
    assert!(matches!(MonitorDescriptor::read_file(&io, &path), Err(Error::InvalidDescriptor)));
    control.put_file(&path, valid.owner.clone(), bytes.clone());
    let wrong_path = directory.path().join("wrong.monitor");
    control.put_file(&wrong_path, valid.owner.clone(), bytes.clone());
    assert!(matches!(
        MonitorDescriptor::read_file(&io, &wrong_path),
        Err(Error::InvalidDescriptor)
    ));
    let mut wrong_endpoint = bytes.clone();
    let endpoint_start = 29 + valid.owner.to_string().len() + 3;
    wrong_endpoint[endpoint_start] = b'x';
    control.put_file(&path, valid.owner.clone(), wrong_endpoint);
    assert!(matches!(MonitorDescriptor::read_file(&io, &path), Err(Error::InvalidDescriptor)));
    let mut invalid = valid.clone();
    invalid.instance = Some(String::new());
    assert!(matches!(
        invalid.write_file(&io, directory.path().join("empty")),
        Err(Error::InvalidDescriptor)
    ));
    for field in [0, 1] {
        invalid = valid.clone();
        if field == 0 {
            invalid.name = "x".repeat(65_536);
        } else {
            invalid.instance = Some("x".repeat(65_536));
        }
        assert!(matches!(
            invalid.write_file(&io, directory.path().join("large")),
            Err(Error::MessageTooLarge)
        ));
    }
    let mut empty_instance = bytes;
    empty_instance.truncate(empty_instance.len() - 5);
    empty_instance.extend_from_slice(&[0, 0]);
    control.put_file(&path, valid.owner, empty_instance);
    assert!(matches!(MonitorDescriptor::read_file(&io, &path), Err(Error::InvalidDescriptor)));
}

#[test]
fn gamma_survivor_descriptor_read_and_publication_failures_keep_context_and_cleanup() {
    let (io, control) = MonitorIo::new_mocked();
    let directory = Directory::current(&io).unwrap();
    let valid = descriptor(&io, &directory);
    let path = directory.path().join(valid.file_name());
    control.fail_next(IoOperation::WriteDescriptor, io::Error::other("write refused"));
    let error = io.publish_descriptor(&directory, &valid).unwrap_err();
    assert_eq!(
        registry_error(error, &path.with_extension("monitor.tmp"), io::ErrorKind::Other).to_string(),
        "write refused"
    );
    assert!(control.file_bytes(&path.with_extension("monitor.tmp")).is_none());
    control.fail_next(IoOperation::PublishDescriptor, io::Error::other("rename refused"));
    let error = io.publish_descriptor(&directory, &valid).unwrap_err();
    assert_eq!(registry_error(error, &path, io::ErrorKind::Other).to_string(), "rename refused");
    assert!(control.file_bytes(&path.with_extension("monitor.tmp")).is_none());
    control.set_max_chunk(3);
    io.publish_descriptor(&directory, &valid).unwrap();
    assert_eq!(MonitorDescriptor::read_file(&io, &path).unwrap(), valid);
    control.set_secure(&path, false);
    assert_eq!(
        registry_error(
            MonitorDescriptor::read_file(&io, &path).unwrap_err(),
            &path,
            io::ErrorKind::PermissionDenied
        )
        .to_string(),
        "monitor descriptor must be a private regular file"
    );
    control.set_secure(&path, true);
    assert_eq!(MonitorDescriptor::read_file(&io, &path).unwrap(), valid);
    control.set_secure(directory.path(), false);
    assert_eq!(
        registry_error(
            Directory::open(&io, directory.path(), directory.owner()).unwrap_err(),
            directory.path(),
            io::ErrorKind::PermissionDenied,
        )
        .to_string(),
        "monitor directory has unsafe ownership or permissions"
    );
}

#[test]
fn gamma_survivor_discovery_does_not_hide_owner_or_nonpermission_os_errors() {
    let (io, control) = MonitorIo::new_mocked();
    let directory = Directory::current(&io).unwrap();
    #[cfg(windows)]
    let (access_denied, invalid_input) = (5, 87);
    #[cfg(unix)]
    let (access_denied, invalid_input) = (13, 22);
    control.fail_next(IoOperation::OpenDirectory, io::Error::from_raw_os_error(access_denied));
    let error = discovery::directories(&io).unwrap().next().unwrap().unwrap_err();
    assert_eq!(
        registry_error(error, directory.path(), io::ErrorKind::PermissionDenied).raw_os_error(),
        Some(access_denied)
    );
    control.set_owner(other_owner(directory.owner()));
    control.fail_next(IoOperation::OpenDirectory, io::Error::from_raw_os_error(invalid_input));
    let error = discovery::directories(&io).unwrap().next().unwrap().unwrap_err();
    assert_eq!(
        registry_error(error, directory.path(), io::ErrorKind::InvalidInput).raw_os_error(),
        Some(invalid_input)
    );
    assert!(discovery::directories(&io).unwrap().next().is_none());
}

#[test]
fn gamma_survivor_bind_connect_peer_and_deadline_validation() {
    let (io, control) = MonitorIo::new_mocked();
    let directory = Directory::current(&io).unwrap();
    let valid = descriptor(&io, &directory);
    let mut invalid = valid.clone();
    invalid.owner = other_owner(&valid.owner);
    invalid.endpoint = MonitorEndpoint::for_instance(directory.path(), &invalid.owner, invalid.process_id, invalid.instance_id);
    assert_eq!(
        Listener::bind(&io, &invalid).unwrap_err().to_string(),
        "monitor endpoint owner or identity does not match"
    );
    invalid = valid.clone();
    invalid.endpoint = MonitorEndpoint::NamedPipe(r"\\.\pipe\other".into());
    assert_eq!(
        Connection::connect(&io, &invalid, Duration::from_secs(1)).unwrap_err().to_string(),
        "monitor endpoint does not match its identity"
    );
    for timeout in [Duration::ZERO, Duration::MAX] {
        assert_eq!(
            Connection::connect(&io, &valid, timeout).unwrap_err().to_string(),
            "connection timeout must be positive and bounded"
        );
    }
    control.fail_next(IoOperation::CurrentOwner, io::Error::other("owner unavailable"));
    assert_eq!(Listener::bind(&io, &valid).unwrap_err().to_string(), "owner unavailable");
    control.fail_next(IoOperation::Bind, io::Error::other("bind unavailable"));
    assert_eq!(Listener::bind(&io, &valid).unwrap_err().to_string(), "bind unavailable");
    let listener = Listener::bind(&io, &valid).unwrap();
    listener.set_nonblocking(true).unwrap();
    assert_eq!(listener.accept().unwrap_err().kind(), io::ErrorKind::WouldBlock);
    control.fail_next(IoOperation::Accept, io::Error::other("accept unavailable"));
    assert_eq!(listener.accept().unwrap_err().to_string(), "accept unavailable");
    let mut client = Connection::connect(&io, &valid, Duration::from_secs(1)).unwrap();
    let mut server = listener.accept().unwrap();
    client.write_all(b"one").unwrap();
    let mut bytes = [0; 3];
    server.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"one");
    drop(listener);
    assert_eq!(
        Connection::connect(&io, &valid, Duration::from_secs(1)).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
}

#[test]
fn gamma_survivor_mock_peer_owner_is_not_only_process_id() {
    let (io, control) = MonitorIo::new_mocked();
    let directory = Directory::current(&io).unwrap();
    let valid = descriptor(&io, &directory);
    let owner = valid.owner.clone();
    let listener = Listener::bind(&io, &valid).unwrap();
    control.set_owner(other_owner(&owner));
    control.set_administrator(true);
    let foreign = descriptor(&io, &Directory::current(&io).unwrap());
    let foreign_listener = Listener::bind(&io, &foreign).unwrap();
    control.set_owner(owner);
    let mut claim = valid;
    claim.endpoint = foreign.endpoint;
    let error = Connection::connect(&io, &claim, Duration::from_secs(1)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    #[cfg(unix)]
    assert_eq!(error.to_string(), "connected monitor has a different OS identity");
    #[cfg(windows)]
    assert_eq!(error.to_string(), "monitor endpoint does not match its identity");
    drop((listener, foreign_listener));
}

#[test]
fn gamma_survivor_peek_preserves_bytes_clones_share_close_and_errors() {
    let (io, control) = MonitorIo::new_mocked();
    let (mut left, mut right) = control.connected_pair();
    control.set_max_chunk(2);
    right.write_all(b"abcd").unwrap();
    let mut bytes = [0; 4];
    assert_eq!(left.peek(&mut bytes).unwrap(), 2);
    assert_eq!(&bytes, &[b'a', b'b', 0, 0]);
    left.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"abcd");
    for timeout in [Duration::ZERO, Duration::MAX] {
        assert_eq!(
            left.set_read_timeout(Some(timeout)).unwrap_err().to_string(),
            "mock timeout must be positive and bounded"
        );
        assert_eq!(
            left.set_write_timeout(Some(timeout)).unwrap_err().to_string(),
            "mock timeout must be positive and bounded"
        );
    }
    left.set_read_timeout(None).unwrap();
    left.set_write_timeout(Some(Duration::from_secs(1))).unwrap();
    control.fail_next(IoOperation::Duplicate, io::Error::other("duplicate refused"));
    assert_eq!(left.try_clone().unwrap_err().to_string(), "duplicate refused");
    control.fail_next(IoOperation::Peek, io::Error::other("peek refused"));
    assert_eq!(left.peek(&mut bytes).unwrap_err().to_string(), "peek refused");
    let clone = left.try_clone().unwrap();
    drop(clone);
    right.write_all(b"xy").unwrap();
    left.read_exact(&mut bytes[..2]).unwrap();
    assert_eq!(&bytes[..2], b"xy");
    let clone = left.try_clone().unwrap();
    clone.close().unwrap();
    assert_eq!(left.read(&mut bytes).unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
    assert_eq!(right.read(&mut bytes).unwrap(), 0);
    assert_eq!(right.write(b"x").unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(io.system_time(), UNIX_EPOCH);
}
