// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Feature-gated local monitor server.
//!
//! Each monitor publishes a Unix socket or local Windows named pipe in the
//! user's private registry. The owning user and root/elevated administrators
//! can connect through OS access control; no authentication token is needed.
//!
//! Startup initializes the private registry without elevation. Unix prefers
//! `$XDG_RUNTIME_DIR/seismograph` with a private temporary fallback; Windows
//! uses local application data. Existing unsafe permissions are rejected.

use std::collections::HashMap;
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use std::{fmt, io};

use seismograph_protocol::discovery::Directory;
use seismograph_protocol::io::MonitorIo;
#[cfg(test)]
use seismograph_protocol::message::SnapshotOptions;
use seismograph_protocol::message::{
    EventBufferDisposition, EventClassCounts, RecorderActivity, RecorderStatistics, RecordingConfiguration, Request, Response,
    ThreadRecorderStatistics,
};
#[cfg(test)]
use seismograph_protocol::monitor::InstanceId;
use seismograph_protocol::monitor::{MonitorDescriptor, MonitorEndpoint};
use seismograph_protocol::transport::{Connection, Listener};

use crate::recorder::SuppressionGuard;

const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(50);
const CLIENT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
// Abandoned interactive clients should release their worker and socket promptly.
const CLIENT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
// Bound detached worker and socket overhead per monitored process.
const MAX_CONCURRENT_CLIENTS: usize = 16;
static SNAPSHOT_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Running OS-authorized local monitor.
pub struct Monitor {
    descriptor: MonitorDescriptor,
    descriptor_path: PathBuf,
    io: MonitorIo,
    stop: Arc<AtomicBool>,
    active_clients: Arc<ActiveClients>,
    last_error: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
    _directory: Directory,
    #[cfg(any(test, feature = "test-util"))]
    _fixture: Option<seismograph_protocol::testing::Directory>,
}

impl Monitor {
    /// Creates a monitor builder.
    #[must_use]
    pub const fn builder() -> Builder {
        Builder {
            name: None,
            instance: None,
            io: None,
            #[cfg(any(test, feature = "test-util"))]
            isolated: false,
        }
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
        self.active_clients.shutdown_all();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("seismograph: monitor listener thread panicked");
        }
        if let Err(error) = self.io.remove_descriptor(&self.descriptor_path) {
            eprintln!("seismograph: failed to remove monitor descriptor: {error}");
        }
    }
}

/// Builder for an OS-authorized local monitor.
#[derive(Debug, Default)]
pub struct Builder {
    name: Option<String>,
    instance: Option<String>,
    io: Option<MonitorIo>,
    #[cfg(any(test, feature = "test-util"))]
    isolated: bool,
}

impl Builder {
    /// Selects the I/O capability shared by this monitor and its workers.
    ///
    /// Omitting this uses native local I/O. Enable `seismograph_protocol`'s
    /// `test-util` feature on a dev-dependency to inject a mocked instance
    /// with deterministic errors and controlled deadlines.
    /// Replaces a previously selected isolated native fixture.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use seismograph::monitor::Monitor;
    /// use seismograph_protocol::io::MonitorIo;
    ///
    /// let monitor = Monitor::builder()
    ///     .name("worker")
    ///     .io(MonitorIo::new_native())
    ///     .start()?;
    /// # Ok::<(), seismograph::monitor::Error>(())
    /// ```
    #[must_use]
    pub fn io(mut self, io: MonitorIo) -> Self {
        self.io = Some(io);
        #[cfg(any(test, feature = "test-util"))]
        {
            self.isolated = false;
        }
        self
    }

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

    /// Starts the local listener and publishes its discovery descriptor.
    ///
    /// # Errors
    ///
    /// Returns an error when user storage is unavailable, ownership or
    /// permissions are unsafe, or identity generation, binding, discovery
    /// publication, or listener-thread creation fails.
    ///
    /// # Panics
    ///
    /// With `test-util`, panics if an explicitly selected isolated native test
    /// directory cannot be created or secured.
    pub fn start(self) -> Result<Monitor, Error> {
        self.start_with(spawn_listener_thread)
    }

    /// Uses an isolated, correctly protected temporary registry for native tests.
    /// Replaces a previously selected I/O capability with native I/O.
    #[cfg(any(test, feature = "test-util"))]
    #[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
    #[must_use]
    pub fn isolated(mut self) -> Self {
        self.isolated = true;
        self.io = None;
        self
    }

    fn start_with(
        self,
        spawn: impl FnOnce(
            MonitorIo,
            Listener,
            MonitorDescriptor,
            Arc<AtomicBool>,
            Arc<ActiveClients>,
            Arc<Mutex<Option<String>>>,
            &Path,
        ) -> Result<JoinHandle<()>, Error>,
    ) -> Result<Monitor, Error> {
        let io = self.io.unwrap_or_else(MonitorIo::new_native);
        let name = match self.name {
            Some(name) => name,
            None => default_application_name(&io)?,
        };
        if name.is_empty() || self.instance.as_ref().is_some_and(String::is_empty) {
            return Err(Error::InvalidIdentity);
        }

        let instance_id = io.instance_id().map_err(Error::Protocol)?;
        #[cfg(any(test, feature = "test-util"))]
        let fixture = self.isolated.then(seismograph_protocol::testing::Directory::new);
        #[cfg(any(test, feature = "test-util"))]
        let directory = match &fixture {
            Some(fixture) => fixture.open(),
            None => Directory::current(&io).map_err(Error::Protocol)?,
        };
        #[cfg(not(any(test, feature = "test-util")))]
        let directory = Directory::current(&io).map_err(Error::Protocol)?;
        let process_id = io.process_id();
        let owner = directory.owner().clone();
        let descriptor = MonitorDescriptor {
            name,
            instance: self.instance,
            process_id,
            instance_id,
            endpoint: MonitorEndpoint::for_instance(directory.path(), &owner, process_id, instance_id),
            owner,
        };
        let listener = Listener::bind(&io, &descriptor).map_err(Error::Bind)?;
        listener.set_nonblocking(true).map_err(Error::ConfigureListener)?;
        let descriptor_path = io.publish_descriptor(&directory, &descriptor).map_err(Error::Protocol)?;

        let stop = Arc::new(AtomicBool::new(false));
        let active_clients = Arc::new(ActiveClients::new());
        let last_error = Arc::new(Mutex::new(None));
        let thread = match spawn(
            io.clone(),
            listener,
            descriptor.clone(),
            Arc::clone(&stop),
            Arc::clone(&active_clients),
            Arc::clone(&last_error),
            &descriptor_path,
        ) {
            Ok(thread) => thread,
            Err(error) => {
                if let Err(cleanup) = io.remove_descriptor(&descriptor_path) {
                    eprintln!("seismograph: failed to clean up unpublished monitor: {cleanup}");
                }
                return Err(error);
            }
        };

        Ok(Monitor {
            descriptor,
            descriptor_path,
            io,
            stop,
            active_clients,
            last_error,
            thread: Some(thread),
            _directory: directory,
            #[cfg(any(test, feature = "test-util"))]
            _fixture: fixture,
        })
    }
}

fn default_application_name(io: &MonitorIo) -> Result<String, Error> {
    let path = io
        .current_exe()
        .map_err(|source| Error::Protocol(seismograph_protocol::Error::Io(source)))?;
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or(Error::InvalidIdentity)
}

struct ActiveClients {
    state: Mutex<ActiveClientsState>,
}

struct ActiveClientsState {
    accepting: bool,
    next_id: u64,
    streams: HashMap<u64, Connection>,
}

impl ActiveClients {
    fn new() -> Self {
        Self {
            state: Mutex::new(ActiveClientsState {
                accepting: true,
                next_id: 0,
                streams: HashMap::new(),
            }),
        }
    }

    fn register(self: &Arc<Self>, stream: &Connection) -> Result<Option<ActiveClient>, io::Error> {
        let tracked_stream = stream.try_clone()?;
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.accepting || state.streams.len() >= MAX_CONCURRENT_CLIENTS {
            return Ok(None);
        }
        let id = state.next_id;
        state.next_id = state
            .next_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("monitor client identifier space exhausted"))?;
        state.streams.insert(id, tracked_stream);
        Ok(Some(ActiveClient {
            id,
            clients: Arc::clone(self),
        }))
    }

    fn shutdown_all(&self) {
        let streams = {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            state.accepting = false;
            state.streams.drain().map(|(_id, stream)| stream).collect::<Vec<_>>()
        };
        for stream in streams {
            if let Err(error) = stream.close() {
                eprintln!("seismograph: failed to close monitor client: {error}");
            }
        }
    }
}

struct ActiveClient {
    id: u64,
    clients: Arc<ActiveClients>,
}

impl Drop for ActiveClient {
    fn drop(&mut self) {
        self.clients
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .streams
            .remove(&self.id);
    }
}

fn run_listener(
    io: &MonitorIo,
    listener: &Listener,
    descriptor: &MonitorDescriptor,
    stop: &Arc<AtomicBool>,
    active_clients: &Arc<ActiveClients>,
    last_error: &Arc<Mutex<Option<String>>>,
) {
    let _suppression = SuppressionGuard::enter();
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        match listener.accept() {
            Ok(stream) => {
                let registration = match active_clients.register(&stream) {
                    Ok(Some(registration)) => registration,
                    Ok(None) => {
                        if let Err(error) = stream.close() {
                            set_last_error(last_error, &error);
                        }
                        continue;
                    }
                    Err(error) => {
                        set_last_error(last_error, &error);
                        continue;
                    }
                };
                spawn_client_thread(io.clone(), stream, descriptor.clone(), Arc::clone(stop), registration, last_error);
            }
            Err(error) => {
                if error.kind() == io::ErrorKind::PermissionDenied {
                    set_last_error(last_error, &error);
                } else if listener_accept_should_retry(error.kind()) {
                    io.wait(ACCEPT_RETRY_DELAY);
                } else {
                    set_last_error(last_error, &error);
                    break;
                }
            }
        }
    }
}

const fn listener_accept_should_retry(kind: io::ErrorKind) -> bool {
    matches!(kind, io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted)
}

fn spawn_listener_thread(
    io: MonitorIo,
    listener: Listener,
    descriptor: MonitorDescriptor,
    stop: Arc<AtomicBool>,
    active_clients: Arc<ActiveClients>,
    last_error: Arc<Mutex<Option<String>>>,
    _descriptor_path: &Path,
) -> Result<JoinHandle<()>, Error> {
    thread::Builder::new()
        .name("seismograph-monitor".into())
        .spawn(move || run_listener(&io, &listener, &descriptor, &stop, &active_clients, &last_error))
        .map_err(Error::Spawn)
}

fn spawn_client_thread(
    io: MonitorIo,
    stream: Connection,
    descriptor: MonitorDescriptor,
    stop: Arc<AtomicBool>,
    registration: ActiveClient,
    last_error: &Arc<Mutex<Option<String>>>,
) {
    spawn_client_thread_with(io, stream, descriptor, stop, registration, last_error, |operation| {
        thread::Builder::new().name("seismograph-monitor-client".into()).spawn(operation)
    });
}

fn spawn_client_thread_with(
    io: MonitorIo,
    stream: Connection,
    descriptor: MonitorDescriptor,
    stop: Arc<AtomicBool>,
    registration: ActiveClient,
    last_error: &Arc<Mutex<Option<String>>>,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<JoinHandle<()>>,
) {
    let worker_last_error = Arc::clone(last_error);
    let spawn_result = spawn(Box::new(move || {
        let _registration = registration;
        let _suppression = SuppressionGuard::enter();
        if let Err(error) = handle_client(&io, stream, &descriptor, &stop) {
            set_last_error(&worker_last_error, &error);
        }
    }));
    if let Err(error) = spawn_result {
        set_last_error(last_error, &error);
    }
}

fn set_last_error(last_error: &Mutex<Option<String>>, error: &impl fmt::Display) {
    *last_error.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.to_string());
}

fn handle_client(io: &MonitorIo, stream: Connection, descriptor: &MonitorDescriptor, stop: &AtomicBool) -> Result<(), ClientError> {
    handle_client_with_timeouts(io, stream, descriptor, stop, CLIENT_HANDSHAKE_TIMEOUT, CLIENT_IDLE_TIMEOUT)
}

fn handle_client_with_timeouts(
    io: &MonitorIo,
    mut stream: Connection,
    descriptor: &MonitorDescriptor,
    stop: &AtomicBool,
    handshake_timeout: Duration,
    idle_timeout: Duration,
) -> Result<(), ClientError> {
    stream.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT)).map_err(ClientError::Io)?;

    let (request_id, request) = read_request_with_timeout(io, &mut stream, stop, handshake_timeout, ReadTimeout::Handshake)?;
    let Request::Hello = request else {
        return Err(ClientError::HandshakeRequired);
    };
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
                configuration.event_capacity_per_thread.get(),
            ),
        },
    )
    .map_err(ClientError::Protocol)?;

    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let (request_id, request) = match read_request_with_timeout(io, &mut stream, stop, idle_timeout, ReadTimeout::Idle) {
            Ok(request) => request,
            Err(ClientError::Stopped | ClientError::Disconnected | ClientError::IdleTimedOut) => return Ok(()),
            Err(error) => return Err(error),
        };
        let response = connected_response_with_guard(&request);
        let mut writer = DeadlineWriter::new(io, &mut stream, CLIENT_WRITE_TIMEOUT);
        seismograph_protocol::write_response(&mut writer, request_id, &response.response).map_err(ClientError::Protocol)?;
    }
    Ok(())
}

struct DeadlineWriter<'a> {
    io: MonitorIo,
    stream: &'a mut Connection,
    deadline: Instant,
}

impl<'a> DeadlineWriter<'a> {
    fn new(io: &MonitorIo, stream: &'a mut Connection, timeout: Duration) -> Self {
        Self {
            io: io.clone(),
            stream,
            deadline: io.instant() + timeout,
        }
    }

    fn apply_remaining_timeout(&self) -> io::Result<()> {
        let remaining = self
            .deadline
            .checked_duration_since(self.io.instant())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Seismograph monitor response write timed out"))?;
        self.stream.set_write_timeout(Some(remaining))
    }
}

impl io::Write for DeadlineWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.apply_remaining_timeout()?;
        io::Write::write(self.stream, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.apply_remaining_timeout()?;
        io::Write::flush(self.stream)
    }
}

#[cfg(test)]
fn connected_response(request: &Request) -> Response {
    connected_response_with_guard(request).response
}

struct ConnectedResponse {
    response: Response,
    _snapshot_guard: Option<SnapshotRequestGuard>,
}

fn connected_response_with_guard(request: &Request) -> ConnectedResponse {
    match request {
        Request::ReadServerVersion => ConnectedResponse {
            response: Response::ServerVersion(env!("CARGO_PKG_VERSION").into()),
            _snapshot_guard: None,
        },
        Request::CaptureSnapshotAndStop => {
            let Ok(guard) = SnapshotRequestGuard::acquire() else {
                return ConnectedResponse {
                    response: Response::Error("a seismograph snapshot is already in progress".into()),
                    _snapshot_guard: None,
                };
            };
            ConnectedResponse {
                response: snapshot_response(crate::snapshot(crate::snapshot::SnapshotOptions {
                    event_buffers: crate::snapshot::EventBufferDisposition::Stop,
                })),
                _snapshot_guard: Some(guard),
            }
        }
        Request::ClearEventBuffers => {
            let Ok(_snapshot) = SnapshotRequestGuard::acquire() else {
                return ConnectedResponse {
                    response: Response::Error("a seismograph snapshot is already in progress".into()),
                    _snapshot_guard: None,
                };
            };
            ConnectedResponse {
                response: crate::recorder::clear_event_buffers()
                    .map_or_else(|error| Response::Error(error.to_string()), |()| Response::Acknowledged),
                _snapshot_guard: None,
            }
        }
        Request::Hello => ConnectedResponse {
            response: Response::Error("handshake already completed".into()),
            _snapshot_guard: None,
        },
        Request::SetRecording(configuration) => ConnectedResponse {
            response: apply_recording_configuration(*configuration)
                .map_or_else(|error| Response::Error(error.to_string()), |()| Response::Acknowledged),
            _snapshot_guard: None,
        },
        Request::SetCacheRecording(policy) => {
            let configuration = crate::recorder::configuration();
            ConnectedResponse {
                response: crate::recorder::try_configure(crate::recorder::Configuration {
                    cache: recorder_policy(*policy),
                    ..configuration
                })
                .map_or_else(|error| Response::Error(error.to_string()), |()| Response::Acknowledged),
                _snapshot_guard: None,
            }
        }
        Request::CaptureSnapshot(options) => {
            let Ok(guard) = SnapshotRequestGuard::acquire() else {
                return ConnectedResponse {
                    response: Response::Error("a seismograph snapshot is already in progress".into()),
                    _snapshot_guard: None,
                };
            };
            let event_buffers = match options.event_buffers {
                EventBufferDisposition::Retain => crate::snapshot::EventBufferDisposition::Retain,
                EventBufferDisposition::Clear => crate::snapshot::EventBufferDisposition::Clear,
                EventBufferDisposition::Release => crate::snapshot::EventBufferDisposition::Release,
            };
            ConnectedResponse {
                response: snapshot_response(crate::snapshot(crate::snapshot::SnapshotOptions { event_buffers })),
                _snapshot_guard: Some(guard),
            }
        }
        Request::ReadRecorderStatistics => ConnectedResponse {
            response: recorder_statistics_response(),
            _snapshot_guard: None,
        },
        Request::ReadRecorderActivity => ConnectedResponse {
            response: recorder_activity_response(),
            _snapshot_guard: None,
        },
        Request::ReadCacheRecording => {
            let policy = protocol_recording_policy(crate::recorder::configuration().cache);
            ConnectedResponse {
                response: Response::CacheRecording(policy),
                _snapshot_guard: None,
            }
        }
    }
}

fn apply_recording_configuration(configuration: RecordingConfiguration) -> Result<(), crate::Error> {
    let event_capacity_per_thread = crate::recorder::EventBufferCapacity::new(
        usize::try_from(configuration.event_capacity_per_thread).expect("Seismograph requires a target whose usize can represent u32"),
    )
    .expect("the monitor protocol decoder validates event buffer capacity");
    let current = crate::recorder::configuration();
    crate::recorder::try_configure(crate::recorder::Configuration {
        allocations: recorder_policy(configuration.allocations),
        general_events: recorder_policy(configuration.general_events),
        arc_dereferences: recorder_policy(configuration.arc_dereferences),
        runtime_tasks: recorder_policy(configuration.runtime_tasks),
        io: recorder_policy(configuration.io),
        cache: current.cache,
        event_capacity_per_thread,
    })
}

fn recorder_statistics_response() -> Response {
    recorder_statistics_response_from(crate::recorder::try_statistics())
}

fn recorder_statistics_response_from(result: Result<crate::recorder::Statistics, crate::Error>) -> Response {
    let statistics = match result {
        Ok(statistics) => statistics,
        Err(error) => return Response::Error(error.to_string()),
    };
    Response::RecorderStatistics(protocol_recorder_statistics(statistics))
}

fn recorder_activity_response() -> Response {
    recorder_activity_response_from(crate::recorder::try_activity())
}

fn recorder_activity_response_from(result: Result<crate::recorder::Activity, crate::Error>) -> Response {
    let activity = match result {
        Ok(activity) => activity,
        Err(error) => return Response::Error(error.to_string()),
    };
    let [allocations, general_events, arc_dereferences, runtime_tasks, io, cache] = activity.class_events;
    let mut statistics = protocol_recorder_statistics(activity.statistics);
    statistics.recording.cache = protocol_recording_policy(activity.statistics.recording.cache);
    Response::RecorderActivity(RecorderActivity {
        statistics,
        session_id: activity.session_id,
        class_events: EventClassCounts {
            allocations,
            general_events,
            arc_dereferences,
            runtime_tasks,
            io,
            cache,
        },
        threads: activity
            .threads
            .into_iter()
            .map(|thread| ThreadRecorderStatistics {
                thread_id: thread.thread_id.get(),
                name: thread.name,
                total_events: thread.total_events,
                retained_events: thread.retained_events,
                lost_events: thread.lost_events,
                event_capacity: thread.event_capacity,
                retired: thread.retired,
            })
            .collect(),
    })
}

fn protocol_recorder_statistics(statistics: crate::recorder::Statistics) -> RecorderStatistics {
    RecorderStatistics {
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
            usize::try_from(statistics.event_capacity_per_thread).unwrap_or(usize::MAX),
        ),
    }
}

struct SnapshotRequestGuard;

impl SnapshotRequestGuard {
    fn acquire() -> Result<Self, ()> {
        SNAPSHOT_IN_PROGRESS
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_previous| Self)
            .map_err(|_active| ())
    }
}

impl Drop for SnapshotRequestGuard {
    fn drop(&mut self) {
        SNAPSHOT_IN_PROGRESS.store(false, Ordering::Release);
    }
}

fn protocol_recording_configuration(
    allocations: crate::recorder::RecordingPolicy,
    general_events: crate::recorder::RecordingPolicy,
    arc_dereferences: crate::recorder::RecordingPolicy,
    runtime_tasks: crate::recorder::RecordingPolicy,
    io: crate::recorder::RecordingPolicy,
    event_capacity_per_thread: usize,
) -> RecordingConfiguration {
    RecordingConfiguration {
        allocations: protocol_recording_policy(allocations),
        general_events: protocol_recording_policy(general_events),
        arc_dereferences: protocol_recording_policy(arc_dereferences),
        runtime_tasks: protocol_recording_policy(runtime_tasks),
        io: protocol_recording_policy(io),
        cache: seismograph_protocol::message::RecordingPolicy::default(),
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
        Ok(snapshot) => Response::Snapshot(snapshot.into_bytes()),
        Err(error) => Response::Error(error.to_string()),
    }
}

struct DeadlineReader<'a> {
    io: MonitorIo,
    stream: &'a mut Connection,
    deadline: Instant,
}

impl io::Read for DeadlineReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(self.io.instant());
        if remaining.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "monitor frame read timed out"));
        }
        self.stream.set_read_timeout(Some(remaining))?;
        io::Read::read(self.stream, buf)
    }
}

fn read_request_with_timeout(
    io: &MonitorIo,
    stream: &mut Connection,
    stop: &AtomicBool,
    timeout: Duration,
    timeout_kind: ReadTimeout,
) -> Result<(u64, Request), ClientError> {
    let deadline = io
        .instant()
        .checked_add(timeout)
        .ok_or_else(|| ClientError::Io(io::Error::new(io::ErrorKind::InvalidInput, "monitor read timeout is too large")))?;
    let mut reader = DeadlineReader {
        io: io.clone(),
        stream,
        deadline,
    };
    seismograph_protocol::read_request(&mut reader).map_err(|error| classify_read_error(error, stop.load(Ordering::Acquire), timeout_kind))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadTimeout {
    Handshake,
    Idle,
}

fn classify_read_error(error: seismograph_protocol::Error, stopped: bool, timeout: ReadTimeout) -> ClientError {
    let seismograph_protocol::Error::Io(io_error) = error else {
        return ClientError::Protocol(error);
    };
    let kind = io_error.kind();
    if matches!(kind, io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) {
        if stopped {
            ClientError::Stopped
        } else {
            match timeout {
                ReadTimeout::Handshake => ClientError::HandshakeTimedOut,
                ReadTimeout::Idle => ClientError::IdleTimedOut,
            }
        }
    } else if matches!(
        kind,
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::NotConnected
    ) {
        ClientError::Disconnected
    } else if stopped {
        ClientError::Stopped
    } else {
        ClientError::Protocol(seismograph_protocol::Error::Io(io_error))
    }
}

#[derive(Debug)]
enum ClientError {
    Io(io::Error),
    Protocol(seismograph_protocol::Error),
    HandshakeRequired,
    HandshakeTimedOut,
    IdleTimedOut,
    Disconnected,
    Stopped,
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Protocol(error) => write!(f, "{error}"),
            Self::HandshakeRequired => f.write_str("monitor handshake is required"),
            Self::HandshakeTimedOut => f.write_str("monitor handshake timed out"),
            Self::IdleTimedOut => f.write_str("monitor client idle timeout expired"),
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
    /// Native local listener could not be bound.
    Bind(io::Error),
    /// Listener socket options could not be configured.
    ConfigureListener(io::Error),
    /// Secure random identity generation failed.
    Random(getrandom::Error),
    /// Protocol discovery operation failed.
    Protocol(seismograph_protocol::Error),
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
            Self::Bind(error) => write!(f, "failed to bind Seismograph local monitor: {error}"),
            Self::ConfigureListener(error) => write!(f, "failed to configure Seismograph monitor listener: {error}"),
            Self::Random(error) => write!(f, "failed to generate Seismograph monitor identity: {error}"),
            Self::Protocol(error) => write!(f, "Seismograph monitor protocol failed: {error}"),
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
            Self::Bind(error) | Self::ConfigureListener(error) | Self::Rename { source: error, .. } | Self::Spawn(error) => Some(error),
            Self::Protocol(error) | Self::Publish { source: error, .. } => Some(error),
            Self::Random(_) | Self::InvalidIdentity => None,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod io_tests;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io::{Read as _, Write as _};

    use super::*;

    #[cfg_attr(coverage_nightly, coverage(off))] // Test assertion adapter; callers exercise only the expected protocol variant.
    fn recorder_activity(response: Response) -> RecorderActivity {
        let Response::RecorderActivity(activity) = response else {
            panic!("authenticated activity request must return live counters");
        };
        activity
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // Test assertion adapter; callers exercise only the expected protocol variant.
    fn recorder_statistics(response: &Response) -> seismograph_protocol::message::RecorderStatistics {
        let Response::RecorderStatistics(statistics) = response else {
            panic!("expected recorder statistics");
        };
        *statistics
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // Test assertion adapter; callers exercise only the expected protocol variant.
    fn snapshot_bytes(response: Response) -> Vec<u8> {
        let Response::Snapshot(bytes) = response else {
            panic!("stop must return a snapshot");
        };
        bytes
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn connected_activity_reports_live_threads_without_snapshot_capture() {
        use crate::recorder::event::{EventClass, EventKind, ObjectId, Record};
        use crate::recorder::{Configuration, EventBufferCapacity, RecordingPolicy};

        let _test = crate::recorder::TEST_LOCK.lock().unwrap();
        crate::recorder::configure(Configuration {
            general_events: RecordingPolicy::all(false),
            cache: RecordingPolicy::all(false),
            event_capacity_per_thread: EventBufferCapacity::new(64).unwrap(),
            ..Default::default()
        });
        crate::recorder::clear_event_buffers().unwrap();
        crate::record(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
        });
        crate::record(EventClass::Cache, || Some(Record::object(EventKind::CacheHit, ObjectId::new(1))));
        let thread_id = crate::recorder::current_thread_id().get();
        let before = crate::recorder::try_activity().unwrap();
        let _snapshot = SnapshotRequestGuard::acquire().unwrap();
        let (mut client, server) = connected_pair();
        let descriptor = test_descriptor();

        let worker = thread::spawn(move || handle_client(&MonitorIo::new_native(), server, &descriptor, &AtomicBool::new(false)));
        seismograph_protocol::write_request(&mut client, 1, &Request::Hello).unwrap();
        assert!(matches!(
            seismograph_protocol::read_response(&mut client).unwrap(),
            (1, Response::Hello { .. })
        ));
        seismograph_protocol::write_request(&mut client, 2, &Request::ReadRecorderActivity).unwrap();
        let (request_id, response) = seismograph_protocol::read_response(&mut client).unwrap();
        let activity = recorder_activity(response);
        assert_eq!(
            (
                request_id,
                activity.session_id,
                activity.statistics.total_events,
                activity.statistics.recording.cache.enabled,
                activity.class_events,
                activity.threads,
            ),
            (
                2,
                before.session_id,
                2,
                true,
                EventClassCounts {
                    general_events: 1,
                    cache: 1,
                    ..Default::default()
                },
                vec![ThreadRecorderStatistics {
                    thread_id,
                    name: before.threads[0].name.clone(),
                    total_events: 2,
                    retained_events: 2,
                    lost_events: 0,
                    event_capacity: 64,
                    retired: false,
                }],
            )
        );
        let after = crate::recorder::try_activity().unwrap();
        assert_eq!((after.statistics, after.threads), (before.statistics, before.threads));
        client.close().unwrap();
        worker.join().unwrap().unwrap();
        crate::recorder::configure(Configuration::default());
        crate::recorder::clear_event_buffers().unwrap();
    }

    #[test]
    fn stopped_runtime_recording_retains_session_policies_and_events() {
        use crate::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp, Record};
        use crate::recorder::runtime::{RuntimeEvent, RuntimeId};

        let _test = crate::recorder::TEST_LOCK.lock().unwrap();
        let configuration = RecordingConfiguration {
            runtime_tasks: seismograph_protocol::message::RecordingPolicy {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(connected_response(&Request::SetRecording(configuration)), Response::Acknowledged);
        crate::record(EventClass::RuntimeTask, || {
            Some(Record::runtime(
                EventTimestamp::now(),
                EventKind::TaskPollStarted,
                RuntimeEvent {
                    runtime_id: RuntimeId::from_raw(1).unwrap(),
                    worker_id: None,
                    subject_id: 1,
                    related_id: 0,
                    value_0: 0,
                    value_1: 0,
                },
                BacktraceCapture::Never,
            ))
        });
        assert_eq!(
            connected_response(&Request::SetRecording(RecordingConfiguration::default())),
            Response::Acknowledged
        );
        let statistics = recorder_statistics(&connected_response(&Request::ReadRecorderStatistics));
        let snapshot = crate::snapshot(crate::snapshot::SnapshotOptions::default()).unwrap();
        let decoded = crate::snapshot::decode(snapshot.as_bytes()).unwrap();
        assert_eq!(
            (
                statistics.recording,
                statistics.retained_events,
                decoded.events.recording.runtime_tasks.enabled,
                decoded.events.events.len(),
                crate::recorder::configuration().runtime_tasks.enabled,
            ),
            (configuration, 1, true, 1, false)
        );
    }

    #[test]
    fn concurrent_snapshot_is_rejected_until_the_current_request_finishes() {
        let _test = crate::recorder::TEST_LOCK.lock().unwrap();
        let active = connected_response_with_guard(&Request::CaptureSnapshot(SnapshotOptions::default()));
        assert!(matches!(active.response, Response::Snapshot(_)));
        let rejected = [
            connected_response(&Request::CaptureSnapshot(SnapshotOptions::default())),
            connected_response(&Request::CaptureSnapshotAndStop),
            connected_response(&Request::ClearEventBuffers),
        ];
        drop(active);
        let resumed = connected_response(&Request::CaptureSnapshot(SnapshotOptions::default()));
        assert_eq!(
            (rejected, matches!(resumed, Response::Snapshot(_))),
            (
                [
                    Response::Error("a seismograph snapshot is already in progress".into()),
                    Response::Error("a seismograph snapshot is already in progress".into()),
                    Response::Error("a seismograph snapshot is already in progress".into()),
                ],
                true,
            )
        );
    }

    #[test]
    fn recorder_statistics_failures_are_returned_to_the_client() {
        let response = recorder_statistics_response_from(Err(crate::Error::new("statistics unavailable")));

        assert_eq!(response, Response::Error("statistics unavailable".into()));
    }

    #[test]
    fn recorder_activity_failures_are_returned_to_the_client() {
        let response = recorder_activity_response_from(Err(crate::Error::new("activity unavailable")));

        assert_eq!(response, Response::Error("activity unavailable".into()));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn expired_frame_deadline_does_not_consume_a_buffered_request() {
        let (mut client, mut server) = connected_pair();
        seismograph_protocol::write_request(&mut client, 1, &Request::ReadRecorderStatistics).unwrap();
        let error = DeadlineReader {
            io: MonitorIo::new_native(),
            stream: &mut server,
            deadline: Instant::now(),
        }
        .read(&mut [0; 1])
        .unwrap_err();
        let request = read_request_with_timeout(
            &MonitorIo::new_native(),
            &mut server,
            &AtomicBool::new(false),
            Duration::from_secs(1),
            ReadTimeout::Idle,
        )
        .unwrap();
        assert_eq!(
            (error.kind(), request),
            (io::ErrorKind::TimedOut, (1, Request::ReadRecorderStatistics))
        );
    }

    #[test]
    fn unclassified_io_failure_preserves_the_protocol_error() {
        let error = classify_read_error(
            seismograph_protocol::Error::Io(io::Error::new(io::ErrorKind::PermissionDenied, "socket denied")),
            false,
            ReadTimeout::Idle,
        );
        assert!(matches!(
            error,
            ClientError::Protocol(seismograph_protocol::Error::Io(error))
                if error.kind() == io::ErrorKind::PermissionDenied && error.to_string() == "socket denied"
        ));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn client_spawn_failure_releases_registration_and_reports_error() {
        let clients = Arc::new(ActiveClients::new());
        let (mut peer, stream) = connected_pair();
        let registration = clients.register(&stream).unwrap().unwrap();
        let last_error = Arc::new(Mutex::new(None));
        spawn_client_thread_with(
            MonitorIo::new_native(),
            stream,
            test_descriptor(),
            Arc::new(AtomicBool::new(false)),
            registration,
            &last_error,
            |_operation| Err(io::Error::other("worker unavailable")),
        );
        assert_eq!(
            (
                last_error.lock().unwrap().clone(),
                clients.state.lock().unwrap().streams.len(),
                connection_is_closed(&mut peer)
            ),
            (Some("worker unavailable".into()), 0, true)
        );
    }

    fn connected_pair() -> (Connection, Connection) {
        seismograph_protocol::testing::connected_pair()
    }

    #[cfg_attr(miri, ignore = "requires native local IPC")]
    #[test]
    fn connected_connection_reports_the_seismograph_crate_version_before_capture() {
        let descriptor = test_descriptor();
        let (mut client, server) = connected_pair();
        client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let worker_descriptor = descriptor.clone();
        let worker = thread::spawn(move || handle_client(&MonitorIo::new_native(), server, &worker_descriptor, &AtomicBool::new(false)));
        seismograph_protocol::write_request(&mut client, 1, &Request::Hello).unwrap();
        assert!(matches!(
            seismograph_protocol::read_response(&mut client).unwrap(),
            (1, Response::Hello { instance_id, .. }) if instance_id == descriptor.instance_id
        ));
        seismograph_protocol::write_request(&mut client, 2, &Request::ReadServerVersion).unwrap();
        assert_eq!(
            seismograph_protocol::read_response(&mut client).unwrap(),
            (2, Response::ServerVersion(env!("CARGO_PKG_VERSION").into()))
        );
        client.close().unwrap();
        worker.join().unwrap().unwrap();
    }

    fn test_descriptor() -> MonitorDescriptor {
        MonitorDescriptor {
            name: "test".into(),
            instance: None,
            process_id: 1,
            instance_id: InstanceId::from_bytes([1; 16]),

            owner: seismograph_protocol::monitor::MonitorOwner::current().unwrap(),

            endpoint: seismograph_protocol::monitor::MonitorEndpoint::for_instance(
                &std::env::temp_dir(),
                &seismograph_protocol::monitor::MonitorOwner::current().unwrap(),
                1,
                InstanceId::from_bytes([1; 16]),
            ),
        }
    }

    fn connection_is_closed(stream: &mut Connection) -> bool {
        stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut byte = [0];
        match stream.read(&mut byte) {
            Ok(count) => count == 0,
            Err(error) => matches!(
                classify_read_error(seismograph_protocol::Error::Io(error), false, ReadTimeout::Idle),
                ClientError::Disconnected
            ),
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // OS scheduling makes the number of polling iterations nondeterministic.
    fn wait_for_active_clients(monitor: &Monitor, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let active_count = monitor
                .active_clients
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .streams
                .len();
            if active_count == expected {
                break;
            }
            assert!(Instant::now() < deadline, "listener did not accept the expected clients");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn connection_check_does_not_mistake_pending_data_or_would_block_for_disconnect() {
        let (mut client, mut server) = connected_pair();
        server.set_read_timeout(Some(Duration::from_millis(1))).unwrap();
        let idle_is_closed = connection_is_closed(&mut server);
        client.write_all(&[1]).unwrap();

        let readable_is_closed = connection_is_closed(&mut server);
        assert_eq!((idle_is_closed, readable_is_closed), (false, false));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    #[expect(
        clippy::useless_let_if_seq,
        reason = "The end-to-end monitor scenario stays together; default initialization keeps impossible response-shape branches out of line coverage"
    )]
    fn monitor_configures_recording_and_returns_snapshot() {
        let _test = crate::recorder::TEST_LOCK.lock().unwrap();
        let monitor = Monitor::builder().isolated().name("monitor-test").instance("one").start().unwrap();
        let descriptor = monitor.descriptor().clone();
        assert!(monitor.descriptor_path.exists());

        let mut stream = Connection::connect(
            &seismograph_protocol::io::MonitorIo::new_native(),
            &descriptor,
            Duration::from_secs(2),
        )
        .unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        seismograph_protocol::write_request(&mut stream, 1, &Request::Hello).unwrap();
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
            Some(crate::recorder::event::Record::object(
                crate::recorder::event::EventKind::ArcDeref,
                crate::recorder::event::ObjectId::new(42),
            ))
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
        let path = monitor.descriptor_path.clone();
        drop(monitor);
        assert!(!path.exists());
        let _ = fs::remove_file(path);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_identity_debug_and_last_error_are_exposed() {
        const BUILDER: Builder = Monitor::builder();
        let default = Builder::default();
        assert_eq!((&BUILDER.name, &BUILDER.instance), (&default.name, &default.instance));

        assert!(matches!(
            Monitor::builder().isolated().name("").start(),
            Err(Error::InvalidIdentity)
        ));
        assert!(matches!(
            Monitor::builder().isolated().name("valid").instance("").start(),
            Err(Error::InvalidIdentity)
        ));
        assert_eq!(
            default_application_name(&MonitorIo::new_native()).unwrap(),
            std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().into_owned()
        );

        let monitor = Monitor::builder().isolated().name("monitor-debug").start().unwrap();
        assert_eq!(monitor.last_error(), None);
        let debug = format!("{monitor:?}");
        assert!(debug.contains("Monitor"));
        assert!(debug.contains("descriptor_path"));

        let mut stream = Connection::connect(
            &seismograph_protocol::io::MonitorIo::new_native(),
            monitor.descriptor(),
            Duration::from_secs(2),
        )
        .unwrap();
        seismograph_protocol::write_request(&mut stream, 1, &Request::ReadRecorderStatistics).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(error) = monitor.last_error() {
                assert_eq!(error, "monitor handshake is required");
                break;
            }
            assert!(Instant::now() < deadline, "listener did not publish its client error");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn handshakes_require_no_authentication_secret() {
        let _test = crate::recorder::TEST_LOCK.lock().unwrap();
        let descriptor = test_descriptor();

        let (mut client, server) = connected_pair();
        seismograph_protocol::write_request(&mut client, 1, &Request::ReadRecorderStatistics).unwrap();
        assert!(matches!(
            handle_client(&MonitorIo::new_native(), server, &descriptor, &AtomicBool::new(false)),
            Err(ClientError::HandshakeRequired)
        ));

        let (mut client, server) = connected_pair();
        seismograph_protocol::write_request(&mut client, 2, &Request::Hello).unwrap();
        handle_client(&MonitorIo::new_native(), server, &descriptor, &AtomicBool::new(true)).unwrap();
        assert_eq!(
            seismograph_protocol::read_response(&mut client).unwrap(),
            (
                2,
                Response::Hello {
                    instance_id: descriptor.instance_id,
                    recording: {
                        let configuration = crate::recorder::configuration();
                        protocol_recording_configuration(
                            configuration.allocations,
                            configuration.general_events,
                            configuration.arc_dereferences,
                            configuration.runtime_tasks,
                            configuration.io,
                            configuration.event_capacity_per_thread.get(),
                        )
                    },
                }
            )
        );
    }

    #[test]
    fn connected_requests_handle_repeat_hello_and_all_snapshot_modes() {
        let _test = crate::recorder::TEST_LOCK.lock().unwrap();
        assert_eq!(
            connected_response(&Request::Hello),
            Response::Error("handshake already completed".into())
        );

        for event_buffers in [
            EventBufferDisposition::Retain,
            EventBufferDisposition::Clear,
            EventBufferDisposition::Release,
        ] {
            assert!(matches!(
                connected_response(&Request::CaptureSnapshot(SnapshotOptions { event_buffers })),
                Response::Snapshot(_)
            ));
        }

        let snapshot = crate::snapshot(crate::snapshot::SnapshotOptions::default()).unwrap();
        let address = snapshot.as_bytes().as_ptr();
        assert!(matches!(
            snapshot_response(Ok(snapshot)),
            Response::Snapshot(bytes) if std::ptr::eq(bytes.as_ptr(), address)
        ));
    }

    #[cfg_attr(
        miri,
        ignore = "native tests cover the local IPC handshake path; Miri retains the socket-free lifecycle tests"
    )]
    #[test]
    fn lifecycle_commands_require_a_completed_handshake() {
        for request in [
            Request::CaptureSnapshotAndStop,
            Request::ClearEventBuffers,
            Request::ReadRecorderActivity,
        ] {
            let (mut client, server) = connected_pair();
            seismograph_protocol::write_request(&mut client, 1, &request).unwrap();
            let descriptor = test_descriptor();
            assert!(matches!(
                handle_client(&MonitorIo::new_native(), server, &descriptor, &AtomicBool::new(false)),
                Err(ClientError::HandshakeRequired)
            ));
        }
    }

    #[test]
    fn lifecycle_commands_preserve_capture_and_do_not_restart_recording() {
        use crate::recorder::event::{EventKind, ObjectId, Record};
        use crate::recorder::{Configuration, EventBufferCapacity, RecordingPolicy};
        let _test = crate::recorder::TEST_LOCK.lock().unwrap();
        let configuration = Configuration {
            general_events: RecordingPolicy::all(false),
            cache: RecordingPolicy::all(false),
            event_capacity_per_thread: EventBufferCapacity::new(64).unwrap(),
            ..Default::default()
        };
        crate::recorder::configure(configuration);
        crate::record(crate::recorder::event::EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
        });
        assert_eq!(connected_response(&Request::ClearEventBuffers), Response::Acknowledged);
        assert_eq!(crate::recorder::configuration(), configuration);
        crate::record(crate::recorder::event::EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(2)))
        });
        let bytes = snapshot_bytes(connected_response(&Request::CaptureSnapshotAndStop));
        let decoded = crate::snapshot::decode(&bytes).unwrap();
        assert_eq!(decoded.events.events.len(), 1);
        assert!(!crate::recorder::recording_enabled());
        crate::recorder::configure(Configuration::default());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn stopped_client_and_protocol_failures_are_reported() {
        let (mut client, mut server) = connected_pair();
        assert!(matches!(
            read_request_with_timeout(
                &MonitorIo::new_native(),
                &mut server,
                &AtomicBool::new(true),
                Duration::from_millis(1),
                ReadTimeout::Idle
            ),
            Err(ClientError::Stopped)
        ));

        client.write_all(b"not a protocol frame").unwrap();
        client.close().unwrap();
        assert!(matches!(
            read_request_with_timeout(
                &MonitorIo::new_native(),
                &mut server,
                &AtomicBool::new(false),
                Duration::from_secs(1),
                ReadTimeout::Idle
            ),
            Err(ClientError::Protocol(_))
        ));
    }

    #[cfg_attr(miri, ignore = "requires native local IPC; native and careful suites cover response deadlines")]
    #[test]
    fn response_write_deadline_is_absolute() {
        let (_client, mut server) = connected_pair();
        let writer = DeadlineWriter {
            io: MonitorIo::new_native(),
            stream: &mut server,
            deadline: Instant::now(),
        };

        assert_eq!(writer.apply_remaining_timeout().unwrap_err().kind(), io::ErrorKind::TimedOut);

        let (client, mut server) = connected_pair();
        drop(client);
        assert!(matches!(
            read_request_with_timeout(
                &MonitorIo::new_native(),
                &mut server,
                &AtomicBool::new(false),
                Duration::from_secs(1),
                ReadTimeout::Idle
            ),
            Err(ClientError::Disconnected)
        ));

        let (_client, mut server) = connected_pair();
        assert!(matches!(
            read_request_with_timeout(
                &MonitorIo::new_native(),
                &mut server,
                &AtomicBool::new(false),
                Duration::from_millis(1),
                ReadTimeout::Idle
            ),
            Err(ClientError::IdleTimedOut)
        ));

        let (client, mut server) = connected_pair();
        server.close().unwrap();
        drop(client);
        assert!(matches!(
            read_request_with_timeout(
                &MonitorIo::new_native(),
                &mut server,
                &AtomicBool::new(true),
                Duration::from_secs(1),
                ReadTimeout::Idle
            ),
            Err(ClientError::Stopped | ClientError::Disconnected)
        ));
    }

    #[cfg_attr(miri, ignore = "requires native local IPC; native and careful suites cover response deadlines")]
    #[test]
    fn response_flush_respects_the_absolute_deadline() {
        let (_client, mut server) = connected_pair();
        let mut writer = DeadlineWriter::new(&MonitorIo::new_native(), &mut server, Duration::from_secs(1));
        writer.flush().unwrap();
        writer.deadline = Instant::now();

        assert_eq!(writer.flush().unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn partial_handshake_frame_timeout_closes_connection() {
        let descriptor = test_descriptor();
        let (mut client, server) = connected_pair();
        let worker = thread::spawn(move || {
            handle_client_with_timeouts(
                &MonitorIo::new_native(),
                server,
                &descriptor,
                &AtomicBool::new(false),
                Duration::from_millis(50),
                Duration::from_secs(1),
            )
        });

        client.write_all(b"SGMP").unwrap();
        let result = worker.join().unwrap();
        assert_eq!(
            (
                matches!(result, Err(ClientError::HandshakeTimedOut)),
                connection_is_closed(&mut client),
            ),
            (true, true)
        );
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn connected_idle_timeout_closes_connection() {
        let descriptor = test_descriptor();

        let (mut client, server) = connected_pair();
        let worker = thread::spawn(move || {
            handle_client_with_timeouts(
                &MonitorIo::new_native(),
                server,
                &descriptor,
                &AtomicBool::new(false),
                Duration::from_secs(1),
                Duration::from_millis(50),
            )
        });

        seismograph_protocol::write_request(&mut client, 1, &Request::Hello).unwrap();
        let hello = seismograph_protocol::read_response(&mut client).unwrap();
        let result = worker.join().unwrap();
        assert_eq!(
            (
                matches!(hello, (1, Response::Hello { .. })),
                result.is_ok(),
                connection_is_closed(&mut client),
            ),
            (true, true, true)
        );
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn blocked_client_does_not_block_another_client() {
        let monitor = Monitor::builder().isolated().name("monitor-concurrent-clients").start().unwrap();
        let descriptor = monitor.descriptor().clone();
        let mut blocked = Connection::connect(
            &seismograph_protocol::io::MonitorIo::new_native(),
            &descriptor,
            Duration::from_secs(2),
        )
        .unwrap();
        blocked.write_all(b"SGMP").unwrap();
        wait_for_active_clients(&monitor, 1);

        let mut responsive = Connection::connect(
            &seismograph_protocol::io::MonitorIo::new_native(),
            &descriptor,
            Duration::from_secs(2),
        )
        .unwrap();
        responsive.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        responsive.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
        seismograph_protocol::write_request(&mut responsive, 1, &Request::Hello).unwrap();

        let response_received = matches!(
            seismograph_protocol::read_response(&mut responsive),
            Ok((1, Response::Hello { .. }))
        );
        drop(monitor);
        assert_eq!((response_received, connection_is_closed(&mut blocked)), (true, true));
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native local IPC")]
    fn active_client_limit_rejects_excess_connections() {
        let clients = Arc::new(ActiveClients::new());
        let mut registrations = Vec::new();
        let mut peers = Vec::new();
        for _ in 0..MAX_CONCURRENT_CLIENTS {
            let (peer, stream) = connected_pair();
            peers.push(peer);
            registrations.push(clients.register(&stream).unwrap());
        }
        let (_peer, excess) = connected_pair();

        assert_eq!(
            (
                peers.len(),
                registrations.iter().all(Option::is_some),
                clients.register(&excess).unwrap().is_none(),
            ),
            (MAX_CONCURRENT_CLIENTS, true, true)
        );
    }

    #[test]
    fn conversion_helpers_preserve_policies_and_errors() {
        let policy = crate::recorder::RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            event_sampling: crate::recorder::EventSampling::one_in(7).unwrap(),
        };
        let configuration = protocol_recording_configuration(policy, policy, policy, policy, policy, usize::MAX);
        assert_eq!(configuration.event_capacity_per_thread, u32::MAX);
        assert_eq!(configuration.allocations.sampling_one_in, 7);
        assert_eq!(recorder_policy(configuration.allocations), policy);
        assert_eq!(
            snapshot_response(Err(crate::Error::new("capture failed"))),
            Response::Error("capture failed".into())
        );
        assert!(listener_accept_should_retry(io::ErrorKind::WouldBlock));
        assert!(!listener_accept_should_retry(io::ErrorKind::Other));

        assert!(matches!(
            classify_read_error(
                seismograph_protocol::Error::Io(io::Error::new(io::ErrorKind::TimedOut, "timeout")),
                false,
                ReadTimeout::Handshake
            ),
            ClientError::HandshakeTimedOut
        ));
        assert!(matches!(
            classify_read_error(
                seismograph_protocol::Error::Io(io::Error::new(io::ErrorKind::WouldBlock, "timeout")),
                false,
                ReadTimeout::Idle
            ),
            ClientError::IdleTimedOut
        ));
        assert!(matches!(
            classify_read_error(
                seismograph_protocol::Error::Io(io::Error::other("protocol")),
                true,
                ReadTimeout::Idle
            ),
            ClientError::Stopped
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
            ClientError::HandshakeTimedOut,
            ClientError::IdleTimedOut,
            ClientError::Disconnected,
            ClientError::Stopped,
        ];
        let expected = [
            "socket failed",
            "invalid Seismograph monitor message",
            "monitor handshake is required",
            "monitor handshake timed out",
            "monitor client idle timeout expired",
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
            Error::Random(getrandom::Error::UNSUPPORTED),
            Error::Protocol(protocol_error()),
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
        let expected = [
            "monitor application and instance names must not be empty".to_owned(),
            "failed to bind Seismograph local monitor: socket failed".to_owned(),
            "failed to configure Seismograph monitor listener: socket failed".to_owned(),
            format!("failed to generate Seismograph monitor identity: {}", getrandom::Error::UNSUPPORTED),
            "Seismograph monitor protocol failed: invalid Seismograph monitor message".to_owned(),
            "failed to publish monitor descriptor monitor: invalid Seismograph monitor message".to_owned(),
            "failed to publish monitor descriptor temporary as monitor: socket failed".to_owned(),
            "failed to start Seismograph monitor thread: socket failed".to_owned(),
        ];
        for (error, expected) in startup_cases.into_iter().zip(expected) {
            assert_eq!(error.to_string(), expected);
            let expected_source = !matches!(error, Error::InvalidIdentity | Error::Random(_));
            assert_eq!(std::error::Error::source(&error).is_some(), expected_source);
        }
    }

    #[test]
    fn builder_cleans_up_after_listener_spawn_failure() {
        let (io, control) = MonitorIo::new_mocked();
        let observed = control.clone();
        let error = Monitor::builder()
            .io(io.clone())
            .name("spawn-failure")
            .start_with(|_io, _listener, _descriptor, _stop, _client, _last_error, descriptor_path| {
                assert!(observed.file_bytes(descriptor_path).is_some());
                Err(Error::Spawn(io::Error::other("injected")))
            })
            .unwrap_err();
        assert!(matches!(error, Error::Spawn(_)));
        assert_eq!(Directory::current(&io).unwrap().descriptors().unwrap().count(), 0);
        assert!(
            control
                .operations()
                .contains(&seismograph_protocol::io::IoOperation::RemoveDescriptor)
        );
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn connected_client_stops_after_handshake_and_propagates_bad_frames() {
        let descriptor = MonitorDescriptor {
            name: "test".into(),
            instance: None,
            process_id: 1,
            instance_id: InstanceId::from_bytes([1; 16]),

            owner: seismograph_protocol::monitor::MonitorOwner::current().unwrap(),

            endpoint: seismograph_protocol::monitor::MonitorEndpoint::for_instance(
                &std::env::temp_dir(),
                &seismograph_protocol::monitor::MonitorOwner::current().unwrap(),
                1,
                InstanceId::from_bytes([1; 16]),
            ),
        };

        let (mut client, server) = connected_pair();
        seismograph_protocol::write_request(&mut client, 1, &Request::Hello).unwrap();
        handle_client(&MonitorIo::new_native(), server, &descriptor, &AtomicBool::new(true)).unwrap();
        assert!(matches!(
            seismograph_protocol::read_response(&mut client).unwrap(),
            (1, Response::Hello { .. })
        ));

        let (mut client, server) = connected_pair();
        let server_descriptor = descriptor;
        let server_thread =
            thread::spawn(move || handle_client(&MonitorIo::new_native(), server, &server_descriptor, &AtomicBool::new(false)));
        seismograph_protocol::write_request(&mut client, 2, &Request::Hello).unwrap();
        let _hello = seismograph_protocol::read_response(&mut client).unwrap();
        client.write_all(&[0; 20]).unwrap();
        assert!(matches!(server_thread.join().unwrap(), Err(ClientError::Protocol(_))));
    }
}
