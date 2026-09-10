// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use seismograph_protocol::message::{RecorderStatistics, RecordingConfiguration, RecordingPolicy, Request, Response, SnapshotOptions};
use seismograph_protocol::monitor::MonitorDescriptor;
use seismograph_protocol::{read_response, write_request};

use super::Error;
use super::app::Instance;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(test)]
thread_local! {
    static TEST_MONITOR_DIRECTORY: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn discover() -> Result<Vec<Instance>, Error> {
    #[cfg(test)]
    if let Some(directory) = TEST_MONITOR_DIRECTORY.with(|directory| directory.borrow().clone()) {
        return discover_in(&directory);
    }
    let directory = seismograph_protocol::monitor_directory().map_err(Error::Protocol)?;
    discover_in(&directory)
}

fn discover_in(directory: &std::path::Path) -> Result<Vec<Instance>, Error> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(Error::Io(error)),
    };
    let mut instances = Vec::new();
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|extension| extension.to_str()) != Some("monitor") {
            continue;
        }
        let Ok(descriptor) = MonitorDescriptor::read_file(entry.path()) else {
            continue;
        };
        if let Ok(recording) = handshake(&descriptor) {
            instances.push(Instance { descriptor, recording });
        }
    }
    instances.sort_by(|left, right| {
        (&left.descriptor.name, &left.descriptor.instance, left.descriptor.process_id).cmp(&(
            &right.descriptor.name,
            &right.descriptor.instance,
            right.descriptor.process_id,
        ))
    });
    Ok(instances)
}

#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn set_recording(descriptor: &MonitorDescriptor, configuration: RecordingConfiguration) -> Result<(), Error> {
    let cache_supported = cache_recording(descriptor)?.is_some();
    if !cache_supported && configuration.cache != RecordingPolicy::default() {
        return Err(Error::Remote("cache recording is not supported by this monitor".into()));
    }
    let legacy_configuration = RecordingConfiguration {
        cache: RecordingPolicy::default(),
        ..configuration
    };
    match command(descriptor, &Request::SetRecording(legacy_configuration))? {
        Response::Acknowledged => {}
        _ => return Err(Error::UnexpectedResponse),
    }
    if cache_supported {
        match command(descriptor, &Request::SetCacheRecording(configuration.cache))? {
            Response::Acknowledged => {}
            _ => return Err(Error::UnexpectedResponse),
        }
    }
    Ok(())
}

#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn capture_snapshot(descriptor: &MonitorDescriptor, options: SnapshotOptions) -> Result<Vec<u8>, Error> {
    match command(descriptor, &Request::CaptureSnapshot(options))? {
        Response::Snapshot(bytes) => Ok(bytes),
        _ => Err(Error::UnexpectedResponse),
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn recorder_statistics(descriptor: &MonitorDescriptor) -> Result<RecorderStatistics, Error> {
    match command(descriptor, &Request::ReadRecorderStatistics)? {
        Response::RecorderStatistics(statistics) => Ok(statistics),
        _ => Err(Error::UnexpectedResponse),
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn save_snapshot(descriptor: &MonitorDescriptor, bytes: &[u8]) -> Result<PathBuf, Error> {
    let directory = std::env::current_dir().map_err(Error::Io)?;
    save_snapshot_to(&directory, descriptor, bytes)
}

fn save_snapshot_to(directory: &std::path::Path, descriptor: &MonitorDescriptor, bytes: &[u8]) -> Result<PathBuf, Error> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::Clock(error.to_string()))?
        .as_nanos();
    let name = sanitize(&descriptor.name);
    let path = directory.join(format!("{name}-{}-{timestamp}.seismograph", descriptor.process_id));
    let mut file = fs::File::create(&path).map_err(Error::Io)?;
    file.write_all(bytes).map_err(Error::Io)?;
    Ok(path)
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn handshake(descriptor: &MonitorDescriptor) -> Result<RecordingConfiguration, Error> {
    let mut stream = connect(descriptor)?;
    write_request(
        &mut stream,
        1,
        &Request::Hello {
            authentication: descriptor.authentication,
        },
    )
    .map_err(Error::Protocol)?;
    match read_response(&mut stream).map_err(Error::Protocol)? {
        (
            1,
            Response::Hello {
                instance_id,
                mut recording,
            },
        ) if instance_id == descriptor.instance_id => {
            write_request(&mut stream, 2, &Request::ReadCacheRecording).map_err(Error::Protocol)?;
            if let Ok((2, Response::CacheRecording(cache))) = read_response(&mut stream) {
                recording.cache = cache;
            }
            Ok(recording)
        }
        (_, Response::Error(message)) => Err(Error::Remote(message)),
        _ => Err(Error::UnexpectedResponse),
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn cache_recording(descriptor: &MonitorDescriptor) -> Result<Option<seismograph_protocol::message::RecordingPolicy>, Error> {
    let mut stream = connect(descriptor)?;
    write_request(
        &mut stream,
        1,
        &Request::Hello {
            authentication: descriptor.authentication,
        },
    )
    .map_err(Error::Protocol)?;
    match read_response(&mut stream).map_err(Error::Protocol)? {
        (1, Response::Hello { instance_id, .. }) if instance_id == descriptor.instance_id => {}
        (_, Response::Error(message)) => return Err(Error::Remote(message)),
        _ => return Err(Error::UnexpectedResponse),
    }
    write_request(&mut stream, 2, &Request::ReadCacheRecording).map_err(Error::Protocol)?;
    Ok(match read_response(&mut stream) {
        Ok((2, Response::CacheRecording(policy))) => Some(policy),
        _ => None,
    })
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn connect(descriptor: &MonitorDescriptor) -> Result<TcpStream, Error> {
    let address = SocketAddr::V4(descriptor.socket_address());
    let stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(Error::Io)?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(Error::Io)?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(Error::Io)?;
    Ok(stream)
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn command(descriptor: &MonitorDescriptor, request: &Request) -> Result<Response, Error> {
    let mut stream = connect(descriptor)?;
    write_request(
        &mut stream,
        1,
        &Request::Hello {
            authentication: descriptor.authentication,
        },
    )
    .map_err(Error::Protocol)?;
    match read_response(&mut stream).map_err(Error::Protocol)? {
        (1, Response::Hello { instance_id, .. }) if instance_id == descriptor.instance_id => {}
        (_, Response::Error(message)) => return Err(Error::Remote(message)),
        _ => return Err(Error::UnexpectedResponse),
    }
    write_request(&mut stream, 2, request).map_err(Error::Protocol)?;
    let (request_id, response) = read_response(&mut stream).map_err(Error::Protocol)?;
    if request_id != 2 {
        return Err(Error::UnexpectedResponse);
    }
    match response {
        Response::Error(message) => Err(Error::Remote(message)),
        response => Ok(response),
    }
}

fn sanitize(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    if value.is_empty() { "seismograph".into() } else { value }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::net::{Shutdown, TcpListener};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    use seismograph_protocol::message::EventBufferDisposition;
    use seismograph_protocol::monitor::{AuthenticationToken, InstanceId};
    use seismograph_protocol::{read_request, write_response};

    use super::*;

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);
    type ScriptedServer = (MonitorDescriptor, mpsc::Receiver<Vec<(u64, Request)>>, thread::JoinHandle<()>);

    fn test_descriptor(port: u16) -> MonitorDescriptor {
        MonitorDescriptor {
            name: "worker west/europe".into(),
            instance: Some("test".into()),
            process_id: 42,
            instance_id: InstanceId::from_bytes([1; 16]),
            port,
            authentication: AuthenticationToken::from_bytes([2; 32]),
        }
    }

    fn directory(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join(format!(
            "client-unit-{name}-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // Defensive test-server failures are not product behavior.
    fn serve(conversations: Vec<Vec<(u64, Response)>>) -> ScriptedServer {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut descriptor = test_descriptor(listener.local_addr().unwrap().port());
        descriptor.name = "worker".into();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            for conversation in conversations {
                let deadline = Instant::now() + Duration::from_secs(2);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break Some(stream),
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break None,
                        Err(error) => panic!("test server accept failed: {error}"),
                    }
                };
                let Some(mut stream) = stream.take() else {
                    break;
                };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                for (response_id, response) in conversation {
                    let Ok(request) = read_request(&mut stream) else {
                        break;
                    };
                    requests.push(request);
                    write_response(&mut stream, response_id, &response).unwrap();
                }
                stream.shutdown(Shutdown::Write).unwrap();
                let mut trailing = Vec::new();
                stream.read_to_end(&mut trailing).unwrap();
                assert!(trailing.is_empty());
            }
            sender.send(requests).unwrap();
        });
        (descriptor, receiver, worker)
    }

    struct TestMonitorDirectory(PathBuf);

    impl TestMonitorDirectory {
        fn new(path: PathBuf) -> Self {
            TEST_MONITOR_DIRECTORY.with(|directory| *directory.borrow_mut() = Some(path.clone()));
            Self(path)
        }
    }

    impl Drop for TestMonitorDirectory {
        fn drop(&mut self) {
            TEST_MONITOR_DIRECTORY.with(|directory| *directory.borrow_mut() = None);
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn hello(descriptor: &MonitorDescriptor, recording: RecordingConfiguration) -> Response {
        Response::Hello {
            instance_id: descriptor.instance_id,
            recording,
        }
    }

    #[test]
    fn snapshot_file_names_replace_unsupported_characters() {
        assert_eq!(sanitize("worker west/europe"), "worker-west-europe");
    }

    #[test]
    fn empty_snapshot_file_name_uses_fallback() {
        assert_eq!(sanitize(""), "seismograph");
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires filesystem access and real TCP sockets")]
    fn discover_finds_a_published_monitor() {
        let name = format!(
            "client-discovery-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        );
        let directory = directory("discovery");
        fs::create_dir_all(&directory).unwrap();
        let _directory = TestMonitorDirectory::new(directory.clone());
        let (mut descriptor, requests, worker) = serve(vec![vec![
            (1, hello(&test_descriptor(0), RecordingConfiguration::default())),
            (2, Response::CacheRecording(RecordingPolicy::default())),
        ]]);
        descriptor.name.clone_from(&name);
        let descriptor_path = directory.join(descriptor.file_name());
        descriptor.write_file(&descriptor_path).unwrap();

        let instances = discover().unwrap();

        assert!(instances.iter().any(|instance| instance.descriptor.name == name));
        requests.recv().unwrap();
        worker.join().unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires real TCP sockets")]
    fn handshake_reads_legacy_and_cache_recording_state() {
        let mut recording = RecordingConfiguration::default();
        recording.allocations.enabled = true;
        let cache = RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            sampling_one_in: 8,
        };
        let (descriptor, requests, worker) = serve(vec![vec![
            (1, hello(&test_descriptor(0), recording)),
            (2, Response::CacheRecording(cache)),
        ]]);
        // The scripted response needs the listener's instance identity, not its port.
        let result = handshake(&descriptor).unwrap();
        let requests = requests.recv().unwrap();
        worker.join().unwrap();

        recording.cache = cache;
        assert_eq!(
            (result, requests),
            (
                recording,
                vec![
                    (
                        1,
                        Request::Hello {
                            authentication: descriptor.authentication,
                        },
                    ),
                    (2, Request::ReadCacheRecording),
                ],
            )
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires real TCP sockets")]
    fn handshake_rejects_remote_wrong_identity_and_unexpected_responses() {
        let cases = [
            Response::Error("denied".into()),
            Response::Hello {
                instance_id: InstanceId::from_bytes([9; 16]),
                recording: RecordingConfiguration::default(),
            },
            Response::Acknowledged,
        ];
        let actual = cases.map(|response| {
            let (descriptor, requests, worker) = serve(vec![vec![(1, response)]]);
            let error = handshake(&descriptor).unwrap_err().to_string();
            requests.recv().unwrap();
            worker.join().unwrap();
            error
        });

        assert_eq!(
            actual,
            [
                "monitor rejected the request: denied",
                "monitor returned an unexpected response",
                "monitor returned an unexpected response",
            ]
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires real TCP sockets")]
    fn cache_recording_distinguishes_supported_unsupported_and_rejected_monitors() {
        let policy = RecordingPolicy {
            enabled: true,
            capture_backtraces: false,
            sampling_one_in: 4,
        };
        let (descriptor, requests, worker) = serve(vec![vec![
            (1, hello(&test_descriptor(0), RecordingConfiguration::default())),
            (2, Response::CacheRecording(policy)),
        ]]);
        let supported = cache_recording(&descriptor).unwrap();
        requests.recv().unwrap();
        worker.join().unwrap();

        let (descriptor, requests, worker) = serve(vec![vec![
            (1, hello(&test_descriptor(0), RecordingConfiguration::default())),
            (3, Response::CacheRecording(policy)),
        ]]);
        let unsupported = cache_recording(&descriptor).unwrap();
        requests.recv().unwrap();
        worker.join().unwrap();

        let (descriptor, requests, worker) = serve(vec![vec![(1, Response::Error("denied".into()))]]);
        let rejected = cache_recording(&descriptor).unwrap_err().to_string();
        requests.recv().unwrap();
        worker.join().unwrap();

        let (descriptor, requests, worker) = serve(vec![vec![(
            1,
            Response::Hello {
                instance_id: InstanceId::from_bytes([9; 16]),
                recording: RecordingConfiguration::default(),
            },
        )]]);
        let wrong_identity = cache_recording(&descriptor).unwrap_err().to_string();
        requests.recv().unwrap();
        worker.join().unwrap();

        assert_eq!(
            (supported, unsupported, rejected, wrong_identity),
            (
                Some(policy),
                None,
                "monitor rejected the request: denied".to_owned(),
                "monitor returned an unexpected response".to_owned(),
            )
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires real TCP sockets")]
    fn set_recording_sends_legacy_and_cache_requests() {
        let mut configuration = RecordingConfiguration::default();
        configuration.allocations.enabled = true;
        configuration.cache = RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            sampling_one_in: 16,
        };
        let (descriptor, requests, worker) = serve(vec![
            vec![
                (1, hello(&test_descriptor(0), RecordingConfiguration::default())),
                (2, Response::CacheRecording(RecordingPolicy::default())),
            ],
            vec![
                (1, hello(&test_descriptor(0), RecordingConfiguration::default())),
                (2, Response::Acknowledged),
            ],
            vec![
                (1, hello(&test_descriptor(0), RecordingConfiguration::default())),
                (2, Response::Acknowledged),
            ],
        ]);

        set_recording(&descriptor, configuration).unwrap();
        let requests = requests.recv().unwrap();
        worker.join().unwrap();
        let mut legacy = configuration;
        legacy.cache = RecordingPolicy::default();

        assert_eq!(
            requests,
            vec![
                (
                    1,
                    Request::Hello {
                        authentication: descriptor.authentication,
                    },
                ),
                (2, Request::ReadCacheRecording),
                (
                    1,
                    Request::Hello {
                        authentication: descriptor.authentication,
                    },
                ),
                (2, Request::SetRecording(legacy)),
                (
                    1,
                    Request::Hello {
                        authentication: descriptor.authentication,
                    },
                ),
                (2, Request::SetCacheRecording(configuration.cache)),
            ]
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires real TCP sockets")]
    fn set_recording_rejects_cache_configuration_for_legacy_monitor() {
        let (descriptor, requests, worker) = serve(vec![vec![
            (1, hello(&test_descriptor(0), RecordingConfiguration::default())),
            (2, Response::Acknowledged),
        ]]);
        let mut configuration = RecordingConfiguration::default();
        configuration.cache.enabled = true;

        let error = set_recording(&descriptor, configuration).unwrap_err().to_string();
        let observed = requests.recv().unwrap();
        worker.join().unwrap();

        assert_eq!(
            (error, observed.len()),
            (
                "monitor rejected the request: cache recording is not supported by this monitor".to_owned(),
                2,
            )
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires real TCP sockets")]
    fn set_recording_accepts_default_cache_configuration_for_legacy_monitor() {
        let configuration = RecordingConfiguration::default();
        let (descriptor, requests, worker) = serve(vec![
            vec![(1, hello(&test_descriptor(0), configuration)), (2, Response::Acknowledged)],
            vec![(1, hello(&test_descriptor(0), configuration)), (2, Response::Acknowledged)],
        ]);

        set_recording(&descriptor, configuration).unwrap();
        let observed = requests.recv().unwrap();
        worker.join().unwrap();

        assert_eq!(
            observed
                .iter()
                .filter(|(_, request)| matches!(request, Request::SetRecording(_)))
                .count(),
            1
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires real TCP sockets")]
    fn snapshot_and_statistics_commands_return_complete_payloads() {
        let options = SnapshotOptions {
            event_buffers: EventBufferDisposition::Release,
        };
        let (descriptor, requests, worker) = serve(vec![vec![
            (1, hello(&test_descriptor(0), RecordingConfiguration::default())),
            (2, Response::Snapshot(vec![1, 2, 3])),
        ]]);
        let bytes = capture_snapshot(&descriptor, options).unwrap();
        let snapshot_requests = requests.recv().unwrap();
        worker.join().unwrap();

        let statistics = RecorderStatistics {
            thread_count: 2,
            total_events: 3,
            retained_events: 4,
            lost_events: 5,
            event_capacity_per_thread: 6,
            allocated_bytes: 7,
            recording: RecordingConfiguration::default(),
        };
        let (descriptor, requests, worker) = serve(vec![vec![
            (1, hello(&test_descriptor(0), RecordingConfiguration::default())),
            (2, Response::RecorderStatistics(statistics)),
        ]]);
        let actual_statistics = recorder_statistics(&descriptor).unwrap();
        let statistics_requests = requests.recv().unwrap();
        worker.join().unwrap();

        assert_eq!(
            (
                bytes,
                snapshot_requests[1].clone(),
                actual_statistics,
                statistics_requests[1].clone()
            ),
            (
                vec![1, 2, 3],
                (2, Request::CaptureSnapshot(options)),
                statistics,
                (2, Request::ReadRecorderStatistics),
            )
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires real TCP sockets")]
    fn command_rejects_wrong_ids_remote_errors_and_unexpected_payloads() {
        let cases = [
            (3, Response::Acknowledged, "monitor returned an unexpected response"),
            (2, Response::Error("failed".into()), "monitor rejected the request: failed"),
        ];
        let expected = cases.each_ref().map(|(_, _, expected)| (*expected).to_owned());
        let actual = cases.map(|(response_id, response, _)| {
            let (descriptor, requests, worker) = serve(vec![vec![
                (1, hello(&test_descriptor(0), RecordingConfiguration::default())),
                (response_id, response),
            ]]);
            let error = command(&descriptor, &Request::ReadRecorderStatistics).unwrap_err().to_string();
            requests.recv().unwrap();
            worker.join().unwrap();
            error
        });

        assert_eq!(actual, expected);

        let handshakes = [
            Response::Hello {
                instance_id: InstanceId::from_bytes([9; 16]),
                recording: RecordingConfiguration::default(),
            },
            Response::Error("hello failed".into()),
        ];
        let actual = handshakes.map(|response| {
            let (descriptor, requests, worker) = serve(vec![vec![(1, response)]]);
            let error = command(&descriptor, &Request::ReadRecorderStatistics).unwrap_err().to_string();
            requests.recv().unwrap();
            worker.join().unwrap();
            error
        });
        assert_eq!(
            actual,
            [
                "monitor returned an unexpected response",
                "monitor rejected the request: hello failed",
            ]
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires filesystem access and real TCP sockets")]
    fn discover_filters_files_and_sorts_reachable_monitors() {
        let directory = directory("discover");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("ignored.txt"), b"not a descriptor").unwrap();
        fs::write(directory.join("invalid.monitor"), b"invalid").unwrap();

        let (mut second, second_requests, second_worker) = serve(vec![vec![
            (1, hello(&test_descriptor(0), RecordingConfiguration::default())),
            (2, Response::Acknowledged),
        ]]);
        second.name = "zeta".into();
        second.write_file(directory.join("second.monitor")).unwrap();
        let (mut first, first_requests, first_worker) = serve(vec![vec![
            (1, hello(&test_descriptor(0), RecordingConfiguration::default())),
            (2, Response::Acknowledged),
        ]]);
        first.name = "alpha".into();
        first.write_file(directory.join("first.monitor")).unwrap();

        let instances = discover_in(&directory).unwrap();
        first_requests.recv().unwrap();
        second_requests.recv().unwrap();
        first_worker.join().unwrap();
        second_worker.join().unwrap();
        fs::remove_dir_all(&directory).unwrap();

        assert_eq!(
            instances
                .iter()
                .map(|instance| (instance.descriptor.name.as_str(), instance.recording))
                .collect::<Vec<_>>(),
            vec![
                ("alpha", RecordingConfiguration::default()),
                ("zeta", RecordingConfiguration::default()),
            ]
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires filesystem access")]
    fn discover_handles_missing_directories_and_snapshot_saves_exact_bytes() {
        let directory = directory("filesystem");
        assert!(discover_in(&directory).unwrap().is_empty());
        fs::create_dir_all(&directory).unwrap();
        let regular_file = directory.join("not-a-directory");
        fs::write(&regular_file, []).unwrap();
        assert!(matches!(
            discover_in(&regular_file),
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotADirectory
        ));
        let descriptor = test_descriptor(0);

        let path = save_snapshot_to(&directory, &descriptor, &[1, 2, 3]).unwrap();
        let contents = fs::read(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let current_directory_path = save_snapshot(&descriptor, &[4, 5, 6]).unwrap();
        let current_directory_contents = fs::read(&current_directory_path).unwrap();
        fs::remove_file(current_directory_path).unwrap();
        fs::remove_dir_all(&directory).unwrap();

        assert_eq!(
            (
                path.parent(),
                name.starts_with("worker-west-europe-42-"),
                name.ends_with(".seismograph"),
                contents,
                current_directory_contents,
            ),
            (Some(directory.as_path()), true, true, vec![1, 2, 3], vec![4, 5, 6])
        );
    }
}
