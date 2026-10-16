// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cross-user authorization using isolated Linux subprocess identities.

#![cfg(all(target_os = "linux", feature = "test-util", not(miri)))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

#[cfg(test)]
mod tests {

    use std::fs;
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::process::CommandExt as _;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use seismograph_protocol::message::{Request, Response};
    use seismograph_protocol::monitor::{InstanceId, MonitorDescriptor, MonitorEndpoint, MonitorOwner};
    use seismograph_protocol::transport::{Connection, Listener};
    use seismograph_protocol::{read_request, read_response, write_request, write_response};

    const OWNER: u32 = 60_001;
    const OTHER: u32 = 60_002;

    fn worker(mode: &str, directory: &std::path::Path, uid: u32) -> Command {
        let mut command =
            Command::new(std::env::current_exe().expect("the integration test executable is available for its subprocess fixtures"));
        command.args(["--ignored", "--exact", "tests::identity_worker", "--nocapture"]);
        command
            .env("SEISMOGRAPH_TEST_MODE", mode)
            .env("SEISMOGRAPH_TEST_DIRECTORY", directory)
            .env("XDG_RUNTIME_DIR", directory);
        command.uid(uid).gid(uid);
        command
    }

    #[test]
    fn owner_and_root_connect_but_another_user_cannot() {
        verify_authorization(true);
    }

    #[test]
    fn private_temporary_registry_allows_owner_and_root_without_runtime_setup() {
        verify_authorization(false);
    }

    fn verify_authorization(with_runtime: bool) {
        if MonitorOwner::current().unwrap() != MonitorOwner::UnixUser(0) {
            eprintln!("cross-user case requires root; ordinary-user native cases run separately");
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let directory = temporary.path().join(OWNER.to_string());
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let file = fs::File::open(&directory).unwrap();
        // SAFETY: the fixture directory descriptor is live; identities are isolated
        // numeric test IDs, not host accounts, and no parent process UID is changed.
        assert_eq!(unsafe { libc::fchown(file.as_raw_fd(), OWNER, OWNER) }, 0);
        let mut command = worker("server", &directory, OWNER);
        if !with_runtime {
            command.env_remove("XDG_RUNTIME_DIR");
        }
        let mut server = command.stdout(Stdio::piped()).spawn().unwrap();
        let mut output = BufReader::new(server.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(output.read_line(&mut line).unwrap(), 0, "server failed before publishing");
            if line.starts_with("READY ") {
                break;
            }
        }
        let path = PathBuf::from(line.trim().strip_prefix("READY ").unwrap());
        let directory = path.parent().unwrap();
        let descriptor = MonitorDescriptor::read_file(&seismograph_protocol::io::MonitorIo::new_native(), &path).unwrap();
        assert_eq!(descriptor.owner, MonitorOwner::UnixUser(OWNER));
        let discovered = seismograph_protocol::discovery::directories(&seismograph_protocol::io::MonitorIo::new_native())
            .unwrap()
            .filter_map(Result::ok)
            .find(|candidate| candidate.path() == directory)
            .unwrap();
        assert!(
            discovered
                .descriptors()
                .unwrap()
                .any(|entry| entry.as_ref().is_ok_and(|entry| *entry == descriptor))
        );
        let mut root = Connection::connect(
            &seismograph_protocol::io::MonitorIo::new_native(),
            &descriptor,
            Duration::from_secs(1),
        )
        .unwrap();
        write_request(&mut root, 1, &Request::Hello).unwrap();
        assert_eq!(read_response(&mut root).unwrap(), (1, Response::Acknowledged));
        assert!(worker("owner", directory, OWNER).status().unwrap().success());
        let status = worker("other", directory, OTHER)
            .env("SEISMOGRAPH_TEST_ENDPOINT", descriptor.endpoint.path())
            .status()
            .unwrap();
        assert!(status.success());
        let mut finish = Connection::connect(
            &seismograph_protocol::io::MonitorIo::new_native(),
            &descriptor,
            Duration::from_secs(1),
        )
        .unwrap();
        write_request(&mut finish, 1, &Request::Hello).unwrap();
        assert_eq!(read_response(&mut finish).unwrap(), (1, Response::Acknowledged));
        assert!(server.wait().unwrap().success());
    }

    #[test]
    #[ignore = "subprocess helper, invoked with an isolated UID by the authorization test"]
    fn identity_worker() {
        let directory = PathBuf::from(std::env::var_os("SEISMOGRAPH_TEST_DIRECTORY").unwrap());
        match std::env::var("SEISMOGRAPH_TEST_MODE").unwrap().as_str() {
            "server" => {
                let io = seismograph_protocol::io::MonitorIo::new_native();
                let registry = seismograph_protocol::discovery::Directory::current(&io).unwrap();
                let directory = registry.path();
                let owner = MonitorOwner::current().unwrap();
                let process_id = std::process::id();
                let instance_id = InstanceId::from_bytes([7; 16]);
                let descriptor = MonitorDescriptor {
                    name: "worker".into(),
                    instance: None,
                    process_id,
                    instance_id,
                    endpoint: MonitorEndpoint::for_instance(directory, &owner, process_id, instance_id),
                    owner,
                };
                let listener = Listener::bind(&seismograph_protocol::io::MonitorIo::new_native(), &descriptor).unwrap();
                listener.set_nonblocking(true).unwrap();
                let path = directory.join(descriptor.file_name());
                descriptor
                    .write_file(&seismograph_protocol::io::MonitorIo::new_native(), &path)
                    .unwrap();
                println!("READY {}", path.display());
                std::io::stdout().flush().unwrap();
                let deadline = Instant::now() + Duration::from_secs(10);
                let mut accepted = 0;
                // Keep the registry alive until the unauthorized peer has tried,
                // then accept root's final acknowledgement before shutting down.
                while accepted != 3 {
                    match listener.accept() {
                        Ok(mut connection) => {
                            connection.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                            assert_eq!(read_request(&mut connection).unwrap(), (1, Request::Hello));
                            write_response(&mut connection, 1, &Response::Acknowledged).unwrap();
                            accepted += 1;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("server did not receive authorized clients: {error}"),
                    }
                }
                drop(listener);
                io.remove_descriptor(&path).unwrap();
            }
            "owner" => {
                let entry = fs::read_dir(directory)
                    .unwrap()
                    .map(Result::unwrap)
                    .find(|entry| entry.path().extension().is_some_and(|extension| extension == "monitor"))
                    .unwrap();
                let descriptor = MonitorDescriptor::read_file(&seismograph_protocol::io::MonitorIo::new_native(), entry.path()).unwrap();
                let mut connection = Connection::connect(
                    &seismograph_protocol::io::MonitorIo::new_native(),
                    &descriptor,
                    Duration::from_secs(1),
                )
                .unwrap();
                write_request(&mut connection, 1, &Request::Hello).unwrap();
                assert_eq!(read_response(&mut connection).unwrap(), (1, Response::Acknowledged));
            }
            "other" => {
                let path = PathBuf::from(std::env::var_os("SEISMOGRAPH_TEST_ENDPOINT").unwrap());
                let error = std::os::unix::net::UnixStream::connect(path).unwrap_err();
                assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
                assert_eq!(fs::read_dir(directory).unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
            }
            mode => panic!("unexpected fixture mode {mode}"),
        }
    }
}
