// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph_protocol::io::{IoOperation, MonitorIoControl};

use super::*;

#[test]
fn gamma_round2_listener_join_diagnostic_distinguishes_success_from_panic() {
    assert!(!listener_join_failed(&Ok(())));
    assert!(listener_join_failed(&Err(Box::new("round2 synthetic panic"))));
}

#[test]
fn gamma_round2_timeout_setup_and_response_writer_store_the_exact_frozen_timeout() {
    let (io, control) = MonitorIo::new_mocked();
    let (mut server, _peer) = control.connected_pair();
    configure_client_stream(&server).unwrap();
    assert!(format!("{server:?}").contains("write_timeout: Mutex { data: Some(5s)"));
    let writer = DeadlineWriter::new(&io, &mut server, Duration::from_secs(11));
    control.advance_time(Duration::from_secs(4));
    writer.apply_remaining_timeout().unwrap();
    assert!(format!("{:?}", writer.stream).contains("write_timeout: Mutex { data: Some(7s)"));
}

#[test]
fn gamma_round2_stop_response_holds_snapshot_exclusion_until_response_drop() {
    let _test = crate::recorder::TEST_LOCK.lock().unwrap();
    let response = connected_response_with_guard(&Request::CaptureSnapshotAndStop);
    assert!(SNAPSHOT_IN_PROGRESS.load(Ordering::Acquire));
    assert!(SnapshotRequestGuard::acquire().is_err());
    drop(response);
    assert!(SnapshotRequestGuard::acquire().is_ok());
}

fn round2_descriptor(io: &MonitorIo) -> MonitorDescriptor {
    let directory = Directory::current(io).unwrap();
    let owner = io.current_owner().unwrap();
    let instance_id = io.instance_id().unwrap();
    MonitorDescriptor {
        name: "round2".into(),
        instance: None,
        process_id: io.process_id(),
        endpoint: MonitorEndpoint::for_instance(directory.path(), &owner, io.process_id(), instance_id),
        owner,
        instance_id,
    }
}

#[test]
fn gamma_round2_client_thread_name_is_observed_at_its_injected_failure() {
    use std::cell::RefCell;
    use std::sync::mpsc;

    thread_local! {
        static EXIT: RefCell<Option<mpsc::Sender<()>>> = const { RefCell::new(None) };
    }

    #[derive(Debug)]
    struct NamedFailure {
        name: mpsc::Sender<Option<String>>,
        exited: Mutex<Option<mpsc::Sender<()>>>,
    }

    impl fmt::Display for NamedFailure {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let exited = self.exited.lock().unwrap().take();
            if let Some(exited) = exited {
                EXIT.with(|value| *value.borrow_mut() = Some(exited));
            }
            self.name.send(thread::current().name().map(str::to_owned)).unwrap();
            f.write_str("round2 named client refused")
        }
    }

    impl std::error::Error for NamedFailure {}

    let (io, control) = MonitorIo::new_mocked();
    let descriptor = round2_descriptor(&io);
    let (_peer, server) = control.connected_pair();
    let clients = Arc::new(ActiveClients::new());
    let registration = clients.register(&server).unwrap().unwrap();
    let error = Arc::new(Mutex::new(None));
    let (name, names) = mpsc::channel();
    let (exited, completed) = mpsc::channel();
    control.fail_next(
        IoOperation::WriteTimeout,
        io::Error::other(NamedFailure {
            name,
            exited: Mutex::new(Some(exited)),
        }),
    );
    spawn_client_thread(io, server, descriptor, Arc::new(AtomicBool::new(false)), registration, &error);
    assert_eq!(names.recv().unwrap().as_deref(), Some("seismograph-monitor-client"));
    // The sender lives in the worker's TLS, so disconnection observes its exit.
    assert!(completed.recv().is_err());
    assert!(clients.state.lock().unwrap().streams.is_empty());
    assert_eq!(error.lock().unwrap().as_deref(), Some("round2 named client refused"));
}

#[test]
fn gamma_round2_listener_retains_permission_denied_before_the_next_accept() {
    #[derive(Debug)]
    struct StopOnDisplay(Arc<AtomicBool>);

    impl fmt::Display for StopOnDisplay {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.store(true, Ordering::Release);
            f.write_str("round2 permission refused")
        }
    }

    impl std::error::Error for StopOnDisplay {}

    let (io, control) = MonitorIo::new_mocked();
    let descriptor = round2_descriptor(&io);
    let listener = Listener::bind(&io, &descriptor).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let error = Arc::new(Mutex::new(None));
    control.fail_next(
        IoOperation::Accept,
        io::Error::new(io::ErrorKind::PermissionDenied, StopOnDisplay(Arc::clone(&stop))),
    );
    control.fail_next(IoOperation::Accept, io::Error::other("round2 unexpected second accept"));
    run_listener(&io, &listener, &descriptor, &stop, &Arc::new(ActiveClients::new()), &error);
    assert_eq!(error.lock().unwrap().as_deref(), Some("round2 permission refused"));
    assert_eq!(
        control
            .operations()
            .iter()
            .filter(|operation| **operation == IoOperation::Accept)
            .count(),
        1
    );
}

#[test]
fn gamma_round2_listener_continues_after_rejected_or_untrackable_clients() {
    #[derive(Debug)]
    struct NextAcceptFailure(MonitorIoControl);

    impl fmt::Display for NextAcceptFailure {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("round2 client registration refused")
        }
    }

    impl std::error::Error for NextAcceptFailure {}

    impl Drop for NextAcceptFailure {
        fn drop(&mut self) {
            self.0.fail_next(IoOperation::Accept, io::Error::other("round2 terminal accept"));
        }
    }

    for rejected in [false, true] {
        let (io, control) = MonitorIo::new_mocked();
        let descriptor = round2_descriptor(&io);
        let listener = Listener::bind(&io, &descriptor).unwrap();
        let _peer = Connection::connect(&io, &descriptor, Duration::from_secs(5)).unwrap();
        let clients = Arc::new(ActiveClients::new());
        clients.state.lock().unwrap().accepting = !rejected;
        control.fail_next(
            if rejected { IoOperation::Close } else { IoOperation::Duplicate },
            io::Error::other(NextAcceptFailure(control.clone())),
        );
        let error = Arc::new(Mutex::new(None));
        run_listener(&io, &listener, &descriptor, &Arc::new(AtomicBool::new(false)), &clients, &error);
        assert_eq!(error.lock().unwrap().as_deref(), Some("round2 terminal accept"));
        assert_eq!(
            control
                .operations()
                .iter()
                .filter(|operation| **operation == IoOperation::Accept)
                .count(),
            2
        );
        assert!(clients.state.lock().unwrap().streams.is_empty());
    }
}

#[test]
fn gamma_round2_listener_retries_transient_accepts_without_advancing_the_frozen_clock() {
    let (io, control) = MonitorIo::new_mocked();
    let descriptor = round2_descriptor(&io);
    let listener = Listener::bind(&io, &descriptor).unwrap();
    let instant = io.instant();
    for kind in [io::ErrorKind::WouldBlock, io::ErrorKind::Interrupted, io::ErrorKind::InvalidInput] {
        control.fail_next(IoOperation::Accept, kind.into());
    }
    run_listener(
        &io,
        &listener,
        &descriptor,
        &Arc::new(AtomicBool::new(false)),
        &Arc::new(ActiveClients::new()),
        &Arc::new(Mutex::new(None)),
    );
    assert_eq!(
        control
            .operations()
            .iter()
            .filter(|operation| **operation == IoOperation::Wait)
            .count(),
        2
    );
    assert_eq!(io.instant(), instant);
}

#[test]
fn gamma_round2_listener_thread_keeps_its_name_and_startup_keeps_the_join_handle() {
    let (io, _) = MonitorIo::new_mocked();
    let monitor = Monitor::builder()
        .io(io)
        .start_with(|io, listener, descriptor, stop, clients, error, path| {
            stop.store(true, Ordering::Release);
            spawn_listener_thread(io, listener, descriptor, stop, clients, error, path)
        })
        .unwrap();
    let ListenerThread::Native(listener) = monitor.thread.as_ref().unwrap() else {
        panic!("startup must retain the native listener thread");
    };
    assert_eq!(listener.thread().name(), Some("seismograph-monitor"));
}

#[test]
fn gamma_round2_stopped_client_does_not_read_a_prequeued_invalid_second_frame() {
    use std::io::{Read as _, Write as _};

    let (io, control) = MonitorIo::new_mocked();
    let directory = Directory::current(&io).unwrap();
    let owner = io.current_owner().unwrap();
    let instance_id = io.instance_id().unwrap();
    let descriptor = MonitorDescriptor {
        name: "round2".into(),
        instance: None,
        process_id: io.process_id(),
        endpoint: MonitorEndpoint::for_instance(directory.path(), &owner, io.process_id(), instance_id),
        owner,
        instance_id,
    };
    let (mut peer, server) = control.connected_pair();
    seismograph_protocol::write_request(&mut peer, 13, &Request::Hello).unwrap();
    peer.write_all(&[0; 20]).unwrap();
    handle_client_with_timeouts(
        &io,
        server,
        &descriptor,
        &AtomicBool::new(true),
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(
        control
            .operations()
            .iter()
            .filter(|operation| **operation == IoOperation::Read)
            .count(),
        1
    );
    assert!(matches!(
        seismograph_protocol::read_response(&mut peer).unwrap(),
        (13, Response::Hello { .. })
    ));
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}

#[test]
fn gamma_round2_startup_preserves_current_directory_failure() {
    let (io, control) = MonitorIo::new_mocked();
    control.fail_next(IoOperation::CreateDirectory, io::Error::other("round2 directory refused"));
    let error = Monitor::builder()
        .io(io)
        .start_with(|_, _, _, _, _, _, _| panic!("directory failure must precede spawning"))
        .unwrap_err();
    assert!(
        matches!(error, StartupFailure::Protocol(seismograph_protocol::Error::Io(ref source)) if source.to_string() == "round2 directory refused")
    );
}

#[test]
fn gamma_round2_client_returns_timeout_setup_and_handshake_write_failures() {
    for operation in [IoOperation::WriteTimeout, IoOperation::Write] {
        let (io, control) = MonitorIo::new_mocked();
        let directory = Directory::current(&io).unwrap();
        let owner = io.current_owner().unwrap();
        let instance_id = io.instance_id().unwrap();
        let descriptor = MonitorDescriptor {
            name: "round2".into(),
            instance: None,
            process_id: io.process_id(),
            endpoint: MonitorEndpoint::for_instance(directory.path(), &owner, io.process_id(), instance_id),
            owner,
            instance_id,
        };
        let (mut peer, server) = control.connected_pair();
        seismograph_protocol::write_request(&mut peer, 13, &Request::Hello).unwrap();
        control.fail_next(operation, io::Error::other("round2 client refused"));
        let error = handle_client_with_timeouts(
            &io,
            server,
            &descriptor,
            &AtomicBool::new(false),
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "round2 client refused");
        assert!(matches!(
            error,
            ClientError::Io(_) | ClientError::Protocol(seismograph_protocol::Error::Io(_))
        ));
    }
}

#[test]
fn gamma_round2_connected_response_write_failure_is_returned() {
    let (io, control) = MonitorIo::new_mocked();
    let directory = Directory::current(&io).unwrap();
    let owner = io.current_owner().unwrap();
    let instance_id = io.instance_id().unwrap();
    let descriptor = MonitorDescriptor {
        name: "round2".into(),
        instance: None,
        process_id: io.process_id(),
        endpoint: MonitorEndpoint::for_instance(directory.path(), &owner, io.process_id(), instance_id),
        owner,
        instance_id,
    };
    let (mut peer, server) = control.connected_pair();
    seismograph_protocol::write_request(&mut peer, 13, &Request::Hello).unwrap();
    let worker = thread::spawn(move || {
        handle_client_with_timeouts(
            &io,
            server,
            &descriptor,
            &AtomicBool::new(false),
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
    });
    assert!(matches!(
        seismograph_protocol::read_response(&mut peer).unwrap(),
        (13, Response::Hello { .. })
    ));
    control.fail_next(IoOperation::WriteTimeout, io::Error::other("round2 response refused"));
    seismograph_protocol::write_request(&mut peer, 17, &Request::ReadServerVersion).unwrap();
    drop(peer);
    let error = worker.join().unwrap().unwrap_err();
    assert_eq!(error.to_string(), "round2 response refused");
}

#[test]
fn gamma_survivor_monitor_debug_names_descriptor_fields_exactly() {
    let (io, _) = MonitorIo::new_mocked();
    let directory = Directory::current(&io).unwrap();
    let owner = io.current_owner().unwrap();
    let instance_id = io.instance_id().unwrap();
    let descriptor = MonitorDescriptor {
        name: "gamma-worker".into(),
        instance: Some("gamma-instance".into()),
        process_id: io.process_id(),
        endpoint: MonitorEndpoint::for_instance(directory.path(), &owner, io.process_id(), instance_id),
        owner,
        instance_id,
    };
    let path = directory.path().join(descriptor.file_name());
    let expected = format!("Monitor {{ descriptor: {descriptor:?}, descriptor_path: {path:?}, .. }}");
    let monitor = Monitor {
        descriptor,
        descriptor_path: path,
        io,
        stop: Arc::new(AtomicBool::new(false)),
        active_clients: Arc::new(ActiveClients::new()),
        last_error: Arc::new(Mutex::new(None)),
        thread: None,
        _directory: directory,
        _fixture: None,
    };
    assert_eq!(format!("{monitor:?}"), expected);
}

#[test]
fn gamma_survivor_monitor_client_ids_increment_and_reject_exhaustion() {
    let (_, control) = MonitorIo::new_mocked();
    let (_peer, server) = control.connected_pair();
    let clients = Arc::new(ActiveClients::new());
    let first = clients.register(&server).unwrap().unwrap();
    let second = clients.register(&server).unwrap().unwrap();
    assert_eq!((first.id, second.id), (0, 1));
    drop(first);
    drop(second);
    clients.state.lock().unwrap().next_id = u64::MAX;
    let error = clients.register(&server).err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert_eq!(error.to_string(), "monitor client identifier space exhausted");
    assert!(clients.state.lock().unwrap().streams.is_empty());
}

#[test]
fn gamma_survivor_monitor_deadlines_share_frozen_clock_and_exact_errors() {
    use std::io::{Read as _, Write as _};
    let (io, control) = MonitorIo::new_mocked();
    let (mut peer, mut server) = control.connected_pair();
    peer.write_all(b"abc").unwrap();
    let mut reader = DeadlineReader {
        io: io.clone(),
        stream: &mut server,
        deadline: io.instant() + Duration::from_secs(5),
    };
    let mut buffer = [0; 2];
    assert_eq!(reader.read(&mut buffer).unwrap(), 2);
    assert_eq!(buffer, *b"ab");
    control.advance_time(Duration::from_secs(5));
    let error = reader.read(&mut buffer).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert_eq!(error.to_string(), "monitor frame read timed out");
    let mut writer = DeadlineWriter::new(&io, &mut server, Duration::from_secs(5));
    assert_eq!(writer.write(b"reply").unwrap(), 5);
    control.advance_time(Duration::from_secs(5));
    for error in [writer.write(b"late").unwrap_err(), writer.flush().unwrap_err()] {
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(error.to_string(), "Seismograph monitor response write timed out");
    }
    let error = read_request_with_timeout(&io, &mut server, &AtomicBool::new(false), Duration::MAX, ReadTimeout::Handshake).unwrap_err();
    assert!(matches!(error, ClientError::Io(ref error) if error.kind() == io::ErrorKind::InvalidInput));
    assert_eq!(error.to_string(), "monitor read timeout is too large");
}

#[test]
fn gamma_survivor_monitor_reader_propagates_mock_deadline_configuration_error() {
    use std::io::Read as _;
    let (io, control) = MonitorIo::new_mocked();
    let (_peer, mut server) = control.connected_pair();
    let mut reader = DeadlineReader {
        io: io.clone(),
        stream: &mut server,
        deadline: io.instant() + Duration::from_secs(5),
    };
    control.fail_next(
        IoOperation::ReadTimeout,
        io::Error::new(io::ErrorKind::PermissionDenied, "gamma timeout refused"),
    );
    let error = reader.read(&mut [0]).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(error.to_string(), "gamma timeout refused");
}

fn wait_for_blocked(control: &MonitorIoControl) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !control.operations().contains(&IoOperation::Wait) {
        assert!(Instant::now() < deadline, "mock transport did not block");
        thread::yield_now();
    }
}

#[test]
fn request_deadlines_use_the_injected_clock_for_handshake_and_idle_timeouts() {
    for timeout_kind in [ReadTimeout::Handshake, ReadTimeout::Idle] {
        let (io, control) = MonitorIo::new_mocked();
        let (_client, mut server) = control.connected_pair();
        let worker = thread::spawn(move || {
            read_request_with_timeout(&io, &mut server, &AtomicBool::new(false), Duration::from_secs(5), timeout_kind)
        });
        wait_for_blocked(&control);
        control.advance_time(Duration::from_secs(5));
        let error = worker.join().unwrap().unwrap_err();
        assert!(matches!(
            (timeout_kind, error),
            (ReadTimeout::Handshake, ClientError::HandshakeTimedOut) | (ReadTimeout::Idle, ClientError::IdleTimedOut)
        ));
    }
}

#[test]
fn response_deadline_does_not_restart_after_partial_writes() {
    use std::io::Write as _;

    let (io, control) = MonitorIo::new_mocked();
    control.set_max_chunk(2);
    let (_client, mut server) = control.connected_pair();
    let mut writer = DeadlineWriter::new(&io, &mut server, Duration::from_secs(5));
    assert_eq!(writer.write(b"hello").unwrap(), 2);
    control.advance_time(Duration::from_secs(5));
    assert_eq!(writer.write(b"remaining").unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert_eq!(writer.flush().unwrap_err().kind(), io::ErrorKind::TimedOut);
}

#[test]
fn listener_reports_fatal_accept_errors_without_using_native_io() {
    let (io, control) = MonitorIo::new_mocked();
    let directory = Directory::current(&io).unwrap();
    let owner = io.current_owner().unwrap();
    let instance_id = io.instance_id().unwrap();
    let descriptor = MonitorDescriptor {
        name: "worker".into(),
        instance: None,
        process_id: io.process_id(),
        endpoint: MonitorEndpoint::for_instance(directory.path(), &owner, io.process_id(), instance_id),
        owner,
        instance_id,
    };
    let listener = Listener::bind(&io, &descriptor).unwrap();
    let error = Arc::new(Mutex::new(None));
    control.fail_next(IoOperation::Accept, io::ErrorKind::PermissionDenied.into());
    control.fail_next(IoOperation::Accept, io::Error::other("accept failed"));
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker_error = Arc::clone(&error);
    let (completed, completion) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        run_listener(
            &io,
            &listener,
            &descriptor,
            &worker_stop,
            &Arc::new(ActiveClients::new()),
            &worker_error,
        );
        completed.send(()).unwrap();
    });
    let result = completion.recv_timeout(Duration::from_secs(5));
    stop.store(true, Ordering::Release);
    result.unwrap();
    worker.join().unwrap();
    assert_eq!(error.lock().unwrap().as_deref(), Some("accept failed"));
    assert_eq!(
        control
            .operations()
            .iter()
            .filter(|operation| **operation == IoOperation::Accept)
            .count(),
        2
    );
}

#[test]
fn isolated_selection_discards_an_injected_backend_and_io_selection_discards_isolation() {
    let (io, _) = MonitorIo::new_mocked();
    let isolated = Monitor::builder().io(io.clone()).isolated();
    assert_eq!((isolated.isolated, isolated.io.is_none()), (true, true));
    let injected = isolated.io(io);
    assert_eq!((injected.isolated, injected.io.is_some()), (false, true));
}

#[test]
fn teardown_continues_after_a_listener_panic_and_reports_failed_descriptor_cleanup() {
    let (io, control) = MonitorIo::new_mocked();
    let monitor = Monitor::builder()
        .io(io.clone())
        .name("teardown-failure")
        .start_with(|_, _, _, _, _, _, _| Ok(thread::spawn(|| panic!("injected listener panic"))))
        .unwrap();
    let stop = Arc::clone(&monitor.stop);
    let path = monitor.descriptor_path.clone();
    control.fail_next(IoOperation::RemoveDescriptor, io::Error::other("cleanup refused"));
    drop(monitor);
    assert!(stop.load(Ordering::Acquire));
    assert!(control.file_bytes(&path).is_some());
    io.remove_descriptor(&path).unwrap();
    assert_eq!(control.file_bytes(&path), None);
}

#[test]
fn failed_startup_preserves_the_spawn_error_when_cleanup_also_fails() {
    let (io, control) = MonitorIo::new_mocked();
    control.fail_next(IoOperation::RemoveDescriptor, io::Error::other("cleanup refused"));
    let error = Monitor::builder()
        .io(io.clone())
        .name("startup-failure")
        .start_with(|_, _, _, _, _, _, _| Err(StartupFailure::Spawn(io::Error::other("spawn refused"))))
        .map_err(Error::monitor)
        .unwrap_err();
    assert_eq!(error.to_string(), "failed to start Seismograph monitor thread: spawn refused");
    let directory = Directory::current(&io).unwrap();
    let descriptor = directory.descriptors().unwrap().next().unwrap().unwrap();
    io.remove_descriptor(&directory.path().join(descriptor.file_name())).unwrap();
}

#[test]
fn shutdown_still_cancels_clients_when_a_close_reports_an_error() {
    use std::io::Read as _;

    let (_io, control) = MonitorIo::new_mocked();
    let (mut peer, server) = control.connected_pair();
    let clients = Arc::new(ActiveClients::new());
    let _registration = clients.register(&server).unwrap().unwrap();
    control.fail_next(IoOperation::Close, io::Error::other("close refused"));
    clients.shutdown_all();
    assert!(clients.register(&server).unwrap().is_none());
    assert!(clients.state.lock().unwrap().streams.is_empty());
    assert!(control.operations().contains(&IoOperation::Close));
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}

#[test]
fn listener_reports_registration_and_rejection_close_failures() {
    for operation in [IoOperation::Duplicate, IoOperation::Close] {
        let (io, control) = MonitorIo::new_mocked();
        let directory = Directory::current(&io).unwrap();
        let owner = io.current_owner().unwrap();
        let instance_id = io.instance_id().unwrap();
        let descriptor = MonitorDescriptor {
            name: "worker".into(),
            instance: None,
            process_id: io.process_id(),
            endpoint: MonitorEndpoint::for_instance(directory.path(), &owner, io.process_id(), instance_id),
            owner,
            instance_id,
        };
        let listener = Listener::bind(&io, &descriptor).unwrap();
        listener.set_nonblocking(true).unwrap();
        let client = Connection::connect(&io, &descriptor, Duration::from_secs(1)).unwrap();
        let clients = Arc::new(ActiveClients::new());
        clients.state.lock().unwrap().accepting = operation != IoOperation::Close;
        let error = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        control.fail_next(operation, io::Error::other("client tracking failed"));
        let (completed, completion) = std::sync::mpsc::channel();
        let (worker_error, worker_stop, worker_clients) = (Arc::clone(&error), Arc::clone(&stop), Arc::clone(&clients));
        let worker = thread::spawn(move || {
            run_listener(&io, &listener, &descriptor, &worker_stop, &worker_clients, &worker_error);
            completed.send(()).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while error.lock().unwrap().is_none() {
            assert!(Instant::now() < deadline, "listener did not report {operation:?}");
            thread::yield_now();
        }
        stop.store(true, Ordering::Release);
        completion.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        assert_eq!(error.lock().unwrap().as_deref(), Some("client tracking failed"));
        clients.shutdown_all();
        drop(client);
    }
}
