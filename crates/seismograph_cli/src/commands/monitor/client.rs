// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(test)]
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant, UNIX_EPOCH};

use seismograph_protocol::io::MonitorIo;
use seismograph_protocol::message::{RecorderActivity, RecordingConfiguration, Request, Response, SnapshotOptions};
use seismograph_protocol::monitor::MonitorDescriptor;
use seismograph_protocol::transport::Connection;
use seismograph_protocol::{read_control_response, read_response, write_request};

use super::Error;
use super::app::Instance;
use super::live_activity::RecorderUpdate;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(super) struct Discovery {
    pub(super) instances: Vec<Instance>,
    pub(super) warnings: Vec<String>,
}

impl From<Vec<Instance>> for Discovery {
    fn from(instances: Vec<Instance>) -> Self {
        Self {
            instances,
            warnings: Vec::new(),
        }
    }
}

pub(super) fn discover(io: &MonitorIo) -> Result<Discovery, Error> {
    let directories = seismograph_protocol::discovery::directories(io).map_err(Error::Protocol)?;
    let mut discovery = Discovery::default();
    for directory in directories {
        match directory {
            Ok(directory) => discover_directory(io, &directory, &mut discovery),
            Err(error) => discovery.warnings.push(error.to_string()),
        }
    }
    sort_instances(&mut discovery.instances);
    Ok(discovery)
}

#[cfg(test)]
fn discover_in(io: &MonitorIo, directory: &std::path::Path) -> Result<Discovery, Error> {
    let owner = io.current_owner().map_err(Error::Io)?;
    let directory = match seismograph_protocol::discovery::Directory::open(io, directory, &owner) {
        Ok(directory) => directory,
        Err(seismograph_protocol::Error::Registry { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(Discovery::default());
        }
        Err(error) => return Err(Error::Protocol(error)),
    };
    let mut discovery = Discovery::default();
    discover_directory(io, &directory, &mut discovery);
    sort_instances(&mut discovery.instances);
    Ok(discovery)
}

fn discover_directory(io: &MonitorIo, directory: &seismograph_protocol::discovery::Directory, discovery: &mut Discovery) {
    let descriptors = match directory.descriptors() {
        Ok(descriptors) => descriptors,
        Err(error) => {
            discovery.warnings.push(error.to_string());
            return;
        }
    };
    for descriptor in descriptors {
        let descriptor = match descriptor {
            Ok(descriptor) => descriptor,
            Err(error) => {
                discovery.warnings.push(error.to_string());
                continue;
            }
        };
        let recording = match handshake(io, &descriptor) {
            Ok(recording) => recording,
            Err(Error::Io(error)) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => continue,
            Err(error) => {
                discovery.warnings.push(format!("{}: {error}", descriptor.endpoint));
                continue;
            }
        };
        let server_version = match server_version(io, &descriptor) {
            Ok(version) => version,
            Err(error) => {
                discovery.warnings.push(format!("{}: {error}", descriptor.endpoint));
                continue;
            }
        };
        discovery.instances.push(Instance {
            descriptor,
            recording,
            server_version: Some(server_version),
        });
    }
}

fn sort_instances(instances: &mut [Instance]) {
    instances.sort_by(|left, right| {
        (
            &left.descriptor.owner,
            &left.descriptor.name,
            &left.descriptor.instance,
            left.descriptor.process_id,
        )
            .cmp(&(
                &right.descriptor.owner,
                &right.descriptor.name,
                &right.descriptor.instance,
                right.descriptor.process_id,
            ))
    });
}

pub(super) fn set_recording(
    io: &MonitorIo,
    descriptor: &MonitorDescriptor,
    configuration: RecordingConfiguration,
) -> Result<RecordingConfiguration, Error> {
    match command(io, descriptor, &Request::SetRecording(configuration))? {
        Response::Acknowledged => {}
        _ => return Err(Error::UnexpectedResponse),
    }
    handshake(io, descriptor)
}

pub(super) fn capture_snapshot(io: &MonitorIo, descriptor: &MonitorDescriptor, options: SnapshotOptions) -> Result<Vec<u8>, Error> {
    match command(io, descriptor, &Request::CaptureSnapshot(options))? {
        Response::Snapshot(bytes) => Ok(bytes),
        _ => Err(Error::UnexpectedResponse),
    }
}

pub(super) fn capture_snapshot_and_stop(io: &MonitorIo, descriptor: &MonitorDescriptor) -> Result<Vec<u8>, Error> {
    match command(io, descriptor, &Request::CaptureSnapshotAndStop)? {
        Response::Snapshot(bytes) => Ok(bytes),
        _ => Err(Error::UnexpectedResponse),
    }
}

pub(super) fn clear_event_buffers(io: &MonitorIo, descriptor: &MonitorDescriptor) -> Result<(), Error> {
    match command(io, descriptor, &Request::ClearEventBuffers)? {
        Response::Acknowledged => Ok(()),
        _ => Err(Error::UnexpectedResponse),
    }
}

pub(super) fn recorder_activity(io: &MonitorIo, descriptor: &MonitorDescriptor) -> Result<RecorderUpdate, Error> {
    let mut stream = connected_stream(io, descriptor)?;
    write_request(&mut stream, 2, &Request::ReadRecorderActivity).map_err(Error::Protocol)?;
    let mut activity = read_recorder_activity(&mut stream)?;
    drop(stream);
    activity.statistics.recording = handshake(io, descriptor)?;
    Ok(activity)
}

fn read_recorder_activity(stream: &mut ClientConnection) -> Result<RecorderActivity, Error> {
    match read_response(stream).map_err(Error::Protocol)? {
        (2, Response::RecorderActivity(activity)) => Ok(activity),
        (2, Response::Error(message)) => Err(Error::Remote(message)),
        _ => Err(Error::UnexpectedResponse),
    }
}

pub(super) fn save_snapshot(io: &MonitorIo, descriptor: &MonitorDescriptor, bytes: &[u8]) -> Result<PathBuf, Error> {
    let directory = std::path::Path::new(".");
    save_snapshot_to(io, directory, descriptor, bytes)
}

fn save_snapshot_to(io: &MonitorIo, directory: &std::path::Path, descriptor: &MonitorDescriptor, bytes: &[u8]) -> Result<PathBuf, Error> {
    let timestamp = io
        .system_time()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::Clock(error.to_string()))?
        .as_nanos();
    let name = sanitize(&descriptor.name);
    let path = directory.join(format!("{name}-{}-{timestamp}.seismograph", descriptor.process_id));
    io.write_snapshot(&path, bytes).map_err(Error::Io)?;
    Ok(path)
}

pub(super) fn handshake(io: &MonitorIo, descriptor: &MonitorDescriptor) -> Result<RecordingConfiguration, Error> {
    let mut stream = connect(io, descriptor)?;
    write_request(&mut stream, 1, &Request::Hello).map_err(Error::Protocol)?;
    match read_control_response(&mut stream).map_err(Error::Protocol)? {
        (1, Response::Hello { instance_id, recording }) if instance_id == descriptor.instance_id => Ok(recording),
        (_, Response::Error(message)) => Err(Error::Remote(message)),
        _ => Err(Error::UnexpectedResponse),
    }
}

fn server_version(io: &MonitorIo, descriptor: &MonitorDescriptor) -> Result<String, Error> {
    let mut stream = connected_stream(io, descriptor)?;
    write_request(&mut stream, 2, &Request::ReadServerVersion).map_err(Error::Protocol)?;
    read_server_version(&mut stream)
}

fn read_server_version(stream: &mut ClientConnection) -> Result<String, Error> {
    match read_control_response(stream).map_err(Error::Protocol)? {
        (2, Response::ServerVersion(version)) => Ok(version),
        (2, Response::Error(message)) => Err(Error::Remote(message)),
        _ => Err(Error::UnexpectedResponse),
    }
}

fn connect(io: &MonitorIo, descriptor: &MonitorDescriptor) -> Result<ClientConnection, Error> {
    let stream = Connection::connect(io, descriptor, CONNECT_TIMEOUT).map_err(Error::Io)?;
    ClientConnection::new(io, stream).map_err(Error::Io)
}

fn command(io: &MonitorIo, descriptor: &MonitorDescriptor, request: &Request) -> Result<Response, Error> {
    let mut stream = connected_stream(io, descriptor)?;
    write_request(&mut stream, 2, request).map_err(Error::Protocol)?;
    let snapshot = matches!(request, Request::CaptureSnapshot(_) | Request::CaptureSnapshotAndStop);
    if snapshot {
        stream.set_read_timeout(None).map_err(Error::Io)?;
    }
    let (request_id, response) = if snapshot {
        read_response(&mut stream)
    } else {
        read_control_response(&mut stream)
    }
    .map_err(Error::Protocol)?;
    if request_id != 2 {
        return Err(Error::UnexpectedResponse);
    }
    match response {
        Response::Error(message) => Err(Error::Remote(message)),
        response => Ok(response),
    }
}

fn connected_stream(io: &MonitorIo, descriptor: &MonitorDescriptor) -> Result<ClientConnection, Error> {
    let mut stream = connect(io, descriptor)?;
    write_request(&mut stream, 1, &Request::Hello).map_err(Error::Protocol)?;
    match read_control_response(&mut stream).map_err(Error::Protocol)? {
        (1, Response::Hello { instance_id, .. }) if instance_id == descriptor.instance_id => {}
        (_, Response::Error(message)) => return Err(Error::Remote(message)),
        _ => return Err(Error::UnexpectedResponse),
    }
    Ok(stream)
}

struct ClientConnection {
    io: MonitorIo,
    stream: Connection,
    deadline: Option<Instant>,
}

impl ClientConnection {
    fn new(io: &MonitorIo, stream: Connection) -> io::Result<Self> {
        let mut connection = Self {
            io: io.clone(),
            stream,
            deadline: None,
        };
        connection.set_read_timeout(Some(IO_TIMEOUT))?;
        connection.stream.set_write_timeout(Some(IO_TIMEOUT))?;
        Ok(connection)
    }

    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        let deadline = timeout
            .map(|timeout| {
                self.io
                    .instant()
                    .checked_add(timeout)
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "monitor response deadline is too large"))
            })
            .transpose()?;
        self.stream.set_read_timeout(timeout)?;
        self.deadline = deadline;
        Ok(())
    }

    fn remaining(&self) -> io::Result<Option<Duration>> {
        self.deadline
            .map(|deadline| {
                deadline
                    .checked_duration_since(self.io.instant())
                    .filter(|remaining| !remaining.is_zero())
                    .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "monitor response timed out"))
            })
            .transpose()
    }
}

impl Read for ClientConnection {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(self.remaining()?)?;
        self.stream.read(bytes)
    }
}

impl Write for ClientConnection {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(self.remaining()?)?;
        self.stream.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod io_tests;

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;

    use seismograph_protocol::message::{EventBufferDisposition, RecorderStatistics, RecordingPolicy};
    use seismograph_protocol::monitor::InstanceId;
    use seismograph_protocol::{read_request, write_response};

    use super::*;

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);
    type ScriptedServer = (MonitorDescriptor, mpsc::Receiver<Vec<(u64, Request)>>, thread::JoinHandle<()>);

    fn connected_pair() -> (ClientConnection, Connection) {
        let (client, server) = seismograph_protocol::testing::connected_pair();
        let client = ClientConnection::new(&MonitorIo::new_native(), client).unwrap();
        assert!(
            client.remaining().unwrap().is_some(),
            "native client fixtures require a finite response deadline"
        );
        (client, server)
    }

    fn test_descriptor() -> MonitorDescriptor {
        MonitorDescriptor {
            name: "worker west/europe".into(),
            instance: Some("test".into()),
            process_id: 42,
            instance_id: InstanceId::from_bytes([1; 16]),

            owner: seismograph_protocol::monitor::MonitorOwner::current().unwrap(),

            endpoint: seismograph_protocol::monitor::MonitorEndpoint::for_instance(
                &std::env::temp_dir(),
                &seismograph_protocol::monitor::MonitorOwner::current().unwrap(),
                42,
                InstanceId::from_bytes([1; 16]),
            ),
        }
    }

    fn directory(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join(format!(
            "client-unit-{name}-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn serve(conversations: Vec<Vec<(u64, Response)>>) -> ScriptedServer {
        serve_on_listener(conversations, false, seismograph_protocol::testing::listener())
    }

    fn serve_closing_after_handshake(conversations: Vec<Vec<(u64, Response)>>) -> ScriptedServer {
        serve_on_listener(conversations, true, seismograph_protocol::testing::listener())
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // Defensive test-server failures are not product behavior.
    fn serve_on_listener(
        conversations: Vec<Vec<(u64, Response)>>,
        close_after_handshake: bool,
        listener: seismograph_protocol::transport::Listener,
    ) -> ScriptedServer {
        listener.set_nonblocking(true).unwrap();
        let mut descriptor = listener.descriptor().clone();
        descriptor.name = "worker".into();
        let instance_id = descriptor.instance_id;
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            for conversation in conversations {
                let closes_on_request = close_after_handshake
                    && conversation
                        .last()
                        .is_some_and(|(_, response)| matches!(response, Response::Hello { .. }));
                let deadline = Instant::now() + Duration::from_secs(2);
                let mut stream = loop {
                    match listener.accept() {
                        Ok(stream) => break Some(stream),
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
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                for (response_id, mut response) in conversation {
                    let Ok(request) = read_request(&mut stream) else {
                        break;
                    };
                    requests.push(request);
                    if let Response::Hello {
                        instance_id: response_instance,
                        ..
                    } = &mut response
                        && *response_instance == InstanceId::from_bytes([1; 16])
                    {
                        *response_instance = instance_id;
                    }
                    write_response(&mut stream, response_id, &response).unwrap();
                }
                if closes_on_request {
                    let request = read_request(&mut stream).unwrap();
                    assert_eq!(request.0, 2);
                    requests.push(request);
                }
                if !closes_on_request {
                    let mut trailing = Vec::new();
                    stream.read_to_end(&mut trailing).unwrap();
                    assert!(trailing.is_empty());
                }
            }
            sender.send(requests).unwrap();
        });
        (descriptor, receiver, worker)
    }

    #[test]
    fn lifecycle_commands_use_distinct_authenticated_requests() {
        let hello = Response::Hello {
            instance_id: InstanceId::from_bytes([1; 16]),
            recording: RecordingConfiguration::default(),
        };
        let (descriptor, requests, worker) = serve(vec![
            vec![(1, hello.clone()), (2, Response::Snapshot(vec![42]))],
            vec![(1, hello), (2, Response::Acknowledged)],
        ]);
        assert_eq!(capture_snapshot_and_stop(&MonitorIo::new_native(), &descriptor).unwrap(), vec![42]);
        clear_event_buffers(&MonitorIo::new_native(), &descriptor).unwrap();
        let requests = requests.recv_timeout(Duration::from_secs(3)).unwrap();
        worker.join().unwrap();
        assert_eq!(requests[1], (2, Request::CaptureSnapshotAndStop));
        assert_eq!(requests[3], (2, Request::ClearEventBuffers));
    }

    #[test]
    fn interrupted_stop_and_clear_report_errors_without_retrying() {
        let hello = Response::Hello {
            instance_id: InstanceId::from_bytes([1; 16]),
            recording: RecordingConfiguration::default(),
        };
        let (descriptor, requests, worker) = serve_closing_after_handshake(vec![vec![(1, hello.clone())], vec![(1, hello)]]);
        let stop = capture_snapshot_and_stop(&MonitorIo::new_native(), &descriptor).unwrap_err();
        let clear = clear_event_buffers(&MonitorIo::new_native(), &descriptor).unwrap_err();
        let requests = requests.recv_timeout(Duration::from_secs(3)).unwrap();
        worker.join().unwrap();
        assert!(matches!(stop, Error::Protocol(_)));
        assert!(matches!(clear, Error::Protocol(_)));
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[1], (2, Request::CaptureSnapshotAndStop));
        assert_eq!(requests[3], (2, Request::ClearEventBuffers));
    }

    #[test]
    fn lifecycle_commands_reject_wrong_response_variants() {
        let hello = Response::Hello {
            instance_id: InstanceId::from_bytes([1; 16]),
            recording: RecordingConfiguration::default(),
        };
        let (descriptor, requests, worker) = serve(vec![
            vec![(1, hello.clone()), (2, Response::Acknowledged)],
            vec![(1, hello), (2, Response::Snapshot(Vec::new()))],
        ]);

        assert!(matches!(
            capture_snapshot_and_stop(&MonitorIo::new_native(), &descriptor),
            Err(Error::UnexpectedResponse)
        ));
        assert!(matches!(
            clear_event_buffers(&MonitorIo::new_native(), &descriptor),
            Err(Error::UnexpectedResponse)
        ));
        requests.recv_timeout(Duration::from_secs(3)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn continue_uses_the_retain_request() {
        let (descriptor, requests, worker) = serve(vec![vec![
            (
                1,
                Response::Hello {
                    instance_id: InstanceId::from_bytes([1; 16]),
                    recording: RecordingConfiguration::default(),
                },
            ),
            (2, Response::Snapshot(vec![7])),
        ]]);
        assert_eq!(
            capture_snapshot(&MonitorIo::new_native(), &descriptor, SnapshotOptions::default()).unwrap(),
            vec![7]
        );
        let requests = requests.recv_timeout(Duration::from_secs(3)).unwrap();
        worker.join().unwrap();
        assert_eq!(requests[1], (2, Request::CaptureSnapshot(SnapshotOptions::default())));
    }

    fn hello(descriptor: &MonitorDescriptor, recording: RecordingConfiguration) -> Response {
        Response::Hello {
            instance_id: descriptor.instance_id,
            recording,
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn version_query_rejects_missing_remote_and_unexpected_responses() {
        let (descriptor, requests, worker) =
            serve_closing_after_handshake(vec![vec![(1, hello(&test_descriptor(), RecordingConfiguration::default()))]]);
        assert!(matches!(
            server_version(&MonitorIo::new_native(), &descriptor),
            Err(Error::Protocol(_))
        ));
        assert_eq!(requests.recv().unwrap()[1], (2, Request::ReadServerVersion));
        worker.join().unwrap();

        for (id, response, expected) in [
            (
                2,
                Response::Error("version unavailable".into()),
                "monitor rejected the request: version unavailable",
            ),
            (
                3,
                Response::ServerVersion("9.8.7".into()),
                "monitor returned an unexpected response",
            ),
            (2, Response::Acknowledged, "monitor returned an unexpected response"),
        ] {
            let (descriptor, requests, worker) = serve(vec![vec![
                (1, hello(&test_descriptor(), RecordingConfiguration::default())),
                (id, response),
            ]]);
            assert_eq!(
                server_version(&MonitorIo::new_native(), &descriptor).unwrap_err().to_string(),
                expected
            );
            requests.recv().unwrap();
            worker.join().unwrap();
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn version_query_does_not_hide_partial_frames_malformed_payloads_or_timeouts() {
        for bytes in [b"SGMP".to_vec(), {
            let mut bytes = Vec::new();
            write_response(&mut bytes, 2, &Response::ServerVersion("1.2.3".into())).unwrap();
            // The frame remains complete, but its string claims more bytes than exist.
            bytes[20] = 0xff;
            bytes
        }] {
            let (mut client, server) = connected_pair();

            client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut server = server;
            server.write_all(&bytes).unwrap();
            drop(server);
            assert!(matches!(read_server_version(&mut client), Err(Error::Protocol(_))));
        }
        let (mut client, _server) = connected_pair();
        client.set_read_timeout(Some(Duration::from_millis(20))).unwrap();

        assert!(matches!(
            read_server_version(&mut client),
            Err(Error::Protocol(seismograph_protocol::Error::Io(error))) if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
        ));
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn live_activity_uses_detailed_counts_and_authoritative_configuration() {
        use seismograph_protocol::message::{EventClassCounts, ThreadRecorderStatistics};
        let activity = RecorderActivity {
            session_id: 7,
            statistics: RecorderStatistics {
                thread_count: 1,
                total_events: 100,
                retained_events: 64,
                lost_events: 36,
                event_capacity_per_thread: 64,
                ..RecorderStatistics::default()
            },
            class_events: EventClassCounts {
                general_events: 100,
                ..EventClassCounts::default()
            },
            threads: vec![ThreadRecorderStatistics {
                thread_id: 9,
                name: "worker".into(),
                total_events: 100,
                retained_events: 64,
                lost_events: 36,
                event_capacity: 64,
                ..ThreadRecorderStatistics::default()
            }],
        };
        let mut recording = RecordingConfiguration::default();
        recording.runtime_tasks.enabled = true;
        let (descriptor, requests, worker) = serve(vec![
            vec![
                (1, hello(&test_descriptor(), recording)),
                (2, Response::RecorderActivity(activity.clone())),
            ],
            vec![(1, hello(&test_descriptor(), recording))],
        ]);
        let actual = recorder_activity(&MonitorIo::new_native(), &descriptor).unwrap();
        let observed = requests.recv().unwrap();
        worker.join().unwrap();
        let mut expected = activity;
        expected.statistics.recording = recording;
        assert_eq!(actual, expected);
        assert_eq!(observed[1], (2, Request::ReadRecorderActivity));
        assert!(!observed.iter().any(|(_, request)| matches!(request, Request::CaptureSnapshot(_))));
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn closed_activity_response_is_an_error_without_a_statistics_fallback() {
        let recording = RecordingConfiguration::default();
        let (descriptor, requests, worker) = serve_closing_after_handshake(vec![vec![(1, hello(&test_descriptor(), recording))]]);
        assert!(matches!(
            recorder_activity(&MonitorIo::new_native(), &descriptor),
            Err(Error::Protocol(_))
        ));
        let observed = requests.recv().unwrap();
        worker.join().unwrap();
        assert_eq!(
            observed
                .iter()
                .filter(|(_, request)| matches!(request, Request::ReadRecorderActivity))
                .count(),
            1
        );
        assert_eq!(
            observed
                .iter()
                .filter(|(_, request)| matches!(request, Request::ReadRecorderStatistics))
                .count(),
            0
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn live_activity_does_not_hide_truncated_responses_timeouts_or_remote_errors() {
        for bytes in [b"SGMP".to_vec(), {
            let mut bytes = Vec::new();
            write_response(&mut bytes, 2, &Response::RecorderActivity(RecorderActivity::default())).unwrap();
            bytes.pop();
            bytes
        }] {
            let (mut client, server) = connected_pair();

            client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut server = server;
            server.write_all(&bytes).unwrap();
            drop(server);
            assert!(matches!(read_recorder_activity(&mut client), Err(Error::Protocol(_))));
        }
        let (mut client, _server) = connected_pair();
        client.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        assert!(matches!(read_recorder_activity(&mut client), Err(Error::Protocol(_))));
        let (mut client, mut server) = connected_pair();
        write_response(&mut server, 2, &Response::Error("busy".into())).unwrap();
        assert!(matches!(read_recorder_activity(&mut client), Err(Error::Remote(message)) if message == "busy"));

        let (mut client, server) = connected_pair();

        let mut server = server;
        write_response(&mut server, 3, &Response::Acknowledged).unwrap();
        assert!(matches!(read_recorder_activity(&mut client), Err(Error::UnexpectedResponse)));
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
    #[cfg_attr(miri, ignore = "requires filesystem access and native local IPC")]
    fn discover_finds_a_published_monitor() {
        let name = format!(
            "client-discovery-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        );
        let directory = seismograph_protocol::testing::Directory::new();
        let listener =
            seismograph_protocol::transport::Listener::bind(&seismograph_protocol::io::MonitorIo::new_native(), &directory.descriptor())
                .unwrap();
        let (mut descriptor, requests, worker) = serve_on_listener(
            vec![
                vec![(1, hello(&test_descriptor(), RecordingConfiguration::default()))],
                vec![
                    (1, hello(&test_descriptor(), RecordingConfiguration::default())),
                    (2, Response::ServerVersion("12.34.56-rc.7".into())),
                ],
            ],
            false,
            listener,
        );
        descriptor.name.clone_from(&name);
        let descriptor_path = directory.path().join(descriptor.file_name());
        descriptor
            .write_file(&seismograph_protocol::io::MonitorIo::new_native(), &descriptor_path)
            .unwrap();
        let mut unavailable = test_descriptor();
        unavailable.name = format!("{name}-unavailable");
        unavailable.process_id = 43;
        unavailable.instance_id = InstanceId::from_bytes([3; 16]);
        unavailable.endpoint = seismograph_protocol::monitor::MonitorEndpoint::for_instance(
            directory.path(),
            &unavailable.owner,
            unavailable.process_id,
            unavailable.instance_id,
        );
        unavailable
            .write_file(
                &seismograph_protocol::io::MonitorIo::new_native(),
                directory.path().join(unavailable.file_name()),
            )
            .unwrap();

        let instances = discover_in(&MonitorIo::new_native(), directory.path()).unwrap();

        assert_eq!(
            instances
                .instances
                .iter()
                .map(|instance| (instance.descriptor.name.as_str(), instance.server_version.as_deref()))
                .collect::<Vec<_>>(),
            [(name.as_str(), Some("12.34.56-rc.7"))]
        );
        requests.recv().unwrap();
        worker.join().unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn handshake_reads_the_complete_recording_state() {
        let mut recording = RecordingConfiguration::default();
        recording.allocations.enabled = true;
        let cache = RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            sampling_one_in: 8,
        };
        recording.cache = cache;
        let (descriptor, requests, worker) = serve(vec![vec![(1, hello(&test_descriptor(), recording))]]);
        // The scripted response needs the listener's instance identity, not its port.
        let result = handshake(&MonitorIo::new_native(), &descriptor).unwrap();
        let requests = requests.recv().unwrap();
        worker.join().unwrap();

        assert_eq!((result, requests), (recording, vec![(1, Request::Hello)],));
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
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
            let error = handshake(&MonitorIo::new_native(), &descriptor).unwrap_err().to_string();
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
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn set_recording_sends_all_policies_in_one_request_and_reads_authoritative_state() {
        let mut configuration = RecordingConfiguration::default();
        configuration.allocations.enabled = true;
        configuration.cache = RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            sampling_one_in: 16,
        };
        let mut authoritative = configuration;
        authoritative.event_capacity_per_thread = 1_024;
        let (descriptor, requests, worker) = serve(vec![
            vec![
                (1, hello(&test_descriptor(), RecordingConfiguration::default())),
                (2, Response::Acknowledged),
            ],
            vec![(1, hello(&test_descriptor(), authoritative))],
        ]);

        let actual = set_recording(&MonitorIo::new_native(), &descriptor, configuration).unwrap();
        let requests = requests.recv().unwrap();
        worker.join().unwrap();
        assert_eq!(actual, authoritative);

        assert_eq!(
            requests,
            vec![
                (1, Request::Hello,),
                (2, Request::SetRecording(configuration)),
                (1, Request::Hello,),
            ]
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn set_recording_reports_rejection_without_a_readback() {
        let (descriptor, requests, worker) = serve(vec![vec![
            (1, hello(&test_descriptor(), RecordingConfiguration::default())),
            (2, Response::Error("configuration rejected".into())),
        ]]);
        let mut configuration = RecordingConfiguration::default();
        configuration.cache.enabled = true;

        let error = set_recording(&MonitorIo::new_native(), &descriptor, configuration)
            .unwrap_err()
            .to_string();
        let observed = requests.recv().unwrap();
        worker.join().unwrap();

        assert_eq!(
            (error, observed.len()),
            ("monitor rejected the request: configuration rejected".to_owned(), 2,)
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn handshake_does_not_hide_remote_errors_or_wrong_response_ids() {
        for (response_id, response, expected) in [
            (1, Response::Acknowledged, "monitor returned an unexpected response"),
            (
                2,
                hello(&test_descriptor(), RecordingConfiguration::default()),
                "monitor returned an unexpected response",
            ),
            (
                1,
                Response::Error("configuration unavailable".into()),
                "monitor rejected the request: configuration unavailable",
            ),
        ] {
            let (descriptor, requests, worker) = serve(vec![vec![(response_id, response)]]);
            let error = handshake(&MonitorIo::new_native(), &descriptor).unwrap_err().to_string();
            requests.recv().unwrap();
            worker.join().unwrap();
            assert_eq!(error, expected);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn unexpected_recording_write_response_is_not_reported_as_success() {
        let configuration = RecordingConfiguration::default();
        let (descriptor, requests, worker) = serve(vec![vec![
            (1, hello(&test_descriptor(), configuration)),
            (2, Response::Snapshot(Vec::new())),
        ]]);

        let error = set_recording(&MonitorIo::new_native(), &descriptor, configuration)
            .unwrap_err()
            .to_string();
        requests.recv().unwrap();
        worker.join().unwrap();

        assert_eq!(error, "monitor returned an unexpected response");
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn successful_write_requires_complete_configuration_readback() {
        let configuration = RecordingConfiguration::default();
        let (descriptor, requests, worker) = serve(vec![
            vec![(1, hello(&test_descriptor(), configuration)), (2, Response::Acknowledged)],
            vec![(1, Response::Acknowledged)],
        ]);

        let error = set_recording(&MonitorIo::new_native(), &descriptor, configuration)
            .unwrap_err()
            .to_string();
        requests.recv().unwrap();
        worker.join().unwrap();

        assert_eq!(error, "monitor returned an unexpected response");
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn snapshot_and_activity_commands_return_complete_payloads() {
        let options = SnapshotOptions {
            event_buffers: EventBufferDisposition::Release,
        };
        let (descriptor, requests, worker) = serve(vec![vec![
            (1, hello(&test_descriptor(), RecordingConfiguration::default())),
            (2, Response::Snapshot(vec![1, 2, 3])),
        ]]);
        let bytes = capture_snapshot(&MonitorIo::new_native(), &descriptor, options).unwrap();
        let snapshot_requests = requests.recv().unwrap();
        worker.join().unwrap();

        let recording = RecordingConfiguration {
            cache: RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                sampling_one_in: 8,
            },
            ..RecordingConfiguration::default()
        };
        let statistics = RecorderStatistics {
            thread_count: 2,
            total_events: 3,
            retained_events: 4,
            lost_events: 5,
            event_capacity_per_thread: 6,
            allocated_bytes: 7,
            recording: RecordingConfiguration::default(),
        };
        let (descriptor, requests, worker) = serve(vec![
            vec![
                (1, hello(&test_descriptor(), RecordingConfiguration::default())),
                (
                    2,
                    Response::RecorderActivity(RecorderActivity {
                        statistics,
                        ..RecorderActivity::default()
                    }),
                ),
            ],
            vec![(1, hello(&test_descriptor(), recording))],
        ]);
        let actual_statistics = recorder_activity(&MonitorIo::new_native(), &descriptor).unwrap().statistics;
        let statistics = RecorderStatistics { recording, ..statistics };
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
                (2, Request::ReadRecorderActivity),
            )
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn statistics_use_live_policy_instead_of_retained_session_policy() {
        let mut retained_recording = RecordingConfiguration::default();
        retained_recording.runtime_tasks.enabled = true;
        retained_recording.runtime_tasks.capture_backtraces = true;
        let statistics = RecorderStatistics {
            total_events: 42,
            retained_events: 42,
            recording: retained_recording,
            ..RecorderStatistics::default()
        };
        let current = RecordingConfiguration::default();
        let (descriptor, requests, worker) = serve(vec![
            vec![
                (1, hello(&test_descriptor(), current)),
                (
                    2,
                    Response::RecorderActivity(RecorderActivity {
                        statistics,
                        ..RecorderActivity::default()
                    }),
                ),
            ],
            vec![(1, hello(&test_descriptor(), current))],
        ]);

        let actual = recorder_activity(&MonitorIo::new_native(), &descriptor).unwrap().statistics;
        requests.recv().unwrap();
        worker.join().unwrap();

        assert_eq!(
            actual,
            RecorderStatistics {
                recording: current,
                ..statistics
            }
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn command_rejects_wrong_ids_remote_errors_and_unexpected_payloads() {
        let cases = [
            (3, Response::Acknowledged, "monitor returned an unexpected response"),
            (2, Response::Error("failed".into()), "monitor rejected the request: failed"),
        ];
        let expected = cases.each_ref().map(|(_, _, expected)| (*expected).to_owned());
        let actual = cases.map(|(response_id, response, _)| {
            let (descriptor, requests, worker) = serve(vec![vec![
                (1, hello(&test_descriptor(), RecordingConfiguration::default())),
                (response_id, response),
            ]]);
            let error = command(&MonitorIo::new_native(), &descriptor, &Request::ReadRecorderStatistics)
                .unwrap_err()
                .to_string();
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
            let error = command(&MonitorIo::new_native(), &descriptor, &Request::ReadRecorderStatistics)
                .unwrap_err()
                .to_string();
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
    #[cfg_attr(miri, ignore = "requires filesystem access and native local IPC")]
    fn discover_filters_files_and_sorts_reachable_monitors() {
        let directory = seismograph_protocol::testing::Directory::new();
        fs::write(directory.path().join("ignored.txt"), b"not a descriptor").unwrap();
        let malformed = directory.descriptor();
        let malformed_path = directory.path().join(malformed.file_name());
        malformed
            .write_file(&seismograph_protocol::io::MonitorIo::new_native(), &malformed_path)
            .unwrap();
        fs::write(&malformed_path, b"invalid descriptor").unwrap();

        let second_listener =
            seismograph_protocol::transport::Listener::bind(&seismograph_protocol::io::MonitorIo::new_native(), &directory.descriptor())
                .unwrap();
        let (mut second, second_requests, second_worker) = serve_on_listener(
            vec![
                vec![(1, hello(&test_descriptor(), RecordingConfiguration::default()))],
                vec![
                    (1, hello(&test_descriptor(), RecordingConfiguration::default())),
                    (2, Response::ServerVersion("current".into())),
                ],
            ],
            false,
            second_listener,
        );
        second.name = "zeta".into();
        let second_path = directory.path().join(second.file_name());
        second
            .write_file(&seismograph_protocol::io::MonitorIo::new_native(), &second_path)
            .unwrap();
        let first_listener =
            seismograph_protocol::transport::Listener::bind(&seismograph_protocol::io::MonitorIo::new_native(), &directory.descriptor())
                .unwrap();
        let (mut first, first_requests, first_worker) = serve_on_listener(
            vec![
                vec![(1, hello(&test_descriptor(), RecordingConfiguration::default()))],
                vec![
                    (1, hello(&test_descriptor(), RecordingConfiguration::default())),
                    (2, Response::ServerVersion("current".into())),
                ],
            ],
            false,
            first_listener,
        );
        first.name = "alpha".into();
        let first_path = directory.path().join(first.file_name());
        first
            .write_file(&seismograph_protocol::io::MonitorIo::new_native(), &first_path)
            .unwrap();

        let instances = discover_in(&MonitorIo::new_native(), directory.path()).unwrap();
        first_requests.recv().unwrap();
        second_requests.recv().unwrap();
        first_worker.join().unwrap();
        second_worker.join().unwrap();

        assert_eq!(instances.warnings.len(), 1);
        assert!(instances.warnings[0].contains("invalid Seismograph monitor descriptor"));
        assert_eq!(
            instances
                .instances
                .iter()
                .map(|instance| (
                    instance.descriptor.name.as_str(),
                    instance.recording,
                    instance.server_version.as_deref()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("alpha", RecordingConfiguration::default(), Some("current")),
                ("zeta", RecordingConfiguration::default(), Some("current")),
            ]
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires filesystem access")]
    fn discover_handles_missing_directories_and_snapshot_saves_exact_bytes() {
        let missing = directory("missing");
        assert!(discover_in(&MonitorIo::new_native(), &missing).unwrap().instances.is_empty());
        let directory = seismograph_protocol::testing::Directory::new();
        let regular_file = directory.path().join("not-a-directory");
        fs::write(&regular_file, []).unwrap();
        assert!(matches!(
            discover_in(&MonitorIo::new_native(), &regular_file),
            Err(Error::Protocol(seismograph_protocol::Error::Registry { .. }))
        ));
        let descriptor = test_descriptor();

        let path = save_snapshot_to(&MonitorIo::new_native(), directory.path(), &descriptor, &[1, 2, 3]).unwrap();
        let contents = fs::read(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let current_directory_path = save_snapshot(&MonitorIo::new_native(), &descriptor, &[4, 5, 6]).unwrap();
        let current_directory_contents = fs::read(&current_directory_path).unwrap();
        fs::remove_file(current_directory_path).unwrap();

        assert_eq!(
            (
                path.parent(),
                name.starts_with("worker-west-europe-42-"),
                name.ends_with(".seismograph"),
                contents,
                current_directory_contents,
            ),
            (Some(directory.path()), true, true, vec![1, 2, 3], vec![4, 5, 6])
        );
    }
}
