// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Feature-gated localhost monitor server.
//!
//! Each monitor binds an ephemeral `127.0.0.1` port and publishes an
//! authenticated descriptor in the current user's runtime directory. This
//! allows many instrumented processes to coexist without fixed-port
//! coordination.

use std::net::{Ipv4Addr, Shutdown, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use std::{fmt, fs, io};

#[cfg(test)]
use seismograph_protocol::message::SnapshotOptions;
use seismograph_protocol::message::{EventBufferDisposition, RecorderStatistics, RecordingConfiguration, Request, Response};
use seismograph_protocol::monitor::{AuthenticationToken, InstanceId, MonitorDescriptor};

use crate::recorder::SuppressionGuard;

const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(50);
const CLIENT_READ_TIMEOUT: Duration = Duration::from_millis(250);
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Running localhost monitor.
pub struct Monitor {
    descriptor: MonitorDescriptor,
    descriptor_path: PathBuf,
    stop: Arc<AtomicBool>,
    active_client: Arc<Mutex<Option<TcpStream>>>,
    last_error: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl Monitor {
    /// Creates a monitor builder.
    #[must_use]
    pub fn builder() -> Builder {
        Builder::default()
    }

    /// Returns this process instance's published descriptor.
    #[must_use]
    pub const fn descriptor(&self) -> &MonitorDescriptor {
        &self.descriptor
    }

    /// Returns the most recent background connection error.
    #[must_use]
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

impl fmt::Debug for Monitor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Monitor")
            .field("descriptor", &self.descriptor)
            .field("descriptor_path", &self.descriptor_path)
            .finish_non_exhaustive()
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(stream) = self
            .active_client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
        let _ = TcpStream::connect(self.descriptor.socket_address());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.descriptor_path);
    }
}

/// Builder for a localhost monitor.
#[derive(Debug, Default)]
pub struct Builder {
    name: Option<String>,
    instance: Option<String>,
}

impl Builder {
    /// Sets the human-readable application name.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Sets an optional instance label used to distinguish similar processes.
    #[must_use]
    pub fn instance(mut self, instance: impl Into<String>) -> Self {
        self.instance = Some(instance.into());
        self
    }

    /// Starts the localhost listener and publishes its discovery descriptor.
    ///
    /// # Errors
    ///
    /// Returns an error when identity generation, binding, discovery publication,
    /// or listener-thread creation fails.
    pub fn start(self) -> Result<Monitor, Error> {
        self.start_with(spawn_listener_thread)
    }

    fn start_with(
        self,
        spawn: impl FnOnce(
            TcpListener,
            MonitorDescriptor,
            Arc<AtomicBool>,
            Arc<Mutex<Option<TcpStream>>>,
            Arc<Mutex<Option<String>>>,
            &Path,
        ) -> Result<JoinHandle<()>, Error>,
    ) -> Result<Monitor, Error> {
        let name = self.name.unwrap_or_else(default_application_name);
        if name.is_empty() || self.instance.as_ref().is_some_and(String::is_empty) {
            return Err(Error::InvalidIdentity);
        }

        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).map_err(Error::Bind)?;
        listener.set_nonblocking(true).map_err(Error::ConfigureListener)?;
        let port = listener.local_addr().map_err(Error::ReadAddress)?.port();

        let mut instance_id = [0; 16];
        getrandom::fill(&mut instance_id).map_err(Error::Random)?;
        let mut authentication = [0; 32];
        getrandom::fill(&mut authentication).map_err(Error::Random)?;
        let descriptor = MonitorDescriptor {
            name,
            instance: self.instance,
            process_id: std::process::id(),
            instance_id: InstanceId::from_bytes(instance_id),
            port,
            authentication: AuthenticationToken::from_bytes(authentication),
        };

        let directory = seismograph_protocol::monitor_directory().map_err(Error::Protocol)?;
        create_monitor_directory(&directory)?;
        let descriptor_path = publish_descriptor(&directory, &descriptor)?;

        let stop = Arc::new(AtomicBool::new(false));
        let active_client = Arc::new(Mutex::new(None));
        let last_error = Arc::new(Mutex::new(None));
        let thread = spawn(
            listener,
            descriptor.clone(),
            Arc::clone(&stop),
            Arc::clone(&active_client),
            Arc::clone(&last_error),
            &descriptor_path,
        )?;

        Ok(Monitor {
            descriptor,
            descriptor_path,
            stop,
            active_client,
            last_error,
            thread: Some(thread),
        })
    }
}

fn default_application_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "application".into())
}

fn create_monitor_directory(path: &Path) -> Result<(), Error> {
    fs::create_dir_all(path).map_err(|source| Error::CreateDirectory {
        path: path.to_owned(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| Error::SetPermissions {
            path: path.to_owned(),
            source,
        })?;
    }
    Ok(())
}

fn publish_descriptor(directory: &Path, descriptor: &MonitorDescriptor) -> Result<PathBuf, Error> {
    let descriptor_path = directory.join(descriptor.file_name());
    let temporary_path = descriptor_path.with_extension("monitor.tmp");
    descriptor.write_file(&temporary_path).map_err(|source| Error::Publish {
        path: temporary_path.clone(),
        source,
    })?;
    fs::rename(&temporary_path, &descriptor_path).map_err(|source| Error::Rename {
        from: temporary_path,
        to: descriptor_path.clone(),
        source,
    })?;
    Ok(descriptor_path)
}

#[cfg_attr(coverage_nightly, coverage(off))] // OS accept and accepted-socket setup failures cannot be injected portably; request dispatch is tested through connected streams.
fn run_listener(
    listener: &TcpListener,
    descriptor: &MonitorDescriptor,
    stop: &AtomicBool,
    active_client: &Mutex<Option<TcpStream>>,
    last_error: &Mutex<Option<String>>,
) {
    let _suppression = SuppressionGuard::enter();
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                if let Err(error) = stream.set_nonblocking(false) {
                    *last_error.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.to_string());
                    continue;
                }
                let client = match stream.try_clone() {
                    Ok(client) => client,
                    Err(error) => {
                        *last_error.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.to_string());
                        continue;
                    }
                };
                *active_client.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(client);
                if let Err(error) = handle_client(stream, descriptor, stop) {
                    *last_error.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.to_string());
                }
                *active_client.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(ACCEPT_RETRY_DELAY),
            Err(error) => {
                *last_error.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.to_string());
                break;
            }
        }
    }
}

fn spawn_listener_thread(
    listener: TcpListener,
    descriptor: MonitorDescriptor,
    stop: Arc<AtomicBool>,
    active_client: Arc<Mutex<Option<TcpStream>>>,
    last_error: Arc<Mutex<Option<String>>>,
    descriptor_path: &Path,
) -> Result<JoinHandle<()>, Error> {
    spawn_listener_thread_with(
        move || run_listener(&listener, &descriptor, &stop, &active_client, &last_error),
        descriptor_path,
        |operation| thread::Builder::new().name("seismograph-monitor".into()).spawn(operation),
    )
}

fn spawn_listener_thread_with(
    operation: impl FnOnce() + Send + 'static,
    descriptor_path: &Path,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<JoinHandle<()>>,
) -> Result<JoinHandle<()>, Error> {
    spawn(Box::new(operation)).map_err(|source| {
        let _ = fs::remove_file(descriptor_path);
        Error::Spawn(source)
    })
}

fn handle_client(mut stream: TcpStream, descriptor: &MonitorDescriptor, stop: &AtomicBool) -> Result<(), ClientError> {
    stream.set_read_timeout(Some(CLIENT_READ_TIMEOUT)).map_err(ClientError::Io)?;
    stream.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT)).map_err(ClientError::Io)?;

    let (request_id, request) = read_request_retry(&mut stream, stop)?;
    let Request::Hello { authentication } = request else {
        return Err(ClientError::HandshakeRequired);
    };
    if !tokens_equal(authentication, descriptor.authentication) {
        seismograph_protocol::write_response(&mut stream, request_id, &Response::Error("authentication failed".into()))
            .map_err(ClientError::Protocol)?;
        return Err(ClientError::Authentication);
    }
    let configuration = crate::recorder::configuration();
    seismograph_protocol::write_response(
        &mut stream,
        request_id,
        &Response::Hello {
            instance_id: descriptor.instance_id,
            recording: protocol_recording_configuration(
                configuration.allocations,
                configuration.general_events,
                configuration.arc_dereferences,
                configuration.runtime_tasks,
                configuration.io,
                configuration.cache,
                configuration.event_capacity_per_thread.get(),
            ),
        },
    )
    .map_err(ClientError::Protocol)?;

    while !stop.load(Ordering::Acquire) {
        let (request_id, request) = match read_request_retry(&mut stream, stop) {
            Ok(request) => request,
            Err(ClientError::Stopped | ClientError::Disconnected) => return Ok(()),
            Err(error) => return Err(error),
        };
        match request {
            Request::Hello { .. } => {
                seismograph_protocol::write_response(&mut stream, request_id, &Response::Error("already authenticated".into()))
                    .map_err(ClientError::Protocol)?;
            }
            Request::SetRecording(configuration) => {
                apply_recording_configuration(configuration);
                seismograph_protocol::write_response(&mut stream, request_id, &Response::Acknowledged).map_err(ClientError::Protocol)?;
            }
            Request::SetCacheRecording(policy) => {
                let configuration = crate::recorder::configuration();
                crate::recorder(crate::recorder::Configuration {
                    cache: recorder_policy(policy),
                    ..configuration
                });
                seismograph_protocol::write_response(&mut stream, request_id, &Response::Acknowledged).map_err(ClientError::Protocol)?;
            }
            Request::CaptureSnapshot(options) => {
                let event_buffers = match options.event_buffers {
                    EventBufferDisposition::Retain => crate::snapshot::EventBufferDisposition::Retain,
                    EventBufferDisposition::Clear => crate::snapshot::EventBufferDisposition::Clear,
                    EventBufferDisposition::Release => crate::snapshot::EventBufferDisposition::Release,
                };
                let response = snapshot_response(crate::snapshot(crate::snapshot::SnapshotOptions { event_buffers }));
                seismograph_protocol::write_response(&mut stream, request_id, &response).map_err(ClientError::Protocol)?;
            }
            Request::ReadRecorderStatistics => {
                seismograph_protocol::write_response(&mut stream, request_id, &recorder_statistics_response())
                    .map_err(ClientError::Protocol)?;
            }
            Request::ReadCacheRecording => {
                let policy = protocol_recording_policy(crate::recorder::configuration().cache);
                seismograph_protocol::write_response(&mut stream, request_id, &Response::CacheRecording(policy))
                    .map_err(ClientError::Protocol)?;
            }
        }
    }
    Ok(())
}

fn apply_recording_configuration(configuration: RecordingConfiguration) {
    let event_capacity_per_thread = crate::recorder::EventBufferCapacity::new(
        usize::try_from(configuration.event_capacity_per_thread).expect("Seismograph requires a target whose usize can represent u32"),
    )
    .expect("the monitor protocol decoder validates event buffer capacity");
    let current = crate::recorder::configuration();
    crate::recorder(crate::recorder::Configuration {
        allocations: recorder_policy(configuration.allocations),
        general_events: recorder_policy(configuration.general_events),
        arc_dereferences: recorder_policy(configuration.arc_dereferences),
        runtime_tasks: recorder_policy(configuration.runtime_tasks),
        io: recorder_policy(configuration.io),
        cache: current.cache,
        event_capacity_per_thread,
    });
}

fn recorder_statistics_response() -> Response {
    let statistics = crate::recorder::statistics();
    Response::RecorderStatistics(RecorderStatistics {
        thread_count: statistics.thread_count,
        total_events: statistics.total_events,
        retained_events: statistics.retained_events,
        lost_events: statistics.lost_events,
        event_capacity_per_thread: statistics.event_capacity_per_thread,
        allocated_bytes: statistics.allocated_bytes,
        recording: protocol_recording_configuration(
            statistics.recording.allocations,
            statistics.recording.general_events,
            statistics.recording.arc_dereferences,
            statistics.recording.runtime_tasks,
            statistics.recording.io,
            statistics.recording.cache,
            usize::try_from(statistics.event_capacity_per_thread).unwrap_or(usize::MAX),
        ),
    })
}

fn protocol_recording_configuration(
    allocations: crate::recorder::RecordingPolicy,
    general_events: crate::recorder::RecordingPolicy,
    arc_dereferences: crate::recorder::RecordingPolicy,
    runtime_tasks: crate::recorder::RecordingPolicy,
    io: crate::recorder::RecordingPolicy,
    cache: crate::recorder::RecordingPolicy,
    event_capacity_per_thread: usize,
) -> RecordingConfiguration {
    RecordingConfiguration {
        allocations: protocol_recording_policy(allocations),
        general_events: protocol_recording_policy(general_events),
        arc_dereferences: protocol_recording_policy(arc_dereferences),
        runtime_tasks: protocol_recording_policy(runtime_tasks),
        io: protocol_recording_policy(io),
        cache: protocol_recording_policy(cache),
        event_capacity_per_thread: u32::try_from(event_capacity_per_thread).unwrap_or(u32::MAX),
    }
}

fn protocol_recording_policy(policy: crate::recorder::RecordingPolicy) -> seismograph_protocol::message::RecordingPolicy {
    seismograph_protocol::message::RecordingPolicy {
        enabled: policy.enabled,
        capture_backtraces: policy.capture_backtraces,
        sampling_one_in: u32::try_from(policy.event_sampling.get()).unwrap_or(u32::MAX),
    }
}

fn recorder_policy(policy: seismograph_protocol::message::RecordingPolicy) -> crate::recorder::RecordingPolicy {
    crate::recorder::RecordingPolicy {
        enabled: policy.enabled,
        capture_backtraces: policy.capture_backtraces,
        event_sampling: crate::recorder::EventSampling::one_in(
            usize::try_from(policy.sampling_one_in).expect("Seismograph requires a target whose usize can represent u32"),
        )
        .expect("the monitor protocol decoder validates event sampling"),
    }
}

fn snapshot_response(snapshot: Result<crate::snapshot::Snapshot, crate::Error>) -> Response {
    match snapshot {
        Ok(snapshot) => Response::Snapshot(snapshot.as_bytes().to_vec()),
        Err(error) => Response::Error(error.to_string()),
    }
}

fn read_request_retry(stream: &mut TcpStream, stop: &AtomicBool) -> Result<(u64, Request), ClientError> {
    read_request_retry_with(stop, || seismograph_protocol::read_request(stream))
}

fn read_request_retry_with(
    stop: &AtomicBool,
    mut read: impl FnMut() -> Result<(u64, Request), seismograph_protocol::Error>,
) -> Result<(u64, Request), ClientError> {
    loop {
        match read() {
            Ok(request) => return Ok(request),
            Err(error) => {
                if let seismograph_protocol::Error::Io(io_error) = &error {
                    match io_error_action(io_error.kind(), stop.load(Ordering::Acquire)) {
                        IoErrorAction::Retry => continue,
                        IoErrorAction::Stopped => return Err(ClientError::Stopped),
                        IoErrorAction::Disconnected => return Err(ClientError::Disconnected),
                        IoErrorAction::Protocol => {}
                    }
                }
                return Err(ClientError::Protocol(error));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IoErrorAction {
    Retry,
    Stopped,
    Disconnected,
    Protocol,
}

fn io_error_action(kind: io::ErrorKind, stopped: bool) -> IoErrorAction {
    if matches!(kind, io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) {
        if stopped { IoErrorAction::Stopped } else { IoErrorAction::Retry }
    } else if matches!(kind, io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset) {
        IoErrorAction::Disconnected
    } else if stopped {
        IoErrorAction::Stopped
    } else {
        IoErrorAction::Protocol
    }
}

fn tokens_equal(left: AuthenticationToken, right: AuthenticationToken) -> bool {
    left.as_bytes()
        .into_iter()
        .zip(right.as_bytes())
        .fold(0_u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

#[derive(Debug)]
enum ClientError {
    Io(io::Error),
    Protocol(seismograph_protocol::Error),
    HandshakeRequired,
    Authentication,
    Disconnected,
    Stopped,
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Protocol(error) => write!(f, "{error}"),
            Self::HandshakeRequired => f.write_str("monitor handshake is required"),
            Self::Authentication => f.write_str("monitor authentication failed"),
            Self::Disconnected => f.write_str("monitor client disconnected"),
            Self::Stopped => f.write_str("monitor stopped"),
        }
    }
}

/// Monitor startup failure.
#[derive(Debug)]
pub enum Error {
    /// Application or instance name is empty.
    InvalidIdentity,
    /// Listener could not bind to localhost.
    Bind(io::Error),
    /// Listener socket options could not be configured.
    ConfigureListener(io::Error),
    /// Listener address could not be read.
    ReadAddress(io::Error),
    /// Secure random identity generation failed.
    Random(getrandom::Error),
    /// Protocol discovery operation failed.
    Protocol(seismograph_protocol::Error),
    /// Discovery directory could not be created.
    CreateDirectory {
        /// Directory being created.
        path: PathBuf,
        /// Filesystem failure.
        source: io::Error,
    },
    /// Discovery directory permissions could not be restricted.
    SetPermissions {
        /// Directory being secured.
        path: PathBuf,
        /// Filesystem failure.
        source: io::Error,
    },
    /// Descriptor could not be published.
    Publish {
        /// Descriptor path.
        path: PathBuf,
        /// Encoding or filesystem failure.
        source: seismograph_protocol::Error,
    },
    /// Temporary descriptor could not be atomically renamed.
    Rename {
        /// Temporary path.
        from: PathBuf,
        /// Published path.
        to: PathBuf,
        /// Filesystem failure.
        source: io::Error,
    },
    /// Listener thread could not be created.
    Spawn(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity => f.write_str("monitor application and instance names must not be empty"),
            Self::Bind(error) => write!(f, "failed to bind Seismograph monitor to localhost: {error}"),
            Self::ConfigureListener(error) => write!(f, "failed to configure Seismograph monitor listener: {error}"),
            Self::ReadAddress(error) => write!(f, "failed to read Seismograph monitor address: {error}"),
            Self::Random(error) => write!(f, "failed to generate Seismograph monitor identity: {error}"),
            Self::Protocol(error) => write!(f, "Seismograph monitor protocol failed: {error}"),
            Self::CreateDirectory { path, source } => {
                write!(f, "failed to create monitor directory {}: {source}", path.display())
            }
            Self::SetPermissions { path, source } => {
                write!(f, "failed to secure monitor directory {}: {source}", path.display())
            }
            Self::Publish { path, source } => {
                write!(f, "failed to publish monitor descriptor {}: {source}", path.display())
            }
            Self::Rename { from, to, source } => {
                write!(
                    f,
                    "failed to publish monitor descriptor {} as {}: {source}",
                    from.display(),
                    to.display()
                )
            }
            Self::Spawn(error) => write!(f, "failed to start Seismograph monitor thread: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bind(error)
            | Self::ConfigureListener(error)
            | Self::ReadAddress(error)
            | Self::CreateDirectory { source: error, .. }
            | Self::SetPermissions { source: error, .. }
            | Self::Rename { source: error, .. }
            | Self::Spawn(error) => Some(error),
            Self::Protocol(error) | Self::Publish { source: error, .. } => Some(error),
            Self::Random(_) | Self::InvalidIdentity => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn connected_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        clippy::useless_let_if_seq,
        reason = "The end-to-end monitor scenario stays together; default initialization keeps impossible response-shape branches out of line coverage"
    )]
    fn monitor_configures_recording_and_returns_snapshot() {
        let _test = crate::recorder::TEST_LOCK.lock().unwrap();
        let monitor = Monitor::builder().name("monitor-test").instance("one").start().unwrap();
        let descriptor = monitor.descriptor().clone();
        assert!(
            seismograph_protocol::monitor_directory()
                .unwrap()
                .join(descriptor.file_name())
                .exists()
        );

        let mut stream = TcpStream::connect(descriptor.socket_address()).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        seismograph_protocol::write_request(
            &mut stream,
            1,
            &Request::Hello {
                authentication: descriptor.authentication,
            },
        )
        .unwrap();
        let response = seismograph_protocol::read_response(&mut stream)
            .unwrap_or_else(|error| panic!("{error}; server error: {:?}", monitor.last_error()));
        assert!(matches!(response, (1, Response::Hello { .. })));

        let cache_policy = seismograph_protocol::message::RecordingPolicy {
            enabled: true,
            sampling_one_in: 4,
            ..Default::default()
        };
        seismograph_protocol::write_request(&mut stream, 2, &Request::SetCacheRecording(cache_policy)).unwrap();
        assert_eq!(
            seismograph_protocol::read_response(&mut stream).unwrap(),
            (2, Response::Acknowledged)
        );

        seismograph_protocol::write_request(
            &mut stream,
            3,
            &Request::SetRecording(RecordingConfiguration {
                arc_dereferences: seismograph_protocol::message::RecordingPolicy {
                    enabled: true,
                    ..Default::default()
                },
                event_capacity_per_thread: 64,
                ..Default::default()
            }),
        )
        .unwrap();
        let response = seismograph_protocol::read_response(&mut stream)
            .unwrap_or_else(|error| panic!("{error}; server error: {:?}", monitor.last_error()));
        assert_eq!(response, (3, Response::Acknowledged));

        seismograph_protocol::write_request(&mut stream, 4, &Request::ReadCacheRecording).unwrap();
        assert_eq!(
            seismograph_protocol::read_response(&mut stream).unwrap(),
            (4, Response::CacheRecording(cache_policy))
        );
        crate::record(crate::recorder::event::EventClass::ArcDereference, || {
            crate::recorder::event::Record::object(
                crate::recorder::event::EventKind::ArcDeref,
                crate::recorder::event::ObjectId::new(42),
            )
        });

        seismograph_protocol::write_request(&mut stream, 5, &Request::ReadRecorderStatistics).unwrap();
        let response = seismograph_protocol::read_response(&mut stream).unwrap();
        assert!(matches!(response, (5, Response::RecorderStatistics(_))));
        let mut before = RecorderStatistics::default();
        if let (5, Response::RecorderStatistics(statistics)) = response {
            before = statistics;
        }

        seismograph_protocol::write_request(
            &mut stream,
            6,
            &Request::CaptureSnapshot(SnapshotOptions {
                event_buffers: EventBufferDisposition::Release,
            }),
        )
        .unwrap();
        let (request_id, response) = seismograph_protocol::read_response(&mut stream).unwrap();
        assert_eq!(request_id, 6);
        assert!(matches!(response, Response::Snapshot(_)));
        let mut bytes = Vec::new();
        if let Response::Snapshot(snapshot) = response {
            bytes = snapshot;
        }
        let decoded = crate::snapshot::decode(&bytes).unwrap();

        seismograph_protocol::write_request(&mut stream, 7, &Request::ReadRecorderStatistics).unwrap();
        let response = seismograph_protocol::read_response(&mut stream).unwrap();
        assert!(matches!(response, (7, Response::RecorderStatistics(_))));
        let mut after = RecorderStatistics::default();
        if let (7, Response::RecorderStatistics(statistics)) = response {
            after = statistics;
        }
        assert_eq!(
            (
                decoded
                    .events
                    .events
                    .iter()
                    .any(|event| event.object_id().is_some_and(|object_id| object_id.get() == 42)),
                after.retained_events,
                after.allocated_bytes < before.allocated_bytes,
            ),
            (true, 0, true)
        );

        crate::recorder(crate::recorder::Configuration::default());
        let path = seismograph_protocol::monitor_directory().unwrap().join(descriptor.file_name());
        drop(monitor);
        assert!(!path.exists());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn builder_identity_debug_and_last_error_are_exposed() {
        assert!(matches!(Monitor::builder().name("").start(), Err(Error::InvalidIdentity)));
        assert!(matches!(
            Monitor::builder().name("valid").instance("").start(),
            Err(Error::InvalidIdentity)
        ));
        assert!(!default_application_name().is_empty());

        let monitor = Monitor::builder().name("monitor-debug").start().unwrap();
        assert_eq!(monitor.last_error(), None);
        let debug = format!("{monitor:?}");
        assert!(debug.contains("Monitor"));
        assert!(debug.contains("descriptor_path"));
    }

    #[test]
    fn handshakes_are_required_and_authenticated() {
        let descriptor = MonitorDescriptor {
            name: "test".into(),
            instance: None,
            process_id: 1,
            instance_id: InstanceId::from_bytes([1; 16]),
            port: 1,
            authentication: AuthenticationToken::from_bytes([2; 32]),
        };

        let (mut client, server) = connected_pair();
        seismograph_protocol::write_request(&mut client, 1, &Request::ReadRecorderStatistics).unwrap();
        assert!(matches!(
            handle_client(server, &descriptor, &AtomicBool::new(false)),
            Err(ClientError::HandshakeRequired)
        ));

        let (mut client, server) = connected_pair();
        seismograph_protocol::write_request(
            &mut client,
            2,
            &Request::Hello {
                authentication: AuthenticationToken::from_bytes([3; 32]),
            },
        )
        .unwrap();
        assert!(matches!(
            handle_client(server, &descriptor, &AtomicBool::new(false)),
            Err(ClientError::Authentication)
        ));
        assert_eq!(
            seismograph_protocol::read_response(&mut client).unwrap(),
            (2, Response::Error("authentication failed".into()))
        );
    }

    #[test]
    fn authenticated_client_handles_repeat_hello_and_all_snapshot_modes() {
        let descriptor = MonitorDescriptor {
            name: "test".into(),
            instance: None,
            process_id: 1,
            instance_id: InstanceId::from_bytes([1; 16]),
            port: 1,
            authentication: AuthenticationToken::from_bytes([2; 32]),
        };
        let (mut client, server) = connected_pair();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let server_descriptor = descriptor.clone();
        let server_thread = thread::spawn(move || handle_client(server, &server_descriptor, &server_stop));

        seismograph_protocol::write_request(
            &mut client,
            1,
            &Request::Hello {
                authentication: descriptor.authentication,
            },
        )
        .unwrap();
        assert!(matches!(
            seismograph_protocol::read_response(&mut client).unwrap(),
            (1, Response::Hello { .. })
        ));

        seismograph_protocol::write_request(
            &mut client,
            2,
            &Request::Hello {
                authentication: descriptor.authentication,
            },
        )
        .unwrap();
        assert_eq!(
            seismograph_protocol::read_response(&mut client).unwrap(),
            (2, Response::Error("already authenticated".into()))
        );

        for (request_id, event_buffers) in [
            (3, EventBufferDisposition::Retain),
            (4, EventBufferDisposition::Clear),
            (5, EventBufferDisposition::Release),
        ] {
            seismograph_protocol::write_request(
                &mut client,
                request_id,
                &Request::CaptureSnapshot(SnapshotOptions { event_buffers }),
            )
            .unwrap();
            assert!(matches!(
                seismograph_protocol::read_response(&mut client).unwrap(),
                (response_id, Response::Snapshot(_)) if response_id == request_id
            ));
        }

        stop.store(true, Ordering::Release);
        client.shutdown(Shutdown::Both).unwrap();
        server_thread.join().unwrap().unwrap();
    }

    #[test]
    fn stopped_client_and_protocol_failures_are_reported() {
        let (mut client, mut server) = connected_pair();
        server.set_read_timeout(Some(Duration::from_millis(1))).unwrap();
        assert!(matches!(
            read_request_retry(&mut server, &AtomicBool::new(true)),
            Err(ClientError::Stopped)
        ));

        client.write_all(b"not a protocol frame").unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        assert!(matches!(
            read_request_retry(&mut server, &AtomicBool::new(false)),
            Err(ClientError::Protocol(_))
        ));

        let (client, mut server) = connected_pair();
        drop(client);
        assert!(matches!(
            read_request_retry(&mut server, &AtomicBool::new(false)),
            Err(ClientError::Disconnected)
        ));

        let (mut client, mut server) = connected_pair();
        server.set_read_timeout(Some(Duration::from_millis(1))).unwrap();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            seismograph_protocol::write_request(&mut client, 7, &Request::ReadRecorderStatistics).unwrap();
        });
        assert!(matches!(
            read_request_retry(&mut server, &AtomicBool::new(false)),
            Ok((7, Request::ReadRecorderStatistics))
        ));
        writer.join().unwrap();

        let (client, mut server) = connected_pair();
        server.shutdown(Shutdown::Read).unwrap();
        drop(client);
        assert!(matches!(
            read_request_retry(&mut server, &AtomicBool::new(true)),
            Err(ClientError::Stopped | ClientError::Disconnected)
        ));
    }

    #[test]
    fn conversion_helpers_preserve_policies_and_errors() {
        let policy = crate::recorder::RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            event_sampling: crate::recorder::EventSampling::one_in(7).unwrap(),
        };
        let configuration = protocol_recording_configuration(policy, policy, policy, policy, policy, policy, usize::MAX);
        assert_eq!(configuration.event_capacity_per_thread, u32::MAX);
        assert_eq!(configuration.allocations.sampling_one_in, 7);
        assert_eq!(recorder_policy(configuration.allocations), policy);
        assert!(tokens_equal(
            AuthenticationToken::from_bytes([1; 32]),
            AuthenticationToken::from_bytes([1; 32])
        ));
        assert!(!tokens_equal(
            AuthenticationToken::from_bytes([1; 32]),
            AuthenticationToken::from_bytes([2; 32])
        ));

        assert_eq!(
            snapshot_response(Err(crate::Error::new("capture failed"))),
            Response::Error("capture failed".into())
        );
        assert_eq!(io_error_action(io::ErrorKind::Other, false), IoErrorAction::Protocol);
        assert_eq!(io_error_action(io::ErrorKind::Other, true), IoErrorAction::Stopped);

        let mut attempts = 0;
        assert!(matches!(
            read_request_retry_with(&AtomicBool::new(false), || {
                attempts += 1;
                if attempts == 1 {
                    Err(seismograph_protocol::Error::Io(io::Error::new(io::ErrorKind::TimedOut, "retry")))
                } else {
                    Ok((1, Request::ReadRecorderStatistics))
                }
            }),
            Ok((1, Request::ReadRecorderStatistics))
        ));
        assert!(matches!(
            read_request_retry_with(&AtomicBool::new(false), || {
                Err(seismograph_protocol::Error::Io(io::Error::other("protocol")))
            }),
            Err(ClientError::Protocol(_))
        ));
    }

    #[test]
    fn client_and_startup_errors_have_descriptive_sources() {
        let io_error = || io::Error::other("socket failed");
        let protocol_error = || seismograph_protocol::Error::InvalidMessage;
        let client_cases = [
            ClientError::Io(io_error()),
            ClientError::Protocol(protocol_error()),
            ClientError::HandshakeRequired,
            ClientError::Authentication,
            ClientError::Disconnected,
            ClientError::Stopped,
        ];
        let expected = [
            "socket failed",
            "invalid Seismograph monitor message",
            "monitor handshake is required",
            "monitor authentication failed",
            "monitor client disconnected",
            "monitor stopped",
        ];
        for (error, expected) in client_cases.into_iter().zip(expected) {
            assert_eq!(error.to_string(), expected);
        }

        let path = PathBuf::from("monitor");
        let startup_cases = [
            Error::InvalidIdentity,
            Error::Bind(io_error()),
            Error::ConfigureListener(io_error()),
            Error::ReadAddress(io_error()),
            Error::Random(getrandom::Error::UNSUPPORTED),
            Error::Protocol(protocol_error()),
            Error::CreateDirectory {
                path: path.clone(),
                source: io_error(),
            },
            Error::SetPermissions {
                path: path.clone(),
                source: io_error(),
            },
            Error::Publish {
                path: path.clone(),
                source: protocol_error(),
            },
            Error::Rename {
                from: PathBuf::from("temporary"),
                to: path,
                source: io_error(),
            },
            Error::Spawn(io_error()),
        ];
        for error in startup_cases {
            let _message = error.to_string();
            let expected_source = !matches!(error, Error::InvalidIdentity | Error::Random(_));
            assert_eq!(std::error::Error::source(&error).is_some(), expected_source);
        }
    }

    #[test]
    fn monitor_directory_creation_failure_preserves_path() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join(format!("seismograph-monitor-file-{}", std::process::id()));
        fs::write(&path, b"not a directory").unwrap();
        let error = create_monitor_directory(&path).unwrap_err();
        fs::remove_file(&path).unwrap();
        assert!(matches!(error, Error::CreateDirectory { path: failed, .. } if failed == path));
    }

    #[test]
    fn descriptor_publication_reports_write_and_rename_failures() {
        let descriptor = MonitorDescriptor {
            name: "test".into(),
            instance: None,
            process_id: 1,
            instance_id: InstanceId::from_bytes([1; 16]),
            port: 1,
            authentication: AuthenticationToken::from_bytes([2; 32]),
        };
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join(format!("seismograph-publish-{}", std::process::id()));
        let missing = base.join("missing");
        assert!(matches!(publish_descriptor(&missing, &descriptor), Err(Error::Publish { .. })));

        fs::create_dir_all(&base).unwrap();
        let destination = base.join(descriptor.file_name());
        fs::create_dir(&destination).unwrap();
        assert!(matches!(publish_descriptor(&base, &descriptor), Err(Error::Rename { .. })));
        fs::remove_dir(destination).unwrap();
        let _ = fs::remove_file(base.join(descriptor.file_name()).with_extension("monitor.tmp"));
        fs::remove_dir(base).unwrap();
    }

    #[test]
    fn listener_spawn_failure_removes_published_descriptor() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join(format!("seismograph-spawn-{}", std::process::id()));
        fs::write(&path, b"descriptor").unwrap();
        assert!(matches!(
            spawn_listener_thread_with(|| {}, &path, |_operation| Err(io::Error::other("injected"))),
            Err(Error::Spawn(_))
        ));
        assert!(!path.exists());
    }

    #[test]
    fn builder_propagates_listener_spawn_failure() {
        let error = Monitor::builder()
            .name("spawn-failure")
            .start_with(|_listener, _descriptor, _stop, _client, _last_error, descriptor_path| {
                fs::remove_file(descriptor_path).unwrap();
                Err(Error::Spawn(io::Error::other("injected")))
            })
            .unwrap_err();
        assert!(matches!(error, Error::Spawn(_)));
    }

    #[test]
    fn authenticated_client_stops_after_handshake_and_propagates_bad_frames() {
        let descriptor = MonitorDescriptor {
            name: "test".into(),
            instance: None,
            process_id: 1,
            instance_id: InstanceId::from_bytes([1; 16]),
            port: 1,
            authentication: AuthenticationToken::from_bytes([2; 32]),
        };

        let (mut client, server) = connected_pair();
        seismograph_protocol::write_request(
            &mut client,
            1,
            &Request::Hello {
                authentication: descriptor.authentication,
            },
        )
        .unwrap();
        handle_client(server, &descriptor, &AtomicBool::new(true)).unwrap();
        assert!(matches!(
            seismograph_protocol::read_response(&mut client).unwrap(),
            (1, Response::Hello { .. })
        ));

        let (mut client, server) = connected_pair();
        let server_descriptor = descriptor.clone();
        let server_thread = thread::spawn(move || handle_client(server, &server_descriptor, &AtomicBool::new(false)));
        seismograph_protocol::write_request(
            &mut client,
            2,
            &Request::Hello {
                authentication: descriptor.authentication,
            },
        )
        .unwrap();
        let _hello = seismograph_protocol::read_response(&mut client).unwrap();
        client.write_all(&[0; 20]).unwrap();
        assert!(matches!(server_thread.join().unwrap(), Err(ClientError::Protocol(_))));
    }
}
