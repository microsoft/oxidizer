// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph_protocol::io::{IoOperation, MonitorIoControl};

use super::*;

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
    run_listener(
        &io,
        &listener,
        &descriptor,
        &Arc::new(AtomicBool::new(false)),
        &Arc::new(ActiveClients::new()),
        &error,
    );
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
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    assert!(clients.register(&server).unwrap().is_none());
    assert!(clients.state.lock().unwrap().streams.is_empty());
    assert!(control.operations().contains(&IoOperation::Close));
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
