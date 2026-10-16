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
}
