// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use ::seismograph::recorder::EventSampling;
use bytes::Bytes;
use seismograph_protocol::io::MonitorIo;
use seismograph_protocol::message::{EventBufferDisposition, RecordingConfiguration, RecordingPolicy, SnapshotOptions};
use seismograph_protocol::monitor::MonitorDescriptor;
use tokio::sync::Semaphore;
use tonic::{Request, Response, Status};

use super::super::client;
use super::super::data::CapturedSnapshot;
use super::super::filter::{FilterSpec, RuntimeStackMode};
use super::{api, report};

pub(super) const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
const MAX_SNAPSHOTS: usize = 4;
const CAPTURE_TIMEOUT: Duration = Duration::from_mins(2);
type Rpc<T> = Result<Response<T>, Status>;

#[derive(Clone)]
pub(super) struct Monitor {
    state: Arc<Mutex<State>>,
    discovery: Arc<Semaphore>,
    control: Arc<Semaphore>,
    activity: Arc<Semaphore>,
}

#[derive(Default)]
struct State {
    targets: HashMap<String, MonitorDescriptor>,
    captures: VecDeque<Capture>,
    revision: u64,
}

struct Capture {
    id: String,
    name: String,
    raw: Bytes,
    original: Arc<CapturedSnapshot>,
    views: VecDeque<(String, Arc<CapturedSnapshot>)>,
    saved_path: String,
    save_warning: Option<String>,
}

impl Default for Monitor {
    fn default() -> Self {
        Self {
            state: Arc::default(),
            discovery: Arc::new(Semaphore::new(1)),
            control: Arc::new(Semaphore::new(1)),
            activity: Arc::new(Semaphore::new(4)),
        }
    }
}

impl Monitor {
    fn lock(&self) -> Result<MutexGuard<'_, State>, Status> {
        self.state
            .lock()
            .map_err(|_poisoned| Status::internal("Monitor state is unavailable. Restart the monitor."))
    }

    fn target(&self, id: &str) -> Result<MonitorDescriptor, Status> {
        self.lock()?
            .targets
            .get(id)
            .cloned()
            .ok_or_else(|| Status::not_found("Application not found. Refresh the application list."))
    }

    async fn job<T: Send + 'static>(
        &self,
        gate: &Arc<Semaphore>,
        operation: impl FnOnce(Self) -> Result<T, Status> + Send + 'static,
    ) -> Rpc<T> {
        let permit = Arc::clone(gate)
            .try_acquire_owned()
            .map_err(|_no_permit| Status::resource_exhausted("The monitor is busy. Try again."))?;
        let monitor = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation(monitor)
        })
        .await
        .map_err(|_join_error| Status::internal("The monitor worker stopped."))??;
        Ok(Response::new(result))
    }

    fn store(&self, name: String, raw: Bytes, saved_path: String, save_warning: Option<String>) -> Result<api::Snapshot, Status> {
        let invalid = |message: String| {
            if saved_path.is_empty() {
                Status::invalid_argument(message)
            } else {
                Status::invalid_argument(format!("Snapshot saved to {saved_path}, but inspection failed: {message}"))
            }
        };
        let decoded = ::seismograph::snapshot::decode(&raw).map_err(|error| invalid(error.to_string()))?;
        let original: Arc<CapturedSnapshot> = Arc::from(super::super::snapshot::prepare(decoded).map_err(invalid)?);
        let mut state = self.lock()?;
        let id = state.next_id();
        let capture = Capture {
            id,
            name,
            raw,
            original,
            views: VecDeque::new(),
            saved_path,
            save_warning,
        };
        let snapshot = capture.summary("", &capture.original);
        let previous = if state.captures.len() == MAX_SNAPSHOTS {
            state.captures.pop_front()
        } else {
            None
        };
        state.captures.push_back(capture);
        drop(state);
        drop(previous);
        Ok(snapshot)
    }

    pub(super) async fn open_file(&self, name: String, data: Bytes) -> Rpc<api::Snapshot> {
        if data.len() > MAX_SNAPSHOT_BYTES || name.len() > 255 {
            return Err(Status::invalid_argument(
                "Use a snapshot of at most 64 MiB with a name of at most 255 bytes.",
            ));
        }
        self.job(&self.control, move |monitor| {
            let name = if name.is_empty() { "capture.seismograph".into() } else { name };
            monitor.store(name, data, String::new(), None)
        })
        .await
    }

    pub(super) fn file(&self, id: &str) -> Result<(String, Bytes), Status> {
        let state = self.lock()?;
        let capture = state.capture(id)?;
        Ok((capture.name.clone(), capture.raw.clone()))
    }
}

impl Capture {
    fn summary(&self, view_id: &str, data: &CapturedSnapshot) -> api::Snapshot {
        let mut summary = report::summary(&self.id, &self.name, data);
        summary.view_id = view_id.into();
        summary.saved_path.clone_from(&self.saved_path);
        summary.warnings.extend(self.save_warning.iter().cloned());
        summary
    }

    fn view(&self, id: &str) -> Result<Arc<CapturedSnapshot>, Status> {
        if id.is_empty() {
            return Ok(Arc::clone(&self.original));
        }
        self.views
            .iter()
            .find(|(view, _)| view == id)
            .map(|(_, snapshot)| Arc::clone(snapshot))
            .ok_or_else(|| Status::not_found("This filtered view expired. Apply or reset the filter."))
    }
}

impl State {
    fn next_id(&mut self) -> String {
        self.revision += 1;
        self.revision.to_string()
    }

    fn capture(&self, id: &str) -> Result<&Capture, Status> {
        self.captures.iter().find(|capture| capture.id == id).ok_or_else(snapshot_missing)
    }
}

fn snapshot_missing() -> Status {
    Status::not_found("This snapshot is no longer open. Open its saved file to inspect it again.")
}

#[tonic::async_trait]
impl api::monitor_server::Monitor for Monitor {
    async fn discover(&self, _: Request<api::Empty>) -> Rpc<api::Applications> {
        self.job(&self.discovery, |monitor| {
            let io = MonitorIo::new_native();
            let discovery = client::discover(&io).map_err(|error| monitor_error(&error))?;
            let mut targets = HashMap::new();
            let mut applications = Vec::new();
            for entry in discovery.instances {
                let descriptor = entry.descriptor;
                let id = descriptor.instance_id.to_hex();
                applications.push(api::Application {
                    id: id.clone(),
                    name: descriptor.name.clone(),
                    instance: descriptor.instance.clone().unwrap_or_default(),
                    pid: descriptor.process_id,
                    owner: descriptor.owner.to_string(),
                    server_version: entry.server_version.unwrap_or_default(),
                    recording: Some(recording(entry.recording)),
                });
                targets.insert(id, descriptor);
            }
            monitor.lock()?.targets = targets;
            Ok(api::Applications {
                applications,
                warnings: discovery.warnings,
                monitor_version: env!("CARGO_PKG_VERSION").to_owned(),
            })
        })
        .await
    }

    async fn activity(&self, request: Request<api::Target>) -> Rpc<api::RecorderActivity> {
        self.job(&self.activity, move |monitor| {
            let descriptor = monitor.target(&request.into_inner().id)?;
            let activity = client::recorder_activity(&MonitorIo::new_native(), &descriptor).map_err(|error| monitor_error(&error))?;
            let stats = activity.statistics;
            let classes = activity.class_events;
            Ok(api::RecorderActivity {
                recording: Some(recording(stats.recording)),
                session_id: activity.session_id,
                total_events: stats.total_events,
                retained_events: stats.retained_events,
                lost_events: stats.lost_events,
                allocated_bytes: stats.allocated_bytes,
                class_events: vec![
                    classes.allocations,
                    classes.general_events,
                    classes.arc_dereferences,
                    classes.runtime_tasks,
                    classes.io,
                    classes.cache,
                ],
                thread_count: stats.thread_count,
                threads: activity
                    .threads
                    .into_iter()
                    .map(|thread| api::LiveThread {
                        id: thread.thread_id,
                        name: thread.name,
                        total_events: thread.total_events,
                        retained_events: thread.retained_events,
                        lost_events: thread.lost_events,
                        retired: thread.retired,
                        capacity: thread.event_capacity,
                    })
                    .collect(),
            })
        })
        .await
    }

    async fn record(&self, request: Request<api::RecordRequest>) -> Rpc<api::Recording> {
        let request = request.into_inner();
        let configuration = configuration(request.recording)?;
        self.job(&self.control, move |monitor| {
            let target = monitor.target(&request.id)?;
            client::set_recording(&MonitorIo::new_native(), &target, configuration)
                .map(recording)
                .map_err(|error| monitor_error(&error))
        })
        .await
    }

    async fn clear(&self, request: Request<api::Target>) -> Rpc<api::Empty> {
        self.job(&self.control, move |monitor| {
            let target = monitor.target(&request.into_inner().id)?;
            client::clear_event_buffers(&MonitorIo::new_native(), &target).map_err(|error| monitor_error(&error))?;
            Ok(api::Empty {})
        })
        .await
    }

    async fn capture(&self, request: Request<api::CaptureRequest>) -> Rpc<api::Snapshot> {
        let request = request.into_inner();
        let mode = api::CaptureMode::try_from(request.mode).map_err(|_invalid_mode| Status::invalid_argument("Invalid capture mode."))?;
        self.job(&self.control, move |monitor| {
            let target = monitor.target(&request.id)?;
            let io = MonitorIo::new_native();
            let options = match mode {
                api::CaptureMode::Unspecified => return Err(Status::invalid_argument("Select a capture mode.")),
                api::CaptureMode::Continue => Some(SnapshotOptions::default()),
                api::CaptureMode::Clear => Some(SnapshotOptions {
                    event_buffers: EventBufferDisposition::Clear,
                }),
                api::CaptureMode::Stop => None,
            };
            // A destructive capture must preserve the recording even when it is too large to inspect here.
            let limit = if mode == api::CaptureMode::Continue {
                MAX_SNAPSHOT_BYTES
            } else {
                u32::MAX as usize
            };
            let raw = client::capture_snapshot_bounded(&io, &target, options, CAPTURE_TIMEOUT, limit)
                .map_err(|error| capture_error(&error, mode))?;
            if raw.len() > MAX_SNAPSHOT_BYTES {
                let path = client::save_snapshot(&io, &target, &raw)
                    .map_err(|error| Status::internal(format!("Recorder changed state, but saving the large capture failed: {error}")))?;
                return Err(Status::resource_exhausted(format!(
                    "The proxy saved the capture to {}. Open it with seismograph view because it exceeds the web monitor's 64 MiB limit.",
                    path.display()
                )));
            }
            let (name, path, warning) = match client::save_snapshot(&io, &target, &raw) {
                Ok(path) => (
                    path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                    path.display().to_string(),
                    None,
                ),
                Err(error) => (
                    format!("{}-{}.seismograph", target.name, target.process_id),
                    String::new(),
                    Some(format!("Could not save the capture: {error}. Download it before replacing it.")),
                ),
            };
            monitor.store(name, Bytes::from(raw), path, warning)
        })
        .await
    }

    async fn open(&self, request: Request<api::NativeSnapshot>) -> Rpc<api::Snapshot> {
        let request = request.into_inner();
        self.open_file(request.name, Bytes::from(request.data)).await
    }

    async fn inspect(&self, request: Request<api::InspectRequest>) -> Rpc<api::Page> {
        let request = request.into_inner();
        self.job(&self.activity, move |monitor| {
            let snapshot = monitor.lock()?.capture(&request.id)?.view(&request.view_id)?;
            report::inspect(&snapshot, &request)
        })
        .await
    }

    async fn filter(&self, request: Request<api::FilterRequest>) -> Rpc<api::Snapshot> {
        let request = request.into_inner();
        let mode = if request.full_stack {
            RuntimeStackMode::Spawn
        } else {
            RuntimeStackMode::Event
        };
        let filter = FilterSpec::parse(&request.include, &request.exclude, request.include_unknown, mode)
            .map_err(|error| Status::invalid_argument(error.to_string()))?;
        self.job(&self.control, move |monitor| {
            let original = Arc::clone(&monitor.lock()?.capture(&request.id)?.original);
            if !filter.is_active() {
                return Ok(monitor.lock()?.capture(&request.id)?.summary("", &original));
            }
            let index = original
                .filter_index
                .as_ref()
                .ok_or_else(|| Status::failed_precondition("This snapshot has no stack index."))?;
            let current: Arc<CapturedSnapshot> = Arc::from(index.render(&filter));
            let mut state = monitor.lock()?;
            let view = state.next_id();
            let capture = state
                .captures
                .iter_mut()
                .find(|capture| capture.id == request.id)
                .ok_or_else(snapshot_missing)?;
            let summary = capture.summary(&view, &current);
            let previous = if capture.views.len() == 2 {
                capture.views.pop_front()
            } else {
                None
            };
            capture.views.push_back((view, current));
            drop(state);
            drop(previous);
            Ok(summary)
        })
        .await
    }

    async fn download(&self, request: Request<api::SnapshotId>) -> Rpc<api::NativeSnapshot> {
        self.job(&self.activity, move |monitor| {
            let (name, data) = monitor.file(&request.into_inner().id)?;
            Ok(api::NativeSnapshot { name, data: data.to_vec() })
        })
        .await
    }
}

fn capture_error(error: &super::super::Error, mode: api::CaptureMode) -> Status {
    let detail = if matches!(mode, api::CaptureMode::Stop | api::CaptureMode::Clear) {
        " The recorder might already have applied the requested change. Read its current state before retrying."
    } else {
        ""
    };
    match error {
        super::super::Error::Protocol(seismograph_protocol::Error::MessageTooLarge) => {
            Status::resource_exhausted(format!("The capture exceeds the web monitor's 64 MiB limit.{detail}"))
        }
        _ => Status::unavailable(format!("Capture failed: {error}.{detail}")),
    }
}

fn monitor_error(error: &super::super::Error) -> Status {
    let message = error.to_string();
    match error {
        super::super::Error::Remote(_) => Status::failed_precondition(message),
        _ => Status::unavailable(message),
    }
}

fn recording(value: RecordingConfiguration) -> api::Recording {
    let policy = |value: RecordingPolicy| {
        Some(api::Policy {
            enabled: value.enabled,
            capture_backtraces: value.capture_backtraces,
            sampling_one_in: value.sampling_one_in,
        })
    };
    api::Recording {
        allocations: policy(value.allocations),
        general_events: policy(value.general_events),
        arc_dereferences: policy(value.arc_dereferences),
        runtime_tasks: policy(value.runtime_tasks),
        io: policy(value.io),
        cache: policy(value.cache),
        event_capacity_per_thread: value.event_capacity_per_thread,
    }
}

fn configuration(value: Option<api::Recording>) -> Result<RecordingConfiguration, Status> {
    let value = value.ok_or_else(|| Status::invalid_argument("Recording policies are required."))?;
    let capacity = value.event_capacity_per_thread;
    if !(64..=1_048_576).contains(&capacity) || !capacity.is_power_of_two() {
        return Err(Status::invalid_argument(
            "Events per thread must be a power of two from 64 to 1,048,576.",
        ));
    }
    let policy = |value: Option<api::Policy>| {
        let value = value.ok_or_else(|| Status::invalid_argument("Send all six recording policies."))?;
        if usize::try_from(value.sampling_one_in)
            .ok()
            .and_then(EventSampling::one_in)
            .is_none()
        {
            return Err(Status::invalid_argument("Each sampling denominator must be from 1 to 1,048,576."));
        }
        Ok(RecordingPolicy {
            enabled: value.enabled,
            capture_backtraces: value.capture_backtraces,
            sampling_one_in: value.sampling_one_in,
        })
    };
    Ok(RecordingConfiguration {
        allocations: policy(value.allocations)?,
        general_events: policy(value.general_events)?,
        arc_dereferences: policy(value.arc_dereferences)?,
        runtime_tasks: policy(value.runtime_tasks)?,
        io: policy(value.io)?,
        cache: policy(value.cache)?,
        event_capacity_per_thread: value.event_capacity_per_thread,
    })
}
