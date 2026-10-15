// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public Arty-owned monitoring configuration, discovery, lifecycle and error contracts.

#![cfg(all(feature = "rt", feature = "seismograph"))]

use std::io::Write as _;
use std::net::{SocketAddr, TcpStream};
use std::sync::Mutex;

use arty::runtime::{MonitorOptions, Runtime, WorkersPolicy};
use seismograph_protocol::message::{Request, Response};
use seismograph_protocol::monitor::MonitorDescriptor;
use seismograph_protocol::{read_response, write_request};

testing_aids::init_tracing!();
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn name(scenario: &str) -> String {
    format!("arty-owned-{scenario}-{}", std::process::id())
}

#[expect(clippy::unwrap_used, reason = "test discovery fixtures must decode successfully")]
#[expect(clippy::panic, reason = "unexpected OS discovery failures invalidate the native test fixture")]
fn descriptors(name: &str) -> Vec<MonitorDescriptor> {
    let directory = seismograph_protocol::monitor_directory().unwrap();
    match std::fs::read_dir(&directory) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&format!("{}-", std::process::id()))
            })
            .map(|path| MonitorDescriptor::read_file(path).unwrap())
            .filter(|descriptor| descriptor.name == name)
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("monitor discovery failed: {error}"),
    }
}

#[expect(clippy::unwrap_used, reason = "test transport fixtures assert successful authenticated setup")]
fn connect(descriptor: &MonitorDescriptor) -> TcpStream {
    let address = SocketAddr::from(descriptor.socket_address());
    let mut connection = TcpStream::connect_timeout(&address, testing_aids::TEST_TIMEOUT).unwrap();
    connection.set_read_timeout(Some(testing_aids::TEST_TIMEOUT)).unwrap();
    connection.set_write_timeout(Some(testing_aids::TEST_TIMEOUT)).unwrap();
    write_request(
        &mut connection,
        1,
        &Request::Hello {
            authentication: descriptor.authentication,
        },
    )
    .unwrap();
    assert!(matches!(read_response(&mut connection).unwrap().1, Response::Hello { .. }));
    connection
}

#[test]
#[cfg_attr(
    miri,
    ignore = "native localhost transport; executor and registration ownership remain covered by Arty/arty_executor Miri tests"
)]
fn configuration_is_deferred_and_each_runtime_owns_its_endpoint() {
    let _test = TEST_LOCK.lock().unwrap();
    let first_name = name("first");
    let second_name = name("second");
    let builder = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .monitor(MonitorOptions::default().name(&first_name).instance("a"));
    assert!(descriptors(&first_name).is_empty());
    let first = builder.build().unwrap();
    let second = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .monitor(MonitorOptions::default().name(&second_name).instance("b"))
        .build()
        .unwrap();
    let first_endpoint = descriptors(&first_name).pop().unwrap();
    let second_endpoint = descriptors(&second_name).pop().unwrap();
    assert_ne!(first_endpoint.port, second_endpoint.port);
    assert_ne!(first_endpoint.instance_id, second_endpoint.instance_id);
    assert_eq!(first_endpoint.instance.as_deref(), Some("a"));
    drop(connect(&first_endpoint));
    drop(connect(&second_endpoint));
    first.stop().unwrap();
    assert!(descriptors(&first_name).is_empty());
    assert_eq!(second.scheduler().block_on(async |_| 42).unwrap(), 42);
    drop(connect(&second_endpoint));
    drop(second);
    assert!(descriptors(&second_name).is_empty());
}

#[test]
#[cfg_attr(miri, ignore = "native localhost transport; unsafe worker teardown remains covered separately")]
fn startup_errors_are_returned_and_failed_runtime_builds_retire_the_endpoint() {
    let _test = TEST_LOCK.lock().unwrap();
    Runtime::builder().monitor(MonitorOptions::default().name("")).build().unwrap_err();
    Runtime::builder()
        .monitor(MonitorOptions::default().instance(""))
        .build()
        .unwrap_err();
    let endpoint_name = name("failed-build");
    Runtime::builder()
        .workers(WorkersPolicy::exactly(0))
        .monitor(MonitorOptions::default().name(&endpoint_name))
        .build()
        .unwrap_err();
    assert!(descriptors(&endpoint_name).is_empty());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "native localhost transport; recorder policy behavior remains covered in seismograph"
)]
fn opt_in_does_not_reset_or_enable_process_recording_policies() {
    use seismograph::recorder::event::EventClass;
    use seismograph::recorder::{Configuration, RecordingPolicy};

    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            seismograph::recorder(Configuration::default());
        }
    }
    let _test = TEST_LOCK.lock().unwrap();
    let _reset = Reset;
    for enabled in [false, true] {
        seismograph::recorder(Configuration {
            runtime_tasks: if enabled {
                RecordingPolicy::all(false)
            } else {
                RecordingPolicy::default()
            },
            ..Configuration::default()
        });
        let runtime = Runtime::builder()
            .workers(WorkersPolicy::exactly(1))
            .monitor(MonitorOptions::default().name(name("policies")))
            .build()
            .unwrap();
        assert_eq!(seismograph::recorder::recording_enabled_for(EventClass::RuntimeTask), enabled);
        runtime.stop().unwrap();
        assert_eq!(seismograph::recorder::recording_enabled_for(EventClass::RuntimeTask), enabled);
    }
}

#[test]
#[cfg_attr(miri, ignore = "native localhost transport; observed event reporting is tested natively")]
fn background_connection_errors_use_the_configured_runtime_sink() {
    use std::time::Instant;

    use observed_testing::{TEST_ID, test_emitter};

    let _test = TEST_LOCK.lock().unwrap();
    let (sink, processor) = test_emitter(TEST_ID);
    let endpoint_name = name("reported-error");
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .sink(sink)
        .monitor(MonitorOptions::default().name(&endpoint_name))
        .build()
        .unwrap();
    let endpoint = descriptors(&endpoint_name).pop().unwrap();
    let mut connection = TcpStream::connect(endpoint.socket_address()).unwrap();
    connection.write_all(b"invalid-frame").unwrap();
    drop(connection);
    let deadline = Instant::now() + testing_aids::TEST_TIMEOUT;
    while !processor.events().iter().any(|event| event.name() == "arty.rt.monitor.failed") {
        assert!(Instant::now() < deadline, "background monitor errors must be reported");
        std::thread::yield_now();
    }
    assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
    runtime.stop().unwrap();
}
